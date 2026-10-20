//! Persistent panel settings (%APPDATA%\browser-relay\config.json).
//! Loaded once at startup into a global; the settings UI edits it live
//! and saves on change.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// "" = auto-detect via find_chrome().
    pub chrome_path: String,
    /// "dark" | "light".
    pub theme: String,
    /// DevTools port for --remote-debugging-port and the CDP base URL.
    pub chrome_port: u16,
    /// Relay listen address ("" = auto-detect LAN IP).
    pub relay_addr: String,
    /// Relay listen port.
    pub relay_port: u16,
    /// When true, the X button hides the panel to the tray.
    /// When false (default), X really quits the app.
    pub hide_to_tray: bool,
    /// When true, the relay server auto-starts when the panel opens.
    /// When false (default), the user starts it manually — this avoids
    /// the Windows Firewall prompt on first open.
    pub auto_start_server: bool,
    /// When true, Chrome is auto-launched when the server starts.
    pub auto_launch_browser: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            chrome_path: String::new(),
            theme: "dark".to_string(),
            chrome_port: 9222,
            relay_addr: String::new(),
            relay_port: 8765,
            hide_to_tray: false,
            auto_start_server: false,
            auto_launch_browser: false,
        }
    }
}

static CONFIG: OnceLock<std::sync::RwLock<Config>> = OnceLock::new();

fn config_path() -> std::path::PathBuf {
    crate::chrome::config_dir().join("config.json")
}

/// Load from disk (once). Call early in main().
pub fn init() {
    let cfg = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    let _ = CONFIG.set(std::sync::RwLock::new(cfg));
}

pub fn get() -> Config {
    CONFIG
        .get()
        .map(|c| c.read().unwrap().clone())
        .unwrap_or_default()
}

pub fn update(f: impl FnOnce(&mut Config)) {
    if let Some(c) = CONFIG.get() {
        {
            let mut cfg = c.write().unwrap();
            f(&mut cfg);
        }
        let cfg = c.read().unwrap().clone();
        save(&cfg);
    }
}

fn save(cfg: &Config) {
    crate::chrome::ensure_config_dir();
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(config_path(), s);
    }
}
