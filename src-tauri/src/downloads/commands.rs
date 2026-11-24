//! Tauri commands for downloads. Events: `downloads:changed` (summary + `added`/`removed` done
//! ids), `downloads:progress` (`[{videoId, done, total}]`, 3×/s while running),
//! `downloads:network` (`{online, metered, offline, offlineMode, allowMetered, paused}`).

use std::path::PathBuf;
use std::sync::Arc;

use innertube::SongItem;
use serde::Deserialize;
use tauri::State;

use super::store::Collection;
use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

fn mgr() -> Result<&'static Arc<super::Manager>, String> {
    super::mgr().ok_or_else(|| "downloads are not ready".to_owned())
}

/// Everything the UI store needs: `{done: id[], collections, count, bytes, queued, failed,
/// active, network, dir, art: {thumbnailUrl: path}, quality}`.
#[tauri::command]
pub async fn downloads_snapshot() -> Result<serde_json::Value, String> {
    Ok(mgr()?.snapshot())
}

/// Download a collection (`kind`: playlist | album | liked | episode | song). Playlists, albums
/// and Liked Songs are read (and later re-synced) by the backend; pass `songs` for an episode.
#[tauri::command]
pub async fn downloads_add(
    _state: St<'_>,
    collection: Collection,
    songs: Option<Vec<SongItem>>,
) -> Result<(), String> {
    mgr()?.add(collection, songs).await
}

/// Remove one collection's download (tracks other downloads still hold stay).
#[tauri::command]
pub async fn downloads_remove(id: String) -> Result<(), String> {
    mgr()?.remove(&id);
    Ok(())
}

/// Settings › Storage › Remove all downloads.
#[tauri::command]
pub async fn downloads_remove_all() -> Result<(), String> {
    mgr()?.remove_all();
    Ok(())
}

/// Try failed tracks again now.
#[tauri::command]
pub async fn downloads_retry() -> Result<(), String> {
    mgr()?.retry();
    Ok(())
}

/// Re-read every downloaded playlist/album now (new songs downloaded, removed ones deleted).
#[tauri::command]
pub async fn downloads_sync() -> Result<(), String> {
    mgr()?.sync_all(true).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub offline_mode: Option<bool>,
    pub allow_metered: Option<bool>,
    /// `HIGH` | `LOW`
    pub quality: Option<String>,
}

#[tauri::command]
pub async fn downloads_set_settings(
    state: St<'_>,
    patch: SettingsPatch,
) -> Result<serde_json::Value, String> {
    let m = mgr()?;
    if let Some(on) = patch.offline_mode {
        m.set_offline_mode(on);
    }
    if let Some(on) = patch.allow_metered {
        m.set_allow_metered(on);
    }
    if let Some(q) = patch.quality {
        let q = if q.eq_ignore_ascii_case("low") { "LOW" } else { "HIGH" };
        state.db.set_setting(super::SET_QUALITY, q);
    }
    Ok(m.snapshot())
}

/// Move the downloads folder (Settings › Storage › Change location).
#[tauri::command]
pub async fn downloads_set_dir(path: String) -> Result<serde_json::Value, String> {
    let m = mgr()?;
    let to = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking({
        let m = m.clone();
        move || m.move_to(to)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(m.snapshot())
}

/// Every downloaded song, newest first.
#[tauri::command]
pub async fn downloads_songs(state: St<'_>) -> Result<Vec<SongItem>, String> {
    Ok(super::store::done_songs(&state.db.conn()))
}
