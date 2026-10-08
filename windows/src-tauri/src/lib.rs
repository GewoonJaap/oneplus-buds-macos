//! Thin Windows client for `buds-daemon`: tray icon, flyout window, OS integration.
//! All device logic lives in the daemon; this process only forwards command lines to its
//! stdin and relays its JSON state lines to the web UI.
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Default)]
struct Backend {
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Option<Child>>,
    state: Mutex<String>,
    hidden_at: Mutex<Option<Instant>>,
}

/// The daemon sits next to this exe when installed (sidecar), or in the cargo target dir in dev.
fn daemon_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("buds-daemon.exe")];
    for up in dir.ancestors().skip(1).take(5) {
        candidates.push(up.join("target").join("debug").join("buds-daemon.exe"));
        candidates.push(up.join("target").join("release").join("buds-daemon.exe"));
    }
    candidates.into_iter().find(|p| p.exists())
}

fn spawn_daemon(app: &AppHandle) -> Result<(), String> {
    let path = daemon_path().ok_or("buds-daemon.exe not found")?;
    let mut child = Command::new(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let backend = app.state::<Backend>();
    let mut stdin = child.stdin.take();
    // Protocol v2: the daemon records battery history and emits notification events for us.
    if let Some(i) = stdin.as_mut() {
        let _ = writeln!(i, "hello 2");
        let _ = i.flush();
    }
    *backend.stdin.lock().unwrap() = stdin;
    *backend.child.lock().unwrap() = Some(child);
    let app = app.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // Only state lines are replayable to a freshly loaded UI; events and stats are one-shot.
            if line.starts_with("{\"connected\"") {
                *app.state::<Backend>().state.lock().unwrap() = line.clone();
            }
            let _ = app.emit("line", line);
        }
    });
    Ok(())
}

#[tauri::command]
fn send_cmd(backend: tauri::State<Backend>, line: String) {
    if let Some(stdin) = backend.stdin.lock().unwrap().as_mut() {
        let _ = writeln!(stdin, "{line}");
        let _ = stdin.flush();
    }
}

#[tauri::command]
fn get_state(backend: tauri::State<Backend>) -> String {
    backend.state.lock().unwrap().clone()
}

#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) {
    let _ = app.notification().builder().title(title).body(body).show();
}

#[tauri::command]
fn autostart_get(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn autostart_set(app: AppHandle, enabled: bool) {
    let al = app.autolaunch();
    let _ = if enabled { al.enable() } else { al.disable() };
}

/// Returns the newer version string, or None when up to date.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<String>, String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

/// Download, verify (signature) and install the update, then relaunch.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else { return Ok(()) };
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    app.restart()
}

#[tauri::command]
fn app_info(app: AppHandle) -> (String, bool) {
    (app.package_info().version.to_string(), cfg!(debug_assertions))
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Show the flyout next to the tray icon, clamped to the monitor, on whichever edge the taskbar is.
fn show_flyout(app: &AppHandle, anchor: Option<(f64, f64, f64, f64)>) {
    let Some(win) = app.get_webview_window("main") else { return };
    if let Some(t) = *app.state::<Backend>().hidden_at.lock().unwrap() {
        if t.elapsed() < Duration::from_millis(250) {
            return; // the click that blurred (hid) the window also hit the tray icon
        }
    }
    if let (Some((ax, ay, aw, ah)), Ok(size)) = (anchor, win.outer_size()) {
        let (w, h) = (size.width as f64, size.height as f64);
        let (mx, my, mw, mh) = win
            .monitor_from_point(ax, ay)
            .ok()
            .flatten()
            .map(|m| (m.position().x as f64, m.position().y as f64, m.size().width as f64, m.size().height as f64))
            .unwrap_or((0.0, 0.0, 1920.0, 1080.0));
        let gap = 8.0;
        let mut x = ax + aw / 2.0 - w / 2.0;
        let mut y = if ay > my + mh / 2.0 { ay - h - gap } else { ay + ah + gap };
        x = x.clamp(mx + gap, mx + mw - w - gap);
        y = y.clamp(my + gap, my + mh - h - gap);
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
    let _ = win.show();
    let _ = win.set_focus();
}

pub fn run() {
    tauri::Builder::default()
        .manage(Backend::default())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show_flyout(app, None)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .invoke_handler(tauri::generate_handler![send_cmd, get_state, notify, autostart_get, autostart_set, quit, check_update, install_update, app_info])
        .setup(|app| {
            let handle = app.handle().clone();
            if let Err(e) = spawn_daemon(&handle) {
                eprintln!("daemon: {e}");
            }
            let quit_item = MenuItem::with_id(app, "quit", "Quit Buds", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit_item])?;
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Buds")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| {
                    if ev.id() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = ev {
                        let app = tray.app_handle();
                        let Some(win) = app.get_webview_window("main") else { return };
                        if win.is_visible().unwrap_or(false) {
                            let _ = win.hide();
                        } else {
                            let scale = win.scale_factor().unwrap_or(1.0);
                            let p = rect.position.to_physical::<f64>(scale);
                            let s = rect.size.to_physical::<f64>(scale);
                            show_flyout(app, Some((p.x, p.y, s.width, s.height)));
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) => {
                let _ = window.hide();
                *window.app_handle().state::<Backend>().hidden_at.lock().unwrap() = Some(Instant::now());
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Closing the daemon's stdin makes it exit; kill as a backstop.
                if let Some(mut c) = app.state::<Backend>().child.lock().unwrap().take() {
                    let _ = c.kill();
                }
            }
        });
}
