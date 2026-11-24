//! Tauri commands for podcasts. Registered in `lib.rs` (block `stream: podcasts`); typed in
//! `ui/src/lib/api.ts` (same marker). Playing an episode needs no command of its own: pass
//! `episodeSong(episode)` to the existing `play` / queue commands.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::State;

use super::refresh::{self, RefreshReport};
use super::{
    search, store, Episode, EpisodePage, Progress,
    SearchResults, Show, ShowPage, Sort,
};
use crate::state::AppState;

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn podcast_search(
    state: St<'_>,
    query: String,
    scope: Option<String>,
) -> Result<SearchResults, String> {
    super::api::podcast_search(&state, query, scope).await
}

#[tauri::command]
pub async fn podcast_show(state: St<'_>, id: String, sort: Option<Sort>) -> Result<ShowPage, String> {
    super::api::podcast_show(&state, id, sort).await
}

#[tauri::command]
pub async fn podcast_show_more(
    state: St<'_>,
    id: String,
    continuation: String,
    sort: Option<Sort>,
) -> Result<EpisodePage, String> {
    super::api::podcast_show_more(&state, id, continuation, sort).await
}

#[tauri::command]
pub async fn podcast_episode(state: St<'_>, id: String) -> Result<Episode, String> {
    super::api::podcast_episode(&state, id).await
}

/// stream: chapters.
#[tauri::command]
pub async fn podcast_chapters(
    state: St<'_>,
    id: String,
    duration_secs: Option<f64>,
) -> Result<super::chapters::Chapters, String> {
    super::api::podcast_chapters(&state, id, duration_secs).await
}

/// stream: video-pods. The picture for episode `id`, if it has one (`podcasts::video`).
#[tauri::command]
pub async fn podcast_episode_video(
    state: St<'_>,
    id: String,
) -> Result<Option<super::video::EpisodeVideo>, String> {
    Ok(super::video::podcast_episode_video(&state.db, &id))
}

#[tauri::command]
pub async fn podcast_follow(state: St<'_>, show: Show) -> Result<Show, String> {
    super::api::podcast_follow(&state, show).await
}

#[tauri::command]
pub async fn podcast_unfollow(state: St<'_>, id: String) -> Result<(), String> {
    store::unfollow(&state.db.podcasts_conn(), &id).map_err(|e| e.to_string())
}

/// Followed shows, most recently followed first.
#[tauri::command]
pub async fn podcast_follows(state: St<'_>) -> Result<Vec<Show>, String> {
    store::follows(&state.db.podcasts_conn()).map_err(|e| e.to_string())
}

/// Follow a pasted feed URL. Returns the show (fetched and parsed, so a bad URL errors here).
#[tauri::command]
pub async fn podcast_add_feed(state: St<'_>, url: String) -> Result<Show, String> {
    let url = url.trim().to_owned();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Enter a feed URL starting with http:// or https://".into());
    }
    let (mut show, eps, etag, lm) = super::rss::fetch_show(&url).await?;
    let conn = state.db.podcasts_conn();
    store::follow(&conn, &show).map_err(|e| e.to_string())?;
    store::upsert_episodes(&conn, &eps).map_err(|e| e.to_string())?;
    let _ = store::set_feed_state(
        &conn,
        &url,
        &store::FeedState { etag, last_modified: lm, last_refresh: Some(super::now_secs()) },
    );
    show.followed = true;
    show.episode_count = Some(eps.len() as u32);
    Ok(show)
}

/// Unplayed episodes of followed shows from the last `days` (default 30), newest first.
#[tauri::command]
pub async fn podcast_new_episodes(
    state: St<'_>,
    limit: Option<usize>,
    days: Option<i64>,
) -> Result<Vec<Episode>, String> {
    let since = super::now_secs() - days.unwrap_or(30) * 86_400;
    store::new_episodes(&state.db.podcasts_conn(), since, limit.unwrap_or(100)).map_err(|e| e.to_string())
}

/// Started but unfinished episodes, most recent first.
#[tauri::command]
pub async fn podcast_continue_listening(state: St<'_>, limit: Option<usize>) -> Result<Vec<Episode>, String> {
    store::continue_listening(&state.db.podcasts_conn(), limit.unwrap_or(20)).map_err(|e| e.to_string())
}

