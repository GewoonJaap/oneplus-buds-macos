//! macOS menu bar app for the buds. One worker thread owns the Bluetooth session;
//! the UI thread (tao event loop) never blocks on Bluetooth.
use buds::protocol::{decode, Battery, Cell, Eq, Event, Mode, Spatial};
use buds::session::Session;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use tao::event::{Event as TaoEvent, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug)]
enum UserEvent {
    /// Connection state: `None` = not connected; `Some(mode)` = connected (mode may be unknown).
    Connected(Option<Option<Mode>>),
    Mode(Option<Mode>),
    Battery(Battery),
    Eq(Option<Eq>),
    Spatial(Option<Spatial>),
    Menu(MenuEvent),
}

enum Cmd {
    SetMode(Mode),
    SetEq(Eq),
    SetSpatial(Spatial),
    Reconnect,
}

const RETRY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(30);

fn worker(rx: mpsc::Receiver<Cmd>, proxy: EventLoopProxy<UserEvent>) {
    let send = |e: UserEvent| {
        let _ = proxy.send_event(e);
    };
    loop {
        let mut session = match Session::open() {
            Ok(s) => s,
            Err(_) => {
                // Wait RETRY, but let a Reconnect/SetMode wake us early.
                match rx.recv_timeout(RETRY) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    _ => continue,
                }
            }
        };
        let mode = match session.query_mode() {
            Ok(m) => m,
            Err(_) => {
                send(UserEvent::Connected(None));
                std::thread::sleep(RETRY);
                continue;
            }
        };
        send(UserEvent::Connected(Some(mode)));
        if let Ok(b) = session.query_battery() {
            send(UserEvent::Battery(b));
        }
        if let Ok(e) = session.query_eq() {
            send(UserEvent::Eq(e));
        }
        if let Ok(m) = session.query_spatial() {
            send(UserEvent::Spatial(m));
        }

        let mut last_poll = Instant::now();
        'conn: loop {
            // Commands from the UI.
            loop {
                match rx.try_recv() {
                    Ok(Cmd::SetMode(m)) => match session.set_mode(m) {
                        Ok(confirmed) => send(UserEvent::Mode(confirmed)),
                        Err(_) => break 'conn,
                    },
                    Ok(Cmd::SetEq(e)) => match session.set_eq(e) {
                        Ok(now) => send(UserEvent::Eq(now)),
                        Err(_) => break 'conn,
                    },
                    Ok(Cmd::SetSpatial(m)) => match session.set_spatial(m).and_then(|_| session.query_spatial()) {
                        Ok(now) => send(UserEvent::Spatial(now)),
                        Err(_) => break 'conn,
                    },
                    Ok(Cmd::Reconnect) => break 'conn,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            // Notifications pushed by the buds.
            match session.link.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(raw) => {
                    match decode(&raw) {
                        Some(Event::Noise(m, _)) => send(UserEvent::Mode(m)),
                        Some(Event::Battery(b)) => send(UserEvent::Battery(b)),
                        Some(Event::Other(0x0504, p)) if !p.is_empty() => send(UserEvent::Eq(Eq::from_id(p[0]))),
                        Some(Event::Other(0x0510, p)) if !p.is_empty() => {
                            send(UserEvent::Spatial(Spatial::ALL.into_iter().find(|m| *m as u8 == p[0])))
                        }
                        _ => {}
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break 'conn,
            }
            // Keepalive / liveness check.
            if last_poll.elapsed() >= POLL {
                last_poll = Instant::now();
                match session.query_mode() {
                    Ok(m) => send(UserEvent::Mode(m)),
                    Err(_) => break 'conn,
                }
                if let Ok(b) = session.query_battery() {
                    send(UserEvent::Battery(b));
                }
            }
        }
        send(UserEvent::Connected(None));
        drop(session);
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn make_icon(connected: bool) -> Icon {
    const N: u32 = 22;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    let alpha: u8 = if connected { 255 } else { 110 };
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let r = (dx * dx + dy * dy).sqrt();
            // Headband: upper half ring. Ear cups: filled rounded rects at the sides.
            let band = dy <= 1.0 && (r - 8.0).abs() <= 1.2;
            let cup = dy > 0.0 && dy < 7.0 && dx.abs() >= 6.0 && dx.abs() <= 9.5;
            if band || cup {
                let i = ((y * N + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[0, 0, 0, alpha]);
            }
        }
    }
    Icon::from_rgba(px, N, N).expect("icon")
}

struct Ui {
    tray: TrayIcon,
    header: MenuItem,
    battery: MenuItem,
    items: Vec<(Mode, CheckMenuItem)>,
    eq_items: Vec<(Eq, CheckMenuItem)>,
    spatial_items: Vec<(Spatial, CheckMenuItem)>,
    reconnect: MenuItem,
    quit: MenuItem,
}

impl Ui {
    fn new() -> Self {
        let menu = Menu::new();
        let header = MenuItem::new("OnePlus Buds Pro 3", false, None);
        let battery = MenuItem::new("Not connected", false, None);
        let section = MenuItem::new("Noise Control", false, None);
        let items: Vec<(Mode, CheckMenuItem)> =
            [Mode::Off, Mode::Transparency, Mode::Adaptive, Mode::High, Mode::Medium, Mode::Low].iter().map(|&m| (m, CheckMenuItem::new(menu_label(m), false, false, None))).collect();
        let reconnect = MenuItem::new("Reconnect", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append(&header).unwrap();
        menu.append(&battery).unwrap();
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        menu.append(&section).unwrap();
        for (_, i) in &items {
            menu.append(i).unwrap();
        }
        let eq_items: Vec<(Eq, CheckMenuItem)> =
            Eq::ALL.iter().map(|&e| (e, CheckMenuItem::new(e.label(), false, false, None))).collect();
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        menu.append(&MenuItem::new("Equalizer", false, None)).unwrap();
        for (_, i) in &eq_items {
            menu.append(i).unwrap();
        }
        let spatial_items: Vec<(Spatial, CheckMenuItem)> =
            Spatial::ALL.iter().map(|&m| (m, CheckMenuItem::new(m.label(), false, false, None))).collect();
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        menu.append(&MenuItem::new("Spatial Audio", false, None)).unwrap();
        for (_, i) in &spatial_items {
            menu.append(i).unwrap();
        }
        menu.append(&PredefinedMenuItem::separator()).unwrap();
        menu.append(&reconnect).unwrap();
        menu.append(&quit).unwrap();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(make_icon(false))
            .with_icon_as_template(true)
            .with_tooltip("Buds")
            .build()
            .expect("tray icon");
        Ui { tray, header, battery, items, eq_items, spatial_items, reconnect, quit }
    }

    fn render(&self, connected: bool, mode: Option<Mode>, bat: Option<Battery>, eq: Option<Eq>, sp: Option<Spatial>) {
        let text = match (connected, bat) {
            (false, _) => "Not connected".to_string(),
            (true, None) => "Connected".to_string(),
            (true, Some(b)) => format!(
                "L {}   R {}   Case {}",
                fmt_cell(b.left),
                fmt_cell(b.right),
                fmt_cell(b.case)
            ),
        };
        self.battery.set_text(text);
        let _ = &self.header;
        for (m, item) in &self.items {
            item.set_enabled(connected);
            item.set_checked(connected && mode == Some(*m));
        }
        for (e, item) in &self.eq_items {
            item.set_enabled(connected);
            item.set_checked(connected && eq == Some(*e));
        }
        for (m, item) in &self.spatial_items {
            item.set_enabled(connected);
            item.set_checked(connected && sp == Some(*m));
        }
        let _ = self.tray.set_icon_with_as_template(Some(make_icon(connected)), true);
    }
}

fn menu_label(m: Mode) -> &'static str {
    match m {
        Mode::Off => "Off",
        Mode::Transparency => "Transparency",
        Mode::Adaptive => "Adaptive",
        Mode::High => "Noise Cancellation: High",
        Mode::Medium => "Noise Cancellation: Medium",
        Mode::Low => "Noise Cancellation: Low",
    }
}

fn fmt_cell(c: Option<Cell>) -> String {
    match c {
        Some(c) if c.charging => format!("{}%\u{26A1}", c.percent),
        Some(c) => format!("{}%", c.percent),
        None => "\u{2013}".to_string(),
    }
}

fn main() {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = event_loop.create_proxy();

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));

    let (cmd_tx, cmd_rx): (Sender<Cmd>, _) = mpsc::channel();
    let worker_proxy = proxy.clone();
    std::thread::spawn(move || worker(cmd_rx, worker_proxy));

    let mut ui: Option<Ui> = None;
    let mut connected = false;
    let mut mode: Option<Mode> = None;
    let mut bat: Option<Battery> = None;
    let mut eq: Option<Eq> = None;
    let mut sp: Option<Spatial> = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            // The tray icon must be created once the event loop is running.
            TaoEvent::NewEvents(StartCause::Init) => {
                let u = Ui::new();
                u.render(connected, mode, bat, eq, sp);
                ui = Some(u);
            }
            TaoEvent::UserEvent(ev) => {
                let Some(u) = ui.as_ref() else { return };
                match ev {
                    UserEvent::Connected(Some(m)) => {
                        connected = true;
                        mode = m;
                    }
                    UserEvent::Connected(None) => {
                        connected = false;
                        mode = None;
                        bat = None;
                        eq = None;
                        sp = None;
                    }
                    UserEvent::Mode(m) => {
                        // Mode updates only make sense while connected.
                        if connected {
                            mode = m;
                        }
                    }
                    UserEvent::Battery(b) => bat = Some(b),
                    UserEvent::Eq(e) => eq = e,
                    UserEvent::Spatial(m) => sp = m,
                    UserEvent::Menu(e) => {
                        if e.id == u.quit.id() {
                            *control_flow = ControlFlow::Exit;
                        } else if e.id == u.reconnect.id() {
                            let _ = cmd_tx.send(Cmd::Reconnect);
                        } else if let Some((x, _)) = u.eq_items.iter().find(|(_, i)| e.id == *i.id()) {
                            let _ = cmd_tx.send(Cmd::SetEq(*x));
                        } else if let Some((x, _)) = u.spatial_items.iter().find(|(_, i)| e.id == *i.id()) {
                            let _ = cmd_tx.send(Cmd::SetSpatial(*x));
                        } else if let Some((m, _)) = u.items.iter().find(|(_, i)| e.id == *i.id()) {
                            let _ = cmd_tx.send(Cmd::SetMode(*m));
                        }
                    }
                }
                // Re-render always: this also reverts the check mark macOS toggles on click
                // so the display reflects the mode the buds report, not the one requested.
                u.render(connected, mode, bat, eq, sp);
            }
            _ => {}
        }
    });
}
