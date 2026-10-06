//! Headless Bluetooth worker for the native Swift UI. Reads commands from stdin
//! (`mode <name>`, `eq <name>`, `spatial <name>`, `reconnect`), writes one JSON state
//! line per change to stdout and to ~/Library/Application Support/Buds/state.json.
use buds::protocol::{by_name, decode, name_of, parse_gestures, parse_switches, Action, Battery, Cell, Eq, Event, Gesture, Mode, Side, Spatial, SWITCH_GAME};
use buds::session::Session;
use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

enum Cmd {
    Mode(Mode),
    Eq(Eq),
    Spatial(Spatial),
    Game(bool),
    Gesture(Side, Gesture, Action),
    Hold(u8),
    Reconnect,
}

#[derive(Default, Clone)]
struct State {
    connected: bool,
    mode: Option<Mode>,
    bat: Option<Battery>,
    eq: Option<Eq>,
    sp: Option<Spatial>,
    game: Option<bool>,
    gestures: Vec<(u8, u8, u8)>,
    hold: Option<u8>,
}

fn name<T: std::fmt::Debug>(v: Option<T>) -> String {
    match v {
        Some(v) => format!("\"{}\"", format!("{v:?}").to_lowercase()),
        None => "null".into(),
    }
}

fn cell(c: Option<Cell>) -> String {
    match c {
        Some(c) => format!("{{\"percent\":{},\"charging\":{}}}", c.percent, c.charging),
        None => "null".into(),
    }
}

fn gestures_json(g: &[(u8, u8, u8)]) -> String {
    if g.is_empty() {
        return "null".into();
    }
    let side = |s: Side| {
        let fields: Vec<String> = Gesture::ALL
            .iter()
            .map(|gest| {
                let a = g.iter().find(|(sd, gs, _)| *sd == s as u8 && *gs == *gest as u8).and_then(|e| Action::from_id(e.2));
                format!("\"{}\":{}", name_of(gest), name(a))
            })
            .collect();
        format!("\"{}\":{{{}}}", name_of(s), fields.join(","))
    };
    format!("{{{},{}}}", side(Side::Left), side(Side::Right))
}

impl State {
    fn json(&self) -> String {
        let bat = match self.bat {
            Some(b) => format!("{{\"left\":{},\"right\":{},\"case\":{}}}", cell(b.left), cell(b.right), cell(b.case)),
            None => "null".into(),
        };
        format!(
            "{{\"connected\":{},\"mode\":{},\"battery\":{},\"eq\":{},\"spatial\":{},\"game\":{},\"gestures\":{},\"hold\":{}}}",
            self.connected,
            name(self.mode),
            bat,
            name(self.eq),
            name(self.sp),
            self.game.map_or("null".to_string(), |g| g.to_string()),
            gestures_json(&self.gestures),
            self.hold.map_or("null".to_string(), |h| h.to_string())
        )
    }
}

fn publish(s: &State, last: &mut String) {
    let j = s.json();
    if j == *last {
        return;
    }
    *last = j.clone();
    println!("{j}");
    let _ = std::io::stdout().flush();
    if let Some(home) = std::env::var_os("HOME") {
        let dir = std::path::Path::new(&home).join("Library/Application Support/Buds");
        let _ = std::fs::create_dir_all(&dir);
        let tmp = dir.join("state.json.tmp");
        if std::fs::write(&tmp, &j).is_ok() {
            let _ = std::fs::rename(tmp, dir.join("state.json"));
        }
    }
}

fn parse_cmd(line: &str) -> Option<Cmd> {
    let mut it = line.split_whitespace();
    let (k, v) = (it.next()?, it.next().unwrap_or(""));
    match k {
        "mode" => by_name(&Mode::ALL, v).map(Cmd::Mode),
        "eq" => by_name(&Eq::ALL, v).map(Cmd::Eq),
        "spatial" => by_name(&Spatial::ALL, v).map(Cmd::Spatial),
        "game" => Some(Cmd::Game(v == "on")),
        "hold" => v.parse().ok().map(Cmd::Hold),
        "gesture" => {
            let g = by_name(&Gesture::ALL, it.next()?)?;
            let a = by_name(&Action::ALL, it.next()?)?;
            let side = by_name(&Side::ALL, v)?;
            g.actions().contains(&a).then_some(Cmd::Gesture(side, g, a))
        }
        "reconnect" => Some(Cmd::Reconnect),
        _ => None,
    }
}

