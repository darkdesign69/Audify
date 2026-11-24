//! Tauri commands of the smart layer (registered in lib.rs, typed in `ui/src/lib/api.ts`).

use std::collections::HashMap;
use std::sync::Arc;

use innertube::SongItem;
use encore_core::HostExt;
use tauri::State;

use super::home::Shelf;
use super::recs::Recommendation;
use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

/// Settings this layer owns. Kept out of `commands::UI_SETTINGS` so the shared list doesn't
/// churn; `private_session` is Stream B's and only read here.
const SMART_SETTINGS: [&str; 2] = [super::settings::SHUFFLE_MODE, super::settings::EXPLICIT_FILTER];

/// `{ shuffle_mode: "smart" | "standard", explicit_filter: bool, private_session: bool }`.
#[tauri::command]
pub fn smart_settings(state: St<'_>) -> serde_json::Value {
    let db = &state.db;
    serde_json::json!({
        "shuffle_mode": if super::settings::smart_shuffle(db) { "smart" } else { "standard" },
        "explicit_filter": super::settings::explicit_filter(db),
        "private_session": super::settings::private_session(db),
    })
}

#[tauri::command]
pub fn smart_set_setting(state: St<'_>, key: String, value: String) -> Result<(), String> {
    if !SMART_SETTINGS.contains(&key.as_str()) {
        return Err(format!("unknown smart setting: {key}"));
    }
    state.db.set_setting(&key, &value);
    Ok(())
}

/// The personal Home: shelves computed locally plus YouTube Music's (see `smart::home`).
#[tauri::command]
pub async fn smart_home(state: St<'_>) -> Result<Vec<Shelf>, String> {
    Ok(super::home::home(&state).await)
}

/// stream: magic-d4 — Home's local shelves only (no network): the quick revalidate after a play.
#[tauri::command]
pub fn smart_home_local(state: St<'_>) -> Vec<Shelf> {
    super::home::local_home(&state.db)
}

/// stream: home-tiles-pods — Home's Podcasts chip (see `smart::podhome`).
#[tauri::command]
pub async fn podcast_home(state: St<'_>) -> Result<Vec<Shelf>, String> {
    Ok(encore_core::smart::podhome::podcast_home(&state).await)
}

/// A podcast category tile's page: Apple's top shows in that genre.
#[tauri::command]
pub async fn podcast_category(state: St<'_>, genre: String) -> Result<Vec<super::home::ShelfItem>, String> {
    Ok(encore_core::smart::podhome::podcast_category(&state, genre).await)
}

/// The tracks of a Home mix item of kind `daily_mix` (its `id`). Play them with `play_playlist`.
#[tauri::command]
pub async fn smart_mix(
    state: St<'_>,
    id: String,
    size: Option<usize>,
) -> Result<Vec<SongItem>, String> {
    super::home::mix_tracks(&state, &id, size.unwrap_or(50).clamp(10, 100)).await
}

/// "Recommended" under a playlist (local `LOCALPLAYLIST:<n>` or a YouTube playlist id). `refresh`
/// counts presses of Refresh (0 first), each moving to the next seeds.
#[tauri::command]
pub async fn playlist_recommendations(
    state: St<'_>,
    playlist_id: String,
    n: Option<usize>,
    refresh: Option<usize>,
) -> Result<Vec<Recommendation>, String> {
    super::recs::playlist_recommendations(
        &state,
        &playlist_id,
        n.unwrap_or(10).clamp(1, 50),
        refresh.unwrap_or(0),
    )
    .await
}

/// Explicit flags for rows on screen, so the UI can dim them while the filter is on. Known flags
/// come back at once; with `lookup`, unknown ones (radio rows have no badge) are asked of Deezer
/// first (a few at a time, cached).
#[tauri::command]
pub async fn explicit_flags(
    state: St<'_>,
    items: Vec<SongItem>,
    lookup: Option<bool>,
) -> Result<HashMap<String, bool>, String> {
    let mut f = super::library::Filter::load(&state.db);
    if lookup.unwrap_or(false) {
        super::explicit::resolve_unknown(&state.db, &items, &mut f, 25).await;
    }
    Ok(items.iter().map(|s| (s.video_id.clone(), f.is_explicit(s))).collect())
}

