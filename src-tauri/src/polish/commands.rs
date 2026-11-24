//! Tauri commands of the polish stream. Registered in lib.rs (`── stream: polish ──`); typed
//! wrappers in `ui/src/lib/api.ts` (same marker).

use std::collections::HashMap;
use std::sync::Arc;

use innertube::explore::{ChartsPage, NewReleasesPage, SectionsPage};
use innertube::SongItem;
use tauri::State;

use super::health::Health;
use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

/// Charts (`polish::explore::charts`): the country is remembered once picked.
#[tauri::command]
pub async fn get_charts(state: St<'_>, country: Option<String>) -> Result<ChartsPage, String> {
    super::explore::charts(&state, country).await
}

/// New releases (`polish::explore::new_releases`).
#[tauri::command]
pub async fn get_new_releases(state: St<'_>) -> Result<NewReleasesPage, String> {
    super::explore::new_releases(&state).await
}

/// A genre/mood page as titled shelves (`polish::explore::browse_sections`).
#[tauri::command]
pub async fn get_browse_sections(
    state: St<'_>,
    browse_id: String,
    params: Option<String>,
) -> Result<SectionsPage, String> {
    super::explore::browse_sections(&state, browse_id, params).await
}

/// Rows playback will step over, by videoId: `explicit` (filter on) or `unavailable`. Grey them.
#[tauri::command]
pub async fn playback_skip_reasons(
    state: St<'_>,
    items: Vec<SongItem>,
) -> Result<HashMap<String, &'static str>, String> {
    Ok(super::explicit::reasons(&state.db, &items))
}

/// Resolve a known video through the normal chain without playing it.
#[tauri::command]
pub async fn playback_health(state: St<'_>) -> Result<Health, String> {
    Ok(super::health::check(&state).await)
}

/// Recently played contexts with listen counts, newest first (what Home's shortcuts read).
#[tauri::command]
pub async fn recent_play_contexts(
    state: St<'_>,
    limit: Option<usize>,
) -> Result<Vec<super::context::RecentPlayContext>, String> {
    Ok(super::context::recent_play_contexts(&state.db, limit))
}

/// The YouTube video an RSS episode streams from, when one was matched (no network).
#[tauri::command]
pub async fn podcast_youtube_twin(state: St<'_>, id: String) -> Result<Option<String>, String> {
    Ok(super::podmatch::known_twin(&state.db, &id))
}

/// `true` (default) plays RSS episodes from their ad-free YouTube twin when one is found.
#[tauri::command]
pub async fn podcast_set_youtube_match(state: St<'_>, on: bool) -> Result<bool, String> {
    Ok(super::podmatch::set_enabled(&state.db, on))
}

#[tauri::command]
pub async fn podcast_youtube_match(state: St<'_>) -> Result<bool, String> {
    Ok(super::podmatch::enabled(&state.db))
}
