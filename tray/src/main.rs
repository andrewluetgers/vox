// vox-tray: macOS menu-bar companion for vox.
//
// - Tray menu: enable/disable Claude readouts, stop speaking, speed presets,
//   a panel window (speak box, history, settings), quit.
// - Configurable global shortcuts (default ⌃⌥⌘V speak-clipboard; press again
//   while speaking to stop).
// - Unix socket API at ~/.claude/vox/vox.sock (JSON lines):
//     {"cmd":"speak","text":"..."} | {"cmd":"clipboard"} | {"cmd":"stop"}
//     {"cmd":"status"} | {"cmd":"set","speed":1.2,...}
//
// Speaking shells out to the vox CLI. All state is ~/.claude/vox/state.json,
// shared with the Claude Stop hook and the /vox skill: settings changed here
// apply to the hook's readouts too. Defaults live in code and in the hook;
// state.json only stores overrides, so "reset" = remove the key.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tauri::image::Image;
use tauri::menu::{
    CheckMenuItem, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

// Must stay identical to DEFAULT_PROMPT in claude/vox-speak.sh — the hook
// falls back to it when state.json has no summary_prompt override.
const DEFAULT_PROMPT: &str = "You rewrite coding-assistant responses as spoken status updates: 1 to 3 short conversational sentences, plain prose only — no markdown, no code, no file paths or symbols unless they are the whole point. Lead with the outcome. Reply with only the sentences.";

const DEFAULT_SHORTCUTS: &[(&str, &str)] = &[
    ("speak_clipboard", "ctrl+alt+super+v"),
    ("stop", "ctrl+alt+super+s"),
    ("replay_last", "ctrl+alt+super+r"),
    ("toggle_readouts", ""),
];

const VOICES: &[&str] = &[
    "bm_george", "bm_lewis", "bm_daniel", "bm_fable", "bf_emma", "bf_isabella",
    "am_adam", "am_michael", "af_heart", "af_bella", "af_nicole", "af_sarah",
];

static CURRENT: Mutex<Option<Child>> = Mutex::new(None);
/// All selectable voice paths (kokoro bare, cloud "provider/voice"), refreshed
/// from `vox --list-voices` on menu rebuild; used to validate picks/previews.
static VOICE_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());
// The tray menu is rebuilt whenever history/state change, so the checkbox
// handle is replaced rather than set once.
static ENABLED_ITEM: Mutex<Option<CheckMenuItem<Wry>>> = Mutex::new(None);
/// Texts behind the Recent submenu items, index-matched to ids "hist-N".
static RECENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
static TRAY: OnceLock<TrayIcon<Wry>> = OnceLock::new();

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

fn history_path() -> PathBuf {
    vox_dir().join("history.jsonl")
}

fn vox_bin() -> PathBuf {
    let local = home().join(".local/bin/vox");
    if local.exists() {
        local
    } else {
        PathBuf::from("vox")
    }
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// --- settings -----------------------------------------------------------

fn default_shortcuts_json() -> Value {
    Value::Object(DEFAULT_SHORTCUTS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
}

fn read_state() -> Value {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}))
}

/// Stored state with every default filled in — what the UI shows and the
/// rest of the app reads.
fn effective_state() -> Value {
    let stored = read_state();
    let mut eff = json!({
        "enabled": true,
        "voice": "bm_george",
        "speed": 1.1,
        "verbatim_max": 300,
        "summary_prompt": DEFAULT_PROMPT,
        "save_audio": false,
        "audio_dir": "~/Music/vox",
        "audio_ttl_minutes": 20,
        "save_history": true,
        "history_ttl_minutes": 20,
        "shortcuts": default_shortcuts_json(),
    });
    if let (Some(e), Some(s)) = (eff.as_object_mut(), stored.as_object()) {
        for (k, v) in s {
            if k == "shortcuts" {
                if let (Some(defaults), Some(overrides)) = (e["shortcuts"].as_object_mut(), v.as_object()) {
                    for (action, accel) in overrides {
                        defaults.insert(action.clone(), accel.clone());
                    }
                }
            } else {
                e.insert(k.clone(), v.clone());
            }
        }
    }
    eff
}