/// "Your Episodes".
#[tauri::command]
pub async fn podcast_saved_episodes(state: St<'_>) -> Result<Vec<Episode>, String> {
    store::saved_episodes(&state.db.podcasts_conn()).map_err(|e| e.to_string())
}

/// Add to / remove from "Your Episodes". Takes the whole episode so it can be listed later even
/// if it came from a search.
#[tauri::command]
pub async fn podcast_save_episode(state: St<'_>, episode: Episode, saved: bool) -> Result<(), String> {
    store::set_saved(&state.db.podcasts_conn(), &episode, saved).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn podcast_mark_played(state: St<'_>, id: String, played: bool) -> Result<Progress, String> {
    let conn = state.db.podcasts_conn();
    store::set_played(&conn, &id, played).map_err(|e| e.to_string())?;
    Ok(store::progress(&conn, &id).unwrap_or_default())
}

/// Mark every known episode of a show played. Returns how many.
#[tauri::command]
pub async fn podcast_mark_show_played(state: St<'_>, show_id: String) -> Result<usize, String> {
    store::mark_show_played(&state.db.podcasts_conn(), &show_id).map_err(|e| e.to_string())
}

/// Progress for a list of episode ids (for rows rendered from elsewhere, e.g. the queue).
#[tauri::command]
pub async fn podcast_progress(state: St<'_>, ids: Vec<String>) -> Result<HashMap<String, Progress>, String> {
    let conn = state.db.podcasts_conn();
    Ok(ids.into_iter().filter_map(|id| store::progress(&conn, &id).map(|p| (id, p))).collect())
}

/// Refresh all followed shows now ("pull to refresh").
#[tauri::command]
pub async fn podcast_refresh(state: St<'_>) -> Result<RefreshReport, String> {
    Ok(refresh::refresh_all(state.inner()).await)
}

#[tauri::command]
pub async fn podcast_opml_export(state: St<'_>) -> Result<String, String> {
    super::api::podcast_opml_export(&state.db.podcasts_conn())
}

pub use super::api::{ImportReport, PodcastSettings};

/// Import follows from OPML text. Titles and artwork fill in as the background sync reaches each.
#[tauri::command]
pub async fn podcast_opml_import(state: St<'_>, xml: String) -> Result<ImportReport, String> {
    super::api::podcast_opml_import(state.inner(), &xml)
}

#[tauri::command]
pub async fn podcast_settings(state: St<'_>) -> Result<PodcastSettings, String> {
    Ok(super::api::podcast_settings(&state.db))
}

/// Partial update: only fields present in `settings` change.
#[tauri::command]
pub async fn podcast_set_settings(state: St<'_>, settings: serde_json::Value) -> Result<PodcastSettings, String> {
    Ok(super::api::podcast_set_settings(&state.db, &settings))
}

// ── stream: podtop ──
/// The parts of a search card [`search::top_show`] reads (`BrowseItem` has no `Deserialize`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TopCard {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
}

impl TopCard {
    fn item(&self) -> innertube::BrowseItem {
        let kind: &'static str = match self.kind.as_str() {
            "song" => "song",
            "album" => "album",
            "artist" => "artist",
            "playlist" => "playlist",
            "podcast" => "podcast",
            "episode" => "episode",
            _ => "other",
        };
        innertube::BrowseItem {
            kind,
            id: self.id.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: Vec::new(),
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
        }
    }
}

/// Which of `shows` (from `podcast_search`) should be the search page's Top result instead of
/// YouTube Music's card (`top` = `searchAll`'s `top[0]`, `artist` = its `artists[0]`); `null` =
/// keep YouTube's. Pure, no I/O (docs/streams/podtop.md).
#[tauri::command]
pub fn podcast_top_show(
    query: String,
    top: Option<TopCard>,
    artist: Option<TopCard>,
    shows: Vec<Show>,
) -> Option<usize> {
    let (top, artist) = (top.map(|c| c.item()), artist.map(|c| c.item()));
    search::top_show(&query, top.as_ref(), artist.as_ref(), &shows)
}
// ── end stream: podtop ──
