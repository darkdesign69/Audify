//! Device sync commands: thin Tauri wrappers over `encore_core::sync` (the iOS bridge calls the
//! same functions from `crates/ffi/src/invoke.rs`). Events: `sync-changed`. See docs/SYNC.md.

use serde_json::Value;

use encore_core::sync;

#[tauri::command]
pub async fn sync_status() -> Result<Value, String> {
    sync::sync_status().await
}

#[tauri::command]
pub async fn sync_start_pairing() -> Result<Value, String> {
    sync::sync_start_pairing().await
}

#[tauri::command]
pub async fn sync_cancel_pairing() -> Result<Value, String> {
    sync::sync_cancel_pairing().await
}

#[tauri::command]
pub async fn sync_join(code: Option<String>, qr: Option<String>, host: Option<String>, port: Option<u16>) -> Result<Value, String> {
    sync::sync_join(code, qr, host, port).await
}

#[tauri::command]
pub async fn sync_unpair() -> Result<Value, String> {
    sync::sync_unpair().await
}

#[tauri::command]
pub async fn sync_now() -> Result<Value, String> {
    sync::sync_now().await
}

#[tauri::command]
pub async fn sync_set_active(active: bool) -> Result<Value, String> {
    sync::sync_set_active(active).await
}
