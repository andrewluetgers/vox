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
    project_overrides_in(&std::env::current_dir().ok()?)
}

pub fn project_overrides_in(start: &std::path::Path) -> Option<serde_json::Value> {
    let mut dir = start.to_path_buf();
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

fn shared_state_in(dir: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(dir.join("state.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Record an utterance in the shared history and last-spoken.txt. Honors the
/// shared save_history setting (last-spoken is always written so repeat-last
/// works with history off). TTL pruning is done by vox-tray and the hook.
pub fn log_history(source: &str, text: &str) {
    log_history_in(&shared_dir(), source, text)
}

pub fn log_history_in(dir: &std::path::Path, source: &str, text: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join("last-spoken.txt"), text);
    if shared_state_in(dir)["save_history"].as_bool() == Some(false) {
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
    recent_history_in(&shared_dir(), n)
}

pub fn recent_history_in(dir: &std::path::Path, n: usize) -> Vec<(String, String)> {
    let content = std::fs::read_to_string(dir.join("history.jsonl")).unwrap_or_default();
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
    last_spoken_in(&shared_dir())
}

pub fn last_spoken_in(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("last-spoken.txt"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vox-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn project_overrides_found_by_walking_up() {
        let root = tmp("proj");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(".vox.json"), r#"{"voice": "bf_emma", "speed": 1.3}"#).unwrap();
        let v = project_overrides_in(&nested).expect("should find .vox.json two levels up");
        assert_eq!(v["voice"], "bf_emma");
        assert_eq!(v["speed"], 1.3);
    }

    #[test]
    fn project_overrides_ignores_invalid_json() {
        let root = tmp("projbad");
        std::fs::write(root.join(".vox.json"), "not json").unwrap();
        assert!(project_overrides_in(&root).is_none());
    }

    #[test]
    fn history_round_trip_newest_first() {
        let dir = tmp("hist");
        log_history_in(&dir, "tui", "first thing");
        log_history_in(&dir, "claude", "second thing");
        let items = recent_history_in(&dir, 10);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], ("claude".to_string(), "second thing".to_string()));
        assert_eq!(items[1], ("tui".to_string(), "first thing".to_string()));
        assert_eq!(last_spoken_in(&dir).as_deref(), Some("second thing"));
        // n caps the result
        assert_eq!(recent_history_in(&dir, 1).len(), 1);
    }

    #[test]
    fn history_respects_save_history_off_but_keeps_last_spoken() {
        let dir = tmp("histoff");
        std::fs::write(dir.join("state.json"), r#"{"save_history": false}"#).unwrap();
        log_history_in(&dir, "tui", "quiet");
        assert!(recent_history_in(&dir, 10).is_empty());
        assert_eq!(last_spoken_in(&dir).as_deref(), Some("quiet"));
    }
}
