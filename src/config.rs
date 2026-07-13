//! Persistent settings, stored at ~/.config/vox/config.toml.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub voice: String,
    pub speed: f32,
    pub audio_dir: String,
    pub save_audio: bool,
    pub cleanup_on_exit: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            voice: "bm_george".into(),
            speed: 1.0,
            audio_dir: "~/Music/vox".into(),
            save_audio: true,
            cleanup_on_exit: false,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_default()
        .join("vox")
        .join("config.toml")
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

/// Per-project overrides: the nearest .vox.json at or above the cwd.
/// Shared with the Claude Code integration (claude/ in this repo) and the
/// vox-tray app — a repo can pin its own voice/speed/audio settings.
pub fn project_overrides() -> Option<serde_json::Value> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".vox.json");
        if candidate.is_file() {
            return std::fs::read_to_string(candidate)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok());
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Shared readout state (~/.claude/vox) used by the Claude integration and
/// vox-tray. The TUI participates so every UI sees one history and
/// "repeat last" means the same thing everywhere.
pub fn shared_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".claude").join("vox")
}

fn shared_state() -> serde_json::Value {
    std::fs::read_to_string(shared_dir().join("state.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Record an utterance in the shared history and last-spoken.txt. Honors the
/// shared save_history setting (last-spoken is always written so repeat-last
/// works with history off). TTL pruning is done by vox-tray and the hook.
pub fn log_history(source: &str, text: &str) {
    let dir = shared_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("last-spoken.txt"), text);
    if shared_state()["save_history"].as_bool() == Some(false) {
        return;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = serde_json::json!({"ts": ts, "source": source, "text": text});
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("history.jsonl"))
    {
        let _ = writeln!(f, "{entry}");
    }
}

/// Last `n` shared-history entries, newest first: (source, text).
pub fn recent_history(n: usize) -> Vec<(String, String)> {
    let content =
        std::fs::read_to_string(shared_dir().join("history.jsonl")).unwrap_or_default();
    let mut items: Vec<(String, String)> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| {
            (
                v["source"].as_str().unwrap_or("?").to_string(),
                v["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .filter(|(_, t)| !t.is_empty())
        .collect();
    items.reverse();
    items.truncate(n);
    items
}

pub fn last_spoken() -> Option<String> {
    std::fs::read_to_string(shared_dir().join("last-spoken.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = config_path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, toml::to_string(self)?)?;
        Ok(())
    }

    pub fn audio_dir_path(&self) -> PathBuf {
        expand_tilde(&self.audio_dir)
    }
}