fn write_state(v: &Value) {
    let _ = fs::create_dir_all(vox_dir());
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = fs::write(state_path(), s);
    }
}

fn update_state(f: impl FnOnce(&mut Value)) {
    let mut v = read_state();
    f(&mut v);
    write_state(&v);
}

/// Merge a settings patch into state.json. Values equal to their default are
/// removed instead of stored, so state.json stays a list of overrides and
/// "reset" works by sending the default back.
fn apply_patch(patch: &Value) {
    let Some(patch) = patch.as_object() else { return };
    let defaults = json!({
        "summary_prompt": DEFAULT_PROMPT,
        "shortcuts": default_shortcuts_json(),
    });
    update_state(|state| {
        if !state.is_object() {
            *state = json!({});
        }
        let obj = state.as_object_mut().unwrap();
        for (k, v) in patch {
            if k == "cmd" {
                continue;
            }
            if defaults[k.as_str()] == *v || (k == "summary_prompt" && v.as_str() == Some(DEFAULT_PROMPT)) {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    });
}

// --- speech -------------------------------------------------------------

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

fn spawn_vox(extra: &[&str], voice_override: Option<&str>) {
    // No stop_speaking() here: vox serializes playback on a shared lock, so
    // a new readout queues behind the current one instead of interrupting.
    let state = effective_state();
    let voice = voice_override
        .unwrap_or_else(|| state["voice"].as_str().unwrap_or("bm_george"))
        .to_string();
    let speed = state["speed"].as_f64().unwrap_or(1.1).to_string();
    let mut cmd = Command::new(vox_bin());
    if state["save_audio"].as_bool().unwrap_or(false) {
        if let Some(dir) = state["audio_dir"].as_str() {
            if !dir.is_empty() {
                cmd.env("VOX_AUDIO_DIR", expand_tilde(dir));
            }
        }
    } else {
        cmd.arg("--no-save");
    }
    cmd.args(["-v", &voice, "-s", &speed])
        .args(extra)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Ok(child) = cmd.spawn() {
        *CURRENT.lock().unwrap() = Some(child);
    }
}

/// One selectable voice: raw path (what gets stored), a human label
/// ("British male · George"), and — when the provider recently failed —
/// the short reason, which grays the voice out with an explanation.
#[derive(Clone)]
struct VoiceEntry {
    path: String,
    label: String,
    error: Option<String>,
}

/// Voices from `vox --list-voices --json`, grouped by provider in output
/// order, labeled for humans. Providers missing an API key are skipped;
/// providers with a recent runtime error (quota, terms, network) are kept
/// but marked so the UI can gray them out. Falls back to the static kokoro
/// list if the binary is missing. Also refreshes VOICE_PATHS (which only
/// contains selectable, non-erroring voices).
fn voice_groups() -> Vec<(String, Vec<VoiceEntry>)> {
    let entries: Vec<Value> = Command::new(vox_bin())
        .args(["--list-voices", "--json"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice(&o.stdout).ok())
        .unwrap_or_default();
    let mut groups: Vec<(String, String, Vec<VoiceEntry>)> = Vec::new();
    for e in &entries {
        if e["ready"].as_bool() != Some(true) {
            continue;
        }
        let (Some(path), Some(provider)) = (e["path"].as_str(), e["provider"].as_str()) else {
            continue;
        };
        let entry = VoiceEntry {
            path: path.to_string(),
            label: e["label"].as_str().unwrap_or(path).to_string(),
            error: e["error"].as_str().map(String::from),
        };
        let header = e["provider_label"].as_str().unwrap_or(provider).to_string();
        match groups.last_mut() {
            Some((g, _, v)) if *g == provider => v.push(entry),
            _ => groups.push((provider.to_string(), header, vec![entry])),
        }
    }
    if groups.is_empty() {
        groups.push((
            "kokoro".into(),
            "Kokoro-82M · local".into(),
            VOICES
                .iter()
                .map(|s| VoiceEntry { path: s.to_string(), label: s.to_string(), error: None })
                .collect(),
        ));
    }
    *VOICE_PATHS.lock().unwrap() = groups
        .iter()
        .flat_map(|(_, _, v)| v.iter().filter(|e| e.error.is_none()).map(|e| e.path.clone()))
        .collect();
    groups.into_iter().map(|(_, header, v)| (header, v)).collect()
}

/// Current selectable voice paths (populating them if the menu hasn't built yet).
fn voice_paths() -> Vec<String> {
    if VOICE_PATHS.lock().unwrap().is_empty() {
        voice_groups();
    }
    VOICE_PATHS.lock().unwrap().clone()
}

/// Markdown -> speakable text via the shared md2speech.sh filter (formatting
/// markers vanish, structure becomes pauses). Falls back to the raw text.
fn speakable(text: &str) -> String {
    let script = vox_dir().join("md2speech.sh");
    if script.exists() {
        let child = Command::new("/bin/bash")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if let Ok(out) = child.wait_with_output() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return s;
                }
            }
        }
    }
    text.to_string()
}

