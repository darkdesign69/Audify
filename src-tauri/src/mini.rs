//! The desktop mini player window. Not available on Android (one window only).

use tauri::AppHandle;

pub const LABEL: &str = "mini";

pub fn open(_app: &AppHandle) -> Result<(), String> {
    Err("The mini player is not available on Android.".into())
}

pub fn dismiss(_app: &AppHandle) {}

pub fn set_compact(_app: &AppHandle, _compact: bool) -> Result<(), String> {
    Ok(())
}

pub fn save_position(_app: &AppHandle) {}

pub fn close(_app: &AppHandle) {}
