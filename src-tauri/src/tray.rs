//! The desktop tray icon. Android has none, so every call is a no-op and `available()` is false.

use tauri::{AppHandle, Emitter};

pub fn available() -> bool {
    false
}

pub fn set_available(_val: bool) {}

pub fn init(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}

pub fn show_main(_app: &AppHandle) {}

/// Tell the SPA whether anyone can see it (it stops animating while hidden).
pub fn set_main_visible(app: &AppHandle, visible: bool) {
    let _ = app.emit_to("main", "ui-visible", visible);
}

pub fn set_playing(_app: &AppHandle, _playing: bool) {}

pub fn set_track(_app: &AppHandle, _title: &str, _artists: &str) {}