fn history_cutoff(state: &Value) -> u64 {
    let ttl_min = state["history_ttl_minutes"].as_f64().unwrap_or(20.0);
    if ttl_min > 0.0 {
        now_secs().saturating_sub((ttl_min * 60.0) as u64)
    } else {
        0 // 0 = keep (still capped at 500 entries)
    }
}

fn add_history(source: &str, text: &str) {
    let state = effective_state();
    if !state["save_history"].as_bool().unwrap_or(true) {
        return;
    }
    let cutoff = history_cutoff(&state);
    let entry = json!({"ts": now_secs(), "source": source, "text": text});
    let path = history_path();
    let _ = fs::create_dir_all(vox_dir());
    let mut lines: Vec<String> = fs::read_to_string(&path)
        .map(|s| {
            s.lines()
                .filter(|l| {
                    serde_json::from_str::<Value>(l)
                        .map(|v| v["ts"].as_u64().unwrap_or(0) >= cutoff)
                        .unwrap_or(false)
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    lines.push(entry.to_string());
    // Cap the history file so it can't grow without bound.
    let start = lines.len().saturating_sub(500);
    let _ = fs::write(&path, lines[start..].join("\n") + "\n");
}

/// Drop history entries older than the TTL (runs on the pruner timer, so
/// text ages out even when nothing new is spoken).
fn prune_history_once() {
    let state = effective_state();
    let cutoff = history_cutoff(&state);
    if cutoff == 0 {
        return;
    }
    let path = history_path();
    let Ok(content) = fs::read_to_string(&path) else { return };
    let kept: Vec<&str> = content
        .lines()
        .filter(|l| {
            serde_json::from_str::<Value>(l)
                .map(|v| v["ts"].as_u64().unwrap_or(0) >= cutoff)
                .unwrap_or(false)
        })
        .collect();
    if kept.len() < content.lines().count() {
        if kept.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            let _ = fs::write(&path, kept.join("\n") + "\n");
        }
    }
}

fn speak_text(text: &str, source: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    add_history(source, text);
    spawn_vox(&[&speakable(text)], None);
}

fn clipboard_text() -> Option<String> {
    Command::new("pbpaste")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn speak_clipboard() {
    if let Some(text) = clipboard_text() {
        speak_text(&text, "clipboard");
    }
}

fn is_speaking() -> bool {
    let mut guard = CURRENT.lock().unwrap();
    match guard.as_mut() {
        Some(child) => child.try_wait().map(|st| st.is_none()).unwrap_or(false),
        None => false,
    }
}

fn replay_last() {
    if let Ok(text) = fs::read_to_string(vox_dir().join("last-spoken.txt")) {
        if !text.trim().is_empty() {
            spawn_vox(&[text.trim()], None);
        }
    }
}

fn set_readouts_enabled(enabled: bool) {
    apply_patch(&json!({"enabled": enabled}));
    if let Some(item) = ENABLED_ITEM.lock().unwrap().as_ref() {
        let _ = item.set_checked(enabled);
    }
}

// --- shortcuts ----------------------------------------------------------

fn shortcut_map() -> Vec<(String, String)> {
    effective_state()["shortcuts"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn run_action(action: &str) {
    match action {
        "speak_clipboard" => {
            if is_speaking() {
                stop_speaking();
            } else {
                speak_clipboard();
            }
        }
        "stop" => stop_speaking(),
        "replay_last" => replay_last(),
        "toggle_readouts" => {
            let enabled = effective_state()["enabled"].as_bool().unwrap_or(true);
            set_readouts_enabled(!enabled);
        }
        _ => {}
    }
}

/// (Re-)register every configured shortcut. Returns per-action errors —
/// bad syntax or combos the OS refused — for the settings UI to display.
fn register_shortcuts(app: &AppHandle) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut errors = Vec::new();
    for (action, accel) in shortcut_map() {
        if accel.is_empty() {
            continue;
        }
        match accel.parse::<Shortcut>() {
            Ok(sc) => {
                if let Err(e) = gs.register(sc) {
                    errors.push(format!("{action}: {e}"));
                }
            }
            Err(e) => errors.push(format!("{action}: invalid shortcut \"{accel}\" ({e})")),
        }
    }
    errors
}

// --- socket API ---------------------------------------------------------

fn handle_cmd(app: &AppHandle, line: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return r#"{"ok":false,"error":"invalid json"}"#.into();
    };
    match v["cmd"].as_str() {
        Some("speak") => match v["text"].as_str() {
            Some(text) => {
                speak_text(text, v["source"].as_str().unwrap_or("socket"));
                r#"{"ok":true}"#.into()
            }
            None => r#"{"ok":false,"error":"missing text"}"#.into(),
        },
        Some("clipboard") => {
            speak_clipboard();
            r#"{"ok":true}"#.into()
        }
        Some("stop") => {
            stop_speaking();
            r#"{"ok":true}"#.into()
        }
        Some("status") => {
            let mut s = effective_state();
            s["speaking"] = json!(is_speaking());
            s.to_string()
        }
        Some("set") => {
            apply_patch(&v);
            let errors = register_shortcuts(app);
            if let Some(item) = ENABLED_ITEM.lock().unwrap().as_ref() {
                let _ = item.set_checked(effective_state()["enabled"].as_bool().unwrap_or(true));
            }
            json!({"ok": true, "shortcut_errors": errors}).to_string()
        }
        _ => r#"{"ok":false,"error":"unknown cmd"}"#.into(),
    }
}

fn handle_client(app: AppHandle, stream: UnixStream) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut out = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { break };
        let resp = handle_cmd(&app, &line);
        if writeln!(out, "{resp}").is_err() {
            break;
        }
    }
}

fn start_socket_server(app: AppHandle) {
    std::thread::spawn(move || {
        let path = sock_path();
        let _ = fs::create_dir_all(vox_dir());
        let _ = fs::remove_file(&path);
        let Ok(listener) = UnixListener::bind(&path) else {
            eprintln!("vox-tray: could not bind {}", path.display());
            return;
        };
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || handle_client(app, stream));
        }
    });
}

