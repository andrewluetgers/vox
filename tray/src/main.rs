// vox-tray: macOS menu-bar companion for vox.
//
// - Tray menu: enable/disable Claude readouts, stop speaking, speed presets,
//   a small panel window, quit.
// - Global shortcut ⌃⌥⌘V: speak the clipboard; press again while speaking
//   to stop.
// - Unix socket API at ~/.claude/vox/vox.sock (JSON lines):
//     {"cmd":"speak","text":"...","voice":"bm_george","speed":1.2}
//     {"cmd":"stop"} | {"cmd":"status"} | {"cmd":"set","enabled":false,...}
//
// Speaking shells out to the vox CLI; state is the same
// ~/.claude/vox/state.json the Claude Stop hook reads, so the tray toggle
// mutes the hook too.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

static CURRENT: Mutex<Option<Child>> = Mutex::new(None);

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set"))
}

fn vox_dir() -> PathBuf {
    home().join(".claude/vox")
}

fn state_path() -> PathBuf {
    vox_dir().join("state.json")
}

fn sock_path() -> PathBuf {
    vox_dir().join("vox.sock")
}

fn vox_bin() -> PathBuf {
    let local = home().join(".local/bin/vox");
    if local.exists() {
        local
    } else {
        PathBuf::from("vox")
    }
}

fn default_state() -> serde_json::Value {
    serde_json::json!({"enabled": true, "voice": "bm_george", "speed": 1.1, "verbatim_max": 300})
}

fn read_state() -> serde_json::Value {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(default_state)
}

fn update_state(f: impl FnOnce(&mut serde_json::Value)) {
    let mut v = read_state();
    f(&mut v);
    let _ = fs::create_dir_all(vox_dir());
    if let Ok(s) = serde_json::to_string_pretty(&v) {
        let _ = fs::write(state_path(), s);
    }
}

fn stop_speaking() {
    // Kills any vox process, including ones spawned by the Claude hook.
    let _ = Command::new(vox_bin())
        .arg("--stop")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Some(mut child) = CURRENT.lock().unwrap().take() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn spawn_vox(extra: &[&str]) {
    stop_speaking();
    let state = read_state();
    let voice = state["voice"].as_str().unwrap_or("bm_george").to_string();
    let speed = state["speed"].as_f64().unwrap_or(1.1).to_string();
    let mut cmd = Command::new(vox_bin());
    cmd.args(["--no-save", "-v", &voice, "-s", &speed])
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = cmd.spawn() {
        *CURRENT.lock().unwrap() = Some(child);
    }
}

fn speak_text(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    spawn_vox(&[text]);
}

fn is_speaking() -> bool {
    let mut guard = CURRENT.lock().unwrap();
    match guard.as_mut() {
        Some(child) => child.try_wait().map(|st| st.is_none()).unwrap_or(false),
        None => false,
    }
}

/// ⌃⌥⌘V: speak the clipboard, or stop if we're already speaking.
fn toggle_clipboard_speech() {
    if is_speaking() {
        stop_speaking();
    } else {
        spawn_vox(&["-c"]);
    }
}

// --- socket API ---------------------------------------------------------

fn handle_cmd(line: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return r#"{"ok":false,"error":"invalid json"}"#.into();
    };
    match v["cmd"].as_str() {
        Some("speak") => match v["text"].as_str() {
            Some(text) => {
                speak_text(text);
                r#"{"ok":true}"#.into()
            }
            None => r#"{"ok":false,"error":"missing text"}"#.into(),
        },
        Some("clipboard") => {
            spawn_vox(&["-c"]);
            r#"{"ok":true}"#.into()
        }
        Some("stop") => {
            stop_speaking();
            r#"{"ok":true}"#.into()
        }
        Some("status") => {
            let mut s = read_state();
            s["speaking"] = serde_json::json!(is_speaking());
            s.to_string()
        }
        Some("set") => {
            update_state(|state| {
                for key in ["enabled", "voice", "speed", "verbatim_max"] {
                    if !v[key].is_null() {
                        state[key] = v[key].clone();
                    }
                }
            });
            r#"{"ok":true}"#.into()
        }
        _ => r#"{"ok":false,"error":"unknown cmd"}"#.into(),
    }
}