fn refresh_extras(session: &mut Session, st: &mut State) {
    if let Ok(g) = session.query_game() {
        st.game = g;
    }
    if let Ok(g) = session.query_gestures() {
        st.gestures = g;
    }
    if let Ok(h) = session.query_hold() {
        st.hold = Some(h);
    }
}

fn main() {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Some(c) = parse_cmd(line.trim()) {
                let _ = tx.send(c);
            }
        }
        // Parent went away: stop.
        std::process::exit(0);
    });

    let mut last = String::new();
    let mut st = State::default();
    publish(&st, &mut last);
    loop {
        let mut session = match Session::open() {
            Ok(s) => s,
            Err(_) => {
                let _ = rx.recv_timeout(Duration::from_secs(5));
                continue;
            }
        };
        let Ok(mode) = session.query_mode() else {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        st.connected = true;
        st.mode = mode;
        publish(&st, &mut last);
        if let Ok(b) = session.query_battery() {
            st.bat = Some(b);
        }
        if let Ok(e) = session.query_eq() {
            st.eq = e;
        }
        if let Ok(m) = session.query_spatial() {
            st.sp = m;
        }
        refresh_extras(&mut session, &mut st);
        publish(&st, &mut last);

        let mut last_poll = Instant::now();
        'conn: loop {
            while let Ok(c) = rx.try_recv() {
                let ok = match c {
                    Cmd::Mode(m) => session.set_mode(m).map(|c| st.mode = c).is_ok(),
                    Cmd::Eq(e) => session.set_eq(e).map(|n| st.eq = n).is_ok(),
                    Cmd::Spatial(m) => session.set_spatial(m).and_then(|_| session.query_spatial()).map(|n| st.sp = n).is_ok(),
                    Cmd::Game(on) => session.set_game(on).map(|g| st.game = g).is_ok(),
                    Cmd::Gesture(sd, g, a) => session.set_gesture(sd, g, a).map(|v| st.gestures = v).is_ok(),
                    Cmd::Hold(m) => session.set_hold(m).map(|h| st.hold = Some(h)).is_ok(),
                    Cmd::Reconnect => false,
                };
                if !ok {
                    break 'conn;
                }
                publish(&st, &mut last);
            }
            match session.link.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(raw) => {
                    match decode(&raw) {
                        Some(Event::Noise(m, _)) => st.mode = m,
                        Some(Event::Battery(b)) => st.bat = Some(b),
                        Some(Event::Other(0x0504, p)) if !p.is_empty() => st.eq = Eq::from_id(p[0]),
                        Some(Event::Other(0x0510, p)) if !p.is_empty() => {
                            st.sp = Spatial::ALL.into_iter().find(|m| *m as u8 == p[0])
                        }
                        Some(Event::Other(0x8108, p)) => st.gestures = parse_gestures(&p),
                        Some(Event::Other(0x810D, p)) => {
                            st.game = parse_switches(&p).into_iter().find(|(i, _)| *i == SWITCH_GAME).map(|(_, v)| v != 0)
                        }
                        Some(Event::Other(0x810C, p)) if p.len() >= 4 && p[1] == 2 => st.hold = Some(p[3]),
                        _ => {}
                    }
                    publish(&st, &mut last);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break 'conn,
            }
            if last_poll.elapsed() >= Duration::from_secs(30) {
                last_poll = Instant::now();
                match session.query_mode() {
                    Ok(m) => st.mode = m,
                    Err(_) => break 'conn,
                }
                if let Ok(b) = session.query_battery() {
                    st.bat = Some(b);
                }
                refresh_extras(&mut session, &mut st);
                publish(&st, &mut last);
            }
        }
        st = State::default();
        publish(&st, &mut last);
        drop(session);
        std::thread::sleep(Duration::from_secs(1));
    }
}