// --- audio TTL pruning ---------------------------------------------------

fn prune_audio_once() {
    let state = effective_state();
    if !state["save_audio"].as_bool().unwrap_or(false) {
        return;
    }
    let ttl_min = state["audio_ttl_minutes"].as_f64().unwrap_or(20.0);
    if ttl_min <= 0.0 {
        return; // 0 = keep forever
    }
    let dir = expand_tilde(state["audio_dir"].as_str().unwrap_or("~/Music/vox"));
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let cutoff = Duration::from_secs((ttl_min * 60.0) as u64);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("wav") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else { continue };
        if SystemTime::now().duration_since(modified).map(|age| age > cutoff).unwrap_or(false) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn start_pruner() {
    std::thread::spawn(|| loop {
        prune_audio_once();
        prune_history_once();
        std::thread::sleep(Duration::from_secs(120));
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

/// One-line menu label for a history entry.
fn menu_label(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut label: String = one_line.chars().take(48).collect();
    if one_line.chars().count() > 48 {
        label.push('…');
    }
    label
}

/// Build the full tray menu, including the Recent submenu (last 10 shared
/// history entries). Called at startup and whenever history/state change;
/// refreshes ENABLED_ITEM and RECENT for the event handler.
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let enabled = CheckMenuItem::with_id(
        app,
        "enabled",
        "Speak Claude replies",
        true,
        effective_state()["enabled"].as_bool().unwrap_or(true),
        None::<&str>,
    )?;
    *ENABLED_ITEM.lock().unwrap() = Some(enabled.clone());

    let stop_item = MenuItemBuilder::with_id("stop", "Stop speaking").build(app)?;
    let clip_item = MenuItemBuilder::with_id("clipboard", "Speak clipboard").build(app)?;
    let repeat_item = MenuItemBuilder::with_id("repeat", "Repeat last").build(app)?;

    // Recent submenu: parity with the TUI's Ctrl-P picker.
    let recent = fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["text"].as_str().map(String::from))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(10)
        .collect::<Vec<_>>();
    let mut recent_menu = SubmenuBuilder::new(app, "Recent");
    if recent.is_empty() {
        recent_menu = recent_menu.item(
            &MenuItemBuilder::with_id("hist-none", "Nothing spoken yet")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for (i, text) in recent.iter().enumerate() {
            recent_menu = recent_menu
                .item(&MenuItemBuilder::with_id(format!("hist-{i}"), menu_label(text)).build(app)?);
        }
    }
    let recent_menu = recent_menu.build()?;
    *RECENT.lock().unwrap() = recent;

    // Voice submenu: quick global voice pick, labeled with the model the
    // voices come from. "More…" deep-links to the panel's Settings tab —
    // the future home of other providers/models.
    let current_voice = effective_state()["voice"].as_str().unwrap_or("bm_george").to_string();
    let mut voice_menu = SubmenuBuilder::new(app, "Voice");
    for (header, voices) in voice_groups() {
        // a provider-level error shows once, on the section header
        let header_text = match voices.iter().find_map(|v| v.error.clone()) {
            Some(err) => format!("{header} — {err}"),
            None => header.clone(),
        };
        voice_menu = voice_menu.item(
            &MenuItemBuilder::with_id(format!("voice-header-{header}"), header_text)
                .enabled(false)
                .build(app)?,
        );
        for v in voices {
            voice_menu = voice_menu.item(&CheckMenuItem::with_id(
                app,
                format!("voice-{}", v.path),
                &v.label,
                v.error.is_none(), // gray out voices whose provider is failing
                v.path == current_voice,
                None::<&str>,
            )?);
        }
    }
    let voice_menu = voice_menu
        .separator()
        .item(&MenuItemBuilder::with_id("voice-more", "More voice settings…").build(app)?)
        .build()?;

    let mut speed_menu = SubmenuBuilder::new(app, "Speed");
    for s in ["0.8", "1.0", "1.1", "1.25", "1.5", "2.0"] {
        speed_menu = speed_menu
            .item(&MenuItemBuilder::with_id(format!("speed-{s}"), format!("{s}x")).build(app)?);
    }
    let speed_menu = speed_menu.build()?;
    let panel_item = MenuItemBuilder::with_id("panel", "Open vox…").build(app)?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    MenuBuilder::new(app)
        .item(&enabled)
        .item(&stop_item)
        .item(&repeat_item)
        .item(&recent_menu)
        .item(&clip_item)
        .item(&voice_menu)
        .item(&speed_menu)
        .separator()
        .item(&panel_item)
        .separator()
        .item(&quit)
        .build()
}

/// Rebuild the tray menu when history or state change on disk, so Recent and
/// the enabled checkbox track edits from the hook, skill, TUI, and panel.
fn start_menu_refresher(app: AppHandle) {
    std::thread::spawn(move || {
        let mtime = |p: PathBuf| fs::metadata(p).and_then(|m| m.modified()).ok();
        let perr = || vox_dir().join("provider-errors.json");
        let mut last = (mtime(history_path()), mtime(state_path()), mtime(perr()));
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let cur = (mtime(history_path()), mtime(state_path()), mtime(perr()));
            if cur != last {
                last = cur;
                let app2 = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let (Some(tray), Ok(menu)) = (TRAY.get(), build_menu(&app2)) {
                        let _ = tray.set_menu(Some(menu));
                    }
                });
            }
        }
    });
}

