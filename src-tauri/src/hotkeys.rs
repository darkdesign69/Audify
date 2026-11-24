//! Global hotkeys exist on the desktop only. This keeps the settings types (the UI still reads
//! and writes them) and the volume helpers; nothing is ever registered with the OS on Android.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::db::Db;

pub const SETTINGS_KEY: &str = "global_hotkeys";

/// The level unmute returns to. `set_volume` writes it, so muting and unmuting land on one level.
pub(crate) static LAST_NONZERO_VOLUME: AtomicI64 = AtomicI64::new(100);

/// Mute is volume 0, so the level to come back to is persisted as `volume_before_mute`.
pub(crate) fn remember_unmute_level(db: &crate::db::Db) {
    let level = LAST_NONZERO_VOLUME.load(Ordering::Relaxed);
    db.set_setting("volume_before_mute", &level.to_string());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    PlayPause,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
    MuteToggle,
    SeekForward,
    SeekBackward,
    ToggleShuffle,
    ToggleRepeat,
    ShowApp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeysConfig {
    pub enabled: bool,
    pub bindings: HashMap<HotkeyAction, String>,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self { enabled: false, bindings: HashMap::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyRegisterResult {
    pub success: bool,
    pub config: HotkeysConfig,
    pub errors: HashMap<HotkeyAction, String>,
}

pub struct HotkeysManager {
    pub config: Mutex<HotkeysConfig>,
}

impl HotkeysManager {
    pub fn new(config: HotkeysConfig) -> Self {
        Self { config: Mutex::new(config) }
    }

    /// Stores the config and reports success; there is nothing to register on Android.
    pub fn apply_config(&self, _app: &AppHandle, new_config: HotkeysConfig) -> HotkeyRegisterResult {
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = new_config.clone();
        HotkeyRegisterResult { success: true, config: new_config, errors: HashMap::new() }
    }

    pub fn get_config(&self) -> HotkeysConfig {
        self.config.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

pub fn load_config(db: &Db) -> HotkeysConfig {
    if let Some(json) = db.get_setting(SETTINGS_KEY) {
        if let Ok(cfg) = serde_json::from_str::<HotkeysConfig>(&json) {
            return cfg;
        }
    }
    HotkeysConfig::default()
}

pub fn save_config(db: &Db, config: &HotkeysConfig) {
    if let Ok(json) = serde_json::to_string(config) {
        db.set_setting(SETTINGS_KEY, &json);
    }
}
