//! Background refresh of followed shows (AntennaPod's feed updater, simplified): on launch (after
//! the UI has had a moment) and every [`INTERVAL`], at most [`PARALLEL`] shows at a time.
//! RSS shows use a conditional GET; YouTube shows re-read the first page of their show page.
//! New episodes are announced with a `podcasts:new` event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use crate::host::HostExt;

use super::{now_secs, rss, store, youtube, Episode, Show, Sort};
use crate::state::AppState;

pub const INTERVAL: Duration = Duration::from_secs(3 * 3600);
const FIRST_DELAY: Duration = Duration::from_secs(20);
const PARALLEL: usize = 4;
/// Only episodes this recent are announced (a show that dumps its back catalogue isn't "new").
const NEW_WINDOW: i64 = 14 * 86_400;

static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport {
    pub checked: usize,
    pub failed: usize,
    /// Recent episodes found since the previous refresh, newest first.
    pub new_episodes: Vec<Episode>,
}

pub fn spawn() {
    crate::rt::spawn(async move {
        tokio::time::sleep(FIRST_DELAY).await;
        loop {
            if let Some(st) = crate::state::global() {
                let report = refresh_all(&st).await;
                tracing::info!(
                    checked = report.checked,
                    failed = report.failed,
                    new = report.new_episodes.len(),
                    "podcasts: refreshed follows"
                );
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Refresh every followed show. A second call while one runs returns an empty report.
pub async fn refresh_all(st: &Arc<AppState>) -> RefreshReport {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return RefreshReport::default();
    }
    let follows = store::follows(&st.db.podcasts_conn()).unwrap_or_default();
    // ── stream: reliability ── every followed show at once is bulk work (YouTube shows go
    // through InnerTube): paced, and user actions go first. RSS feeds are unaffected.
    let results: Vec<Result<Vec<Episode>, String>> =
        innertube::background(
            futures_util::stream::iter(follows)
                .map(|show| {
                    let st = st.clone();
                    async move { refresh_show(&st, &show).await }
                })
                .buffer_unordered(PARALLEL)
                .collect(),
        )
        .await;
    RUNNING.store(false, Ordering::SeqCst);
    let mut report = RefreshReport { checked: results.len(), ..Default::default() };
    for r in results {
        match r {
            Ok(eps) => report.new_episodes.extend(eps),
            Err(e) => {
                tracing::debug!("podcasts: refresh failed: {e}");
                report.failed += 1;
            }
        }
    }
    report.new_episodes.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    if !report.new_episodes.is_empty() {
        let _ = st.host.emit("podcasts:new", &report);
    }
    report
}

/// Refresh one show; returns its newly found recent episodes (empty on the first sync, which only
/// seeds the table).
pub async fn refresh_show(st: &AppState, show: &Show) -> Result<Vec<Episode>, String> {
    let state = store::feed_state(&st.db.podcasts_conn(), &show.id);
    let first_sync = state.last_refresh.is_none();
    let (meta, episodes, etag, last_modified) = if show.is_youtube() {
        let client = st.clients.get(innertube::METADATA_CLIENT).ok_or("metadata client missing")?;
        let p = youtube::fetch_show(&st.it, client, &show.id, Sort::Newest).await?;
        (Some(p.show), p.episodes, None, None)
    } else {
        let url = show.feed_url.as_deref().unwrap_or(&show.id);
        match rss::fetch(url, state.etag.as_deref(), state.last_modified.as_deref()).await? {
            rss::Fetched::NotModified => {
                let mut s = state.clone();
                s.last_refresh = Some(now_secs());
                let _ = store::set_feed_state(&st.db.podcasts_conn(), &show.id, &s);
                return Ok(Vec::new());
            }
            rss::Fetched::Body { bytes, etag, last_modified } => {
                let (mut s, eps) = rss::parse_feed(url, &bytes)?;
                s.id = show.id.clone();
                let eps = eps
                    .into_iter()
                    .map(|mut e| {
                        e.show_id = Some(show.id.clone());
                        e
                    })
                    .collect();
                (Some(s), eps, etag, last_modified)
            }
        }
    };
    let conn = st.db.podcasts_conn();
    if let Some(m) = &meta {
        let _ = store::update_follow_meta(&conn, &show.id, m);
        store::remember_show_art(&conn, &show.id, m.thumbnail.as_deref()); // stream: tilefix
        if !show.is_youtube() {
            store::set_serial(&conn, &show.id, m.serial); // stream: podcast-upnext
        }
    }
    let fresh = store::upsert_episodes(&conn, &episodes).map_err(|e| e.to_string())?;
    let _ = store::set_feed_state(
        &conn,
        &show.id,
        &store::FeedState { etag, last_modified, last_refresh: Some(now_secs()) },
    );
    if first_sync {
        return Ok(Vec::new());
    }
    let cutoff = now_secs() - NEW_WINDOW;
    let mut new: Vec<Episode> = episodes
        .into_iter()
        .filter(|e| fresh.contains(&e.id) && e.published_at.unwrap_or(i64::MAX) >= cutoff)
        .collect();
    store::decorate(&conn, &mut new);
    Ok(new)
}