// --- launch at login ------------------------------------------------------

const LAUNCH_AGENT_LABEL: &str = "dev.andrewluetgers.vox-tray";

fn launch_agent_path() -> PathBuf {
    home().join(format!("Library/LaunchAgents/{LAUNCH_AGENT_LABEL}.plist"))
}

#[tauri::command]
fn get_launch_at_login() -> bool {
    launch_agent_path().exists()
}

/// Toggle launch-at-login via a plain LaunchAgent plist pointing at the
/// current executable — transparent, and uninstall is just removing the file.
#[tauri::command]
fn set_launch_at_login(enabled: bool) -> Value {
    let path = launch_agent_path();
    if !enabled {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("gui/{}/{LAUNCH_AGENT_LABEL}", uid())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_file(&path);
        return json!({"ok": true, "enabled": false});
    }
    let Ok(exe) = std::env::current_exe() else {
        return json!({"ok": false, "error": "can't locate the app executable"});
    };
    // minimal XML escaping for the path
    let exe = exe.display().to_string().replace('&', "&amp;").replace('<', "&lt;");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LAUNCH_AGENT_LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#
    );
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match fs::write(&path, plist) {
        Ok(_) => json!({"ok": true, "enabled": true, "path": path.display().to_string()}),
        Err(e) => json!({"ok": false, "error": e.to_string()}),
    }
}

