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