fn handle_client(stream: UnixStream) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut out = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { break };
        let resp = handle_cmd(&line);
        if writeln!(out, "{resp}").is_err() {
            break;
        }
    }
}

fn start_socket_server() {
    std::thread::spawn(|| {
        let path = sock_path();
        let _ = fs::create_dir_all(vox_dir());
        let _ = fs::remove_file(&path);
        let Ok(listener) = UnixListener::bind(&path) else {
            eprintln!("vox-tray: could not bind {}", path.display());
            return;
        };
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle_client(stream));
        }
    });
}

// --- tray ---------------------------------------------------------------

/// 22x22 template image (black + alpha): a speaker with sound waves.
fn tray_image() -> Image<'static> {
    const W: usize = 22;
    const H: usize = 22;
    let mut rgba = vec![0u8; W * H * 4];
    let mut set = |x: i32, y: i32| {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            rgba[(y as usize * W + x as usize) * 4 + 3] = 255;
        }
    };
    for y in 8..=13 {
        for x in 3..=7 {
            set(x, y);
        }
    }
    for x in 8..=12 {
        let spread = x - 8;
        for y in (8 - spread)..=(13 + spread) {
            set(x, y);
        }
    }
    for (x, y) in [(15, 8), (16, 10), (16, 12), (15, 14), (17, 6), (18, 9), (18, 13), (17, 16)] {
        set(x, y);
    }
    Image::new_owned(rgba, W as u32, H as u32)
}

fn open_panel(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("panel") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(app, "panel", tauri::WebviewUrl::App("index.html".into()))
        .title("vox")
        .inner_size(400.0, 320.0)
        .build();
}

// --- panel commands -----------------------------------------------------

#[tauri::command]
fn speak(text: String) {
    speak_text(&text);
}

#[tauri::command]
fn stop() {
    stop_speaking();
}

#[tauri::command]
fn status() -> serde_json::Value {
    let mut s = read_state();
    s["speaking"] = serde_json::json!(is_speaking());
    s
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![speak, stop, status])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            start_socket_server();

            let speak_clip = Shortcut::new(
                Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER),
                Code::KeyV,
            );
            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        if shortcut == &speak_clip && event.state() == ShortcutState::Pressed {
                            toggle_clipboard_speech();
                        }
                    })
                    .build(),
            )?;
            app.global_shortcut().register(speak_clip)?;

            let enabled = CheckMenuItem::with_id(
                app,
                "enabled",
                "Speak Claude replies",
                true,
                read_state()["enabled"].as_bool().unwrap_or(true),
                None::<&str>,
            )?;
            let stop_item = MenuItemBuilder::with_id("stop", "Stop speaking").build(app)?;
            let clip_item = MenuItemBuilder::with_id("clipboard", "Speak clipboard")
                .accelerator("ctrl+alt+cmd+v")
                .build(app)?;
            let mut speed_menu = SubmenuBuilder::new(app, "Speed");
            for s in ["0.8", "1.0", "1.1", "1.25", "1.5", "2.0"] {
                speed_menu = speed_menu.item(&MenuItemBuilder::with_id(format!("speed-{s}"), format!("{s}x")).build(app)?);
            }
            let speed_menu = speed_menu.build()?;
            let panel_item = MenuItemBuilder::with_id("panel", "Open panel…").build(app)?;
            let quit = PredefinedMenuItem::quit(app, None)?;
            let menu = MenuBuilder::new(app)
                .item(&enabled)
                .item(&stop_item)
                .item(&clip_item)
                .item(&speed_menu)
                .separator()
                .item(&panel_item)
                .separator()
                .item(&quit)
                .build()?;

            let enabled_handle = enabled.clone();
            TrayIconBuilder::with_id("vox")
                .icon(tray_image())
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "enabled" => {
                        let checked = enabled_handle.is_checked().unwrap_or(true);
                        update_state(|s| s["enabled"] = serde_json::json!(checked));
                    }
                    "stop" => stop_speaking(),
                    "clipboard" => spawn_vox(&["-c"]),
                    "panel" => open_panel(app),
                    id if id.starts_with("speed-") => {
                        if let Ok(speed) = id["speed-".len()..].parse::<f64>() {
                            update_state(|s| s["speed"] = serde_json::json!(speed));
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build vox-tray")
        .run(|_app, event| {
            // Keep running as a tray app when the panel window closes.
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