fn uid() -> u32 {
    // uid without a libc dependency: launchctl needs "gui/<uid>/<label>"
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(501)
}

fn open_panel(app: &AppHandle) {
    open_panel_tab(app, "");
}

/// Open the panel, optionally jumping to a tab ("settings", "history").
/// The page reads location.hash on load and on hashchange.
fn open_panel_tab(app: &AppHandle, tab: &str) {
    if let Some(w) = app.get_webview_window("panel") {
        if !tab.is_empty() {
            let _ = w.eval(&format!("location.hash = '{tab}'"));
        }
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let url = if tab.is_empty() {
        "index.html".to_string()
    } else {
        format!("index.html#{tab}")
    };
    let _ = tauri::WebviewWindowBuilder::new(app, "panel", tauri::WebviewUrl::App(url.into()))
        .title("vox")
        .inner_size(560.0, 520.0)
        .build();
}

// --- panel commands ------------------------------------------------------

#[tauri::command]
fn speak(text: String) {
    speak_text(&text, "panel");
}

#[tauri::command]
fn stop() {
    stop_speaking();
}

/// Speak without logging to history — used by History-tab Replay so
/// replays don't duplicate entries.
#[tauri::command]
fn replay(text: String) {
    spawn_vox(&[&speakable(&text)], None);
}

#[tauri::command]
fn status() -> Value {
    let mut s = effective_state();
    s["speaking"] = json!(is_speaking());
    s
}

#[tauri::command]
fn get_settings() -> Value {
    let stored = read_state();
    json!({
        "settings": effective_state(),
        "defaults": {
            "summary_prompt": DEFAULT_PROMPT,
            "shortcuts": default_shortcuts_json(),
        },
        "prompt_overridden": !stored["summary_prompt"].is_null(),
        "launch_at_login": get_launch_at_login(),
        // [{path, label, group, error}] — the panel renders optgroups from
        // `group` and disables entries with an `error`
        "voices": voice_groups()
            .into_iter()
            .flat_map(|(header, voices)| {
                voices.into_iter().map(move |v| {
                    json!({"path": v.path, "label": v.label, "group": header.clone(),
                           "error": v.error})
                })
            })
            .collect::<Vec<_>>(),
    })
}

#[tauri::command]
fn save_settings(app: AppHandle, patch: Value) -> Value {
    apply_patch(&patch);
    let errors = register_shortcuts(&app);
    if let Some(item) = ENABLED_ITEM.lock().unwrap().as_ref() {
        let _ = item.set_checked(effective_state()["enabled"].as_bool().unwrap_or(true));
    }
    json!({"ok": true, "shortcut_errors": errors})
}

/// Speak a short sample so the user hears the voice they just picked.
#[tauri::command]
fn preview_voice(voice: String) {
    if !voice_paths().contains(&voice) {
        return;
    }
    // "openai/nova" -> "nova"; "bm_george" -> "george"
    let last = voice.rsplit('/').next().unwrap_or(&voice);
    let name = last.split('_').nth(1).unwrap_or(last);
    let sample = format!("Hello, this is the {name} voice. This is how I sound.");
    spawn_vox(&[&sample], Some(&voice));
}

/// Last 200 log entries (newest first) from the shared operational log —
/// readout lifecycle and errors from vox, the hook, and the TUI.
#[tauri::command]
fn get_log() -> Value {
    let lines = fs::read_to_string(vox_dir().join("log.jsonl")).unwrap_or_default();
    let mut entries: Vec<Value> = lines
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries.truncate(200);
    json!(entries)
}

#[tauri::command]
fn get_history() -> Value {
    let lines = fs::read_to_string(history_path()).unwrap_or_default();
    let mut entries: Vec<Value> = lines
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries.truncate(500);
    json!(entries)
}

#[tauri::command]
fn clear_history() {
    let _ = fs::remove_file(history_path());
}

// --- per-project overrides ------------------------------------------------
//
// The Claude hook registers every repo it speaks from in projects.json.
// A repo's overrides live in <repo>/.vox.json; disabling the override
// renames it to .vox.json.disabled so the hook stops finding it.

fn projects_path() -> PathBuf {
    vox_dir().join("projects.json")
}

fn override_files(root: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(root);
    (root.join(".vox.json"), root.join(".vox.json.disabled"))
}

#[tauri::command]
fn list_projects() -> Value {
    let reg: Value = fs::read_to_string(projects_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let mut list: Vec<Value> = Vec::new();
    if let Some(m) = reg.as_object() {
        for (path, meta) in m {
            let (on, off) = override_files(path);
            let disabled = off.exists() && !on.exists();
            let file = if disabled { &off } else { &on };
            let overrides: Value = fs::read_to_string(file)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| json!({}));
            list.push(json!({
                "path": path,
                "name": PathBuf::from(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone()),
                "last_seen": meta["last_seen"],
                "has_override": on.exists() || off.exists(),
                "override_disabled": disabled,
                "overrides": overrides,
            }));
        }
    }
    list.sort_by_key(|p| std::cmp::Reverse(p["last_seen"].as_u64().unwrap_or(0)));
    json!(list)
}

/// Merge a patch into a repo's .vox.json. Null values clear keys; an emptied
/// file is deleted (no override left behind).
#[tauri::command]
fn save_project_settings(path: String, patch: Value) -> Value {
    let (on, off) = override_files(&path);
    let target = if off.exists() && !on.exists() { off } else { on };
    let mut cur: Value = fs::read_to_string(&target)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let (Some(obj), Some(p)) = (cur.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if v.is_null() {
                obj.remove(k);
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    if cur.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        let _ = fs::remove_file(&target);
    } else if let Ok(s) = serde_json::to_string_pretty(&cur) {
        let _ = fs::write(&target, s + "\n");
    }
    json!({"ok": true})
}

#[tauri::command]
fn set_project_override_enabled(path: String, enabled: bool) {
    let (on, off) = override_files(&path);
    if enabled {
        if off.exists() && !on.exists() {
            let _ = fs::rename(&off, &on);
        }
    } else if on.exists() {
        let _ = fs::rename(&on, &off);
    }
}

#[tauri::command]
fn delete_project_override(path: String) {
    let (on, off) = override_files(&path);
    let _ = fs::remove_file(on);
    let _ = fs::remove_file(off);
}

/// Open the markdown-to-speech rules script in the default text editor.
/// It's a deterministic filter (awk), not a prompt — edits apply to the
/// next utterance, no restart needed.
#[tauri::command]
fn open_md_filter() {
    let script = vox_dir().join("md2speech.sh");
    if script.exists() {
        let _ = Command::new("open").arg("-t").arg(&script).status();
    }
}

#[tauri::command]
fn open_audio_dir() {
    let dir = expand_tilde(effective_state()["audio_dir"].as_str().unwrap_or("~/Music/vox"));
    let _ = fs::create_dir_all(&dir);
    let _ = Command::new("open").arg(&dir).status();
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            speak,
            stop,
            replay,
            status,
            get_settings,
            save_settings,
            get_launch_at_login,
            set_launch_at_login,
            preview_voice,
            get_history,
            get_log,
            clear_history,
            open_audio_dir,
            open_md_filter,
            list_projects,
            save_project_settings,
            set_project_override_enabled,
            delete_project_override
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            start_socket_server(app.handle().clone());
            start_pruner();

            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(|_app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        for (action, accel) in shortcut_map() {
                            let matches = accel
                                .parse::<Shortcut>()
                                .map(|s| &s == shortcut)
                                .unwrap_or(false);
                            if matches {
                                run_action(&action);
                            }
                        }
                    })
                    .build(),
            )?;
            let errors = register_shortcuts(app.handle());
            for e in errors {
                eprintln!("vox-tray: shortcut registration: {e}");
            }

            let menu = build_menu(app.handle())?;
            let tray = TrayIconBuilder::with_id("vox")
                .icon(tray_image())
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "enabled" => {
                        let checked = ENABLED_ITEM
                            .lock()
                            .unwrap()
                            .as_ref()
                            .and_then(|i| i.is_checked().ok())
                            .unwrap_or(true);
                        apply_patch(&json!({"enabled": checked}));
                    }
                    "stop" => stop_speaking(),
                    "clipboard" => speak_clipboard(),
                    "repeat" => replay_last(),
                    "panel" => open_panel(app),
                    "voice-more" => open_panel_tab(app, "settings"),
                    id if id.starts_with("voice-") => {
                        let v = &id["voice-".len()..];
                        if voice_paths().iter().any(|p| p == v) {
                            apply_patch(&json!({"voice": v}));
                            preview_voice(v.to_string());
                        }
                    }
                    id if id.starts_with("hist-") => {
                        if let Ok(i) = id["hist-".len()..].parse::<usize>() {
                            let text = RECENT.lock().unwrap().get(i).cloned();
                            if let Some(text) = text {
                                spawn_vox(&[&speakable(&text)], None);
                            }
                        }
                    }
                    id if id.starts_with("speed-") => {
                        if let Ok(speed) = id["speed-".len()..].parse::<f64>() {
                            apply_patch(&json!({"speed": speed}));
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            let _ = TRAY.set(tray);
            start_menu_refresher(app.handle().clone());

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