/// Whether each row may play: `false` for hidden songs, blocked artists and (filter on) explicit.
#[tauri::command]
pub fn playable_flags(state: St<'_>, items: Vec<SongItem>) -> HashMap<String, bool> {
    let f = super::library::Filter::load(&state.db);
    items.iter().map(|s| (s.video_id.clone(), !f.rejects(s))).collect()
}

/// The taste profile's top artists and tracks, for a "Your taste" debug view / offline tuning.
#[tauri::command]
pub fn taste_profile(state: St<'_>, limit: Option<usize>) -> serde_json::Value {
    let t = super::taste::profile(&state.db);
    let n = limit.unwrap_or(20);
    let mut tracks: Vec<(&String, &f64)> = t.tracks.iter().collect();
    tracks.sort_by(|a, b| b.1.total_cmp(a.1));
    serde_json::json!({
        "artists": t.top_artists(n).into_iter().map(|(k, s)| serde_json::json!({
            "key": k, "name": t.artist_names.get(&k), "score": s,
        })).collect::<Vec<_>>(),
        "tracks": tracks.into_iter().take(n).map(|(k, s)| serde_json::json!({
            "videoId": k, "score": s,
        })).collect::<Vec<_>>(),
    })
}

// ── stream: smart2 ── Made for you, What's New, Smart Shuffle.

/// Made for you cards (Your Mix 1–6, Weekly Discoveries, New Release Mix, Your Rotation, Throwback Mix),
/// Spotify's order. Builds what is missing; `refresh` rebuilds everything now.
#[tauri::command]
pub async fn made_for_you(state: St<'_>, refresh: Option<bool>) -> Result<Vec<super::mfy::MfyCard>, String> {
    let st = state.inner().clone();
    if refresh.unwrap_or(false) {
        super::mfy::refresh(&st, true).await;
    }
    Ok(super::mfy::cards(&st).await)
}

/// The What's New feed: releases of followed artists and new episodes of followed podcasts.
#[tauri::command]
pub async fn whats_new(state: St<'_>, refresh: Option<bool>) -> Result<super::whatsnew::WhatsNew, String> {
    if refresh.unwrap_or(false) {
        super::whatsnew::refresh(&state, true).await;
    }
    Ok(super::whatsnew::whats_new(&state.db))
}

/// How many What's New items arrived since the page was last opened (the bell's dot).
#[tauri::command]
pub fn whats_new_count(state: St<'_>) -> usize {
    super::whatsnew::new_count(&state.db)
}

/// The What's New page was opened: everything on it is seen.
#[tauri::command]
pub fn whats_new_mark_seen(state: St<'_>) {
    super::whatsnew::mark_seen(&state.db);
    let _ = state.host.emit("whats-new-changed", serde_json::json!({ "newCount": 0 }));
}

/// Smart Shuffle state: `{ on, sourceId, recIds }`. Also re-syncs it with what is playing.
#[tauri::command]
pub async fn smart_shuffle_state(state: St<'_>) -> Result<super::smartshuffle::SmartShuffle, String> {
    Ok(super::smartshuffle::sync(state.inner()).await)
}

/// Turn Smart Shuffle on (shuffle + suggestions) or off (suggestions out, shuffle off).
#[tauri::command]
pub async fn smart_shuffle_set(state: St<'_>, on: bool) -> Result<super::smartshuffle::SmartShuffle, String> {
    super::smartshuffle::set(state.inner(), on).await
}

/// Remove one suggestion from the queue ("don't suggest this").
#[tauri::command]
pub async fn smart_shuffle_remove(state: St<'_>, video_id: String) -> Result<super::smartshuffle::SmartShuffle, String> {
    Ok(super::smartshuffle::remove(state.inner(), &video_id).await)
}

/// A suggestion was added to the playlist: drop its sparkle.
#[tauri::command]
pub fn smart_shuffle_keep(state: St<'_>, video_id: String) -> super::smartshuffle::SmartShuffle {
    super::smartshuffle::keep(&state, &video_id)
}
