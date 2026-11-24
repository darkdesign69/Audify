//! The podcast commands with a body (search, show pages, episode, follow), shared by the desktop's
//! Tauri wrappers (`src-tauri/src/podcasts/commands.rs`) and the iOS core (`crates/ffi`).

use std::sync::Arc;

use super::refresh;
use super::{itunes, search, store, youtube, Episode, EpisodePage, SearchResults, Show, ShowPage, Sort};
use crate::db::Db;
use crate::state::AppState;
use rusqlite::Connection;

const PAGE: usize = 50;
/// An RSS show page re-fetches the feed when it is older than this.
const SHOW_FRESH_SECS: i64 = 10 * 60;

pub fn client(st: &AppState) -> Result<&innertube::YouTubeClient, String> {
    st.clients.get(innertube::METADATA_CLIENT).ok_or_else(|| "metadata client missing".into())
}

pub(crate) fn mark_followed(st: &AppState, shows: &mut [Show]) {
    let conn = st.db.podcasts_conn();
    for s in shows.iter_mut() {
        s.followed = store::is_followed(&conn, &s.id);
    }
}

pub(crate) fn decorate(st: &AppState, eps: &mut [Episode]) {
    store::decorate(&st.db.podcasts_conn(), eps);
}

/// Search shows and/or episodes on YouTube Music and the directories, merged and deduplicated.
/// `scope`: `"shows"`, `"episodes"`, or absent for both.
pub async fn podcast_search(
    state: &Arc<AppState>,
    query: String,
    scope: Option<String>,
) -> Result<SearchResults, String> {
    let st = state.clone();
    let q = query.trim();
    if q.is_empty() {
        return Ok(SearchResults::default());
    }
    let want_shows = scope.as_deref() != Some("episodes");
    let want_eps = scope.as_deref() != Some("shows");
    let c = client(&st)?;
    let empty_shows = || async { Ok::<Vec<Show>, String>(Vec::new()) };
    let (yt_s, it_s, pi_s, yt_e, it_e, chart) = tokio::join!(
        async {
            if want_shows { youtube::search_shows(&st.it, c, q).await } else { empty_shows().await }
        },
        async { if want_shows { itunes::search_shows(&st.db, q).await } else { empty_shows().await } },
        async {
            if want_shows { itunes::search_podcastindex(&st.db, q).await } else { empty_shows().await }
        },
        async {
            if want_eps { youtube::search_episodes(&st.it, c, q).await } else { Ok(Vec::new()) }
        },
        async { if want_eps { itunes::search_episodes(&st.db, q).await } else { Ok(Vec::new()) } },
        // stream: search-pop — Apple's chart, a popularity signal (memory-cached, never waited on long).
        async { if want_shows { super::charts::top(&st.db).await } else { Default::default() } },
    );
    let errors: Vec<String> = [&yt_s.as_ref().err(), &it_s.as_ref().err(), &yt_e.as_ref().err(), &it_e.as_ref().err()]
        .iter()
        .filter_map(|e| e.map(|e| e.to_string()))
        .collect();
    let all_failed = yt_s.is_err() && it_s.is_err() && yt_e.is_err() && it_e.is_err();
    if all_failed {
        return Err(errors.join("; "));
    }
    let mut dir = it_s.unwrap_or_default();
    dir.extend(pi_s.unwrap_or_default());
    let mut shows = search::merge_shows(yt_s.unwrap_or_default(), dir);
    super::charts::apply(&mut shows, &chart);
    // stream: search2 — the host's own show and exact titles first ("chris williamson").
    shows = search::rank_shows(q, shows);
    let mut episodes = search::merge_episodes(yt_e.unwrap_or_default(), it_e.unwrap_or_default());
    mark_followed(&st, &mut shows);
    decorate(&st, &mut episodes);
    Ok(SearchResults { shows, episodes })
}

fn page_token(kind: &str, value: impl std::fmt::Display) -> String {
    format!("{kind}:{value}")
}

/// A show page: `id` is an `MPSP…` browse id or a feed URL. `sort` defaults to newest first.
pub async fn podcast_show(state: &Arc<AppState>, id: String, sort: Option<Sort>) -> Result<ShowPage, String> {
    let st = state.clone();
    let sort = sort.unwrap_or_default();
    if id.starts_with("MPSP") {
        let p = youtube::fetch_show(&st.it, client(&st)?, &id, sort).await?;
        let mut show = p.show;
        {
            let conn = st.db.podcasts_conn();
            let _ = store::upsert_episodes(&conn, &p.episodes);
            // stream: tilefix — fresh (signed, short-lived) art for recently played cards.
            store::remember_show_art(&conn, &id, show.thumbnail.as_deref());
            if store::is_followed(&conn, &id) {
                let _ = store::update_follow_meta(&conn, &id, &show);
            }
        }
        mark_followed(&st, std::slice::from_mut(&mut show));
        let mut episodes = p.episodes;
        decorate(&st, &mut episodes);
        return Ok(ShowPage {
            show,
            episodes,
            continuation: p.continuation.map(|t| page_token("yt", t)),
            sort,
        });
    }
    // RSS: refresh the stored copy when stale, then page from SQLite.
    let fs = store::feed_state(&st.db.podcasts_conn(), &id);
    let cache_key = format!("show:{id}");
    let cached: Option<Show> = store::cache_get(&st.db.podcasts_conn(), &cache_key, SHOW_FRESH_SECS)
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(|_| fs.last_refresh.is_some_and(|t| super::now_secs() - t < SHOW_FRESH_SECS));
    let mut show = match cached {
        Some(s) => s,
        None => {
            let (show, eps, etag, lm) = super::rss::fetch_show(&id).await?;
            let conn = st.db.podcasts_conn();
            store::upsert_episodes(&conn, &eps).map_err(|e| e.to_string())?;
            store::set_serial(&conn, &id, show.serial); // stream: podcast-upnext
            let _ = store::set_feed_state(
                &conn,
                &id,
                &store::FeedState { etag, last_modified: lm, last_refresh: Some(super::now_secs()) },
            );
            if store::is_followed(&conn, &id) {
                let _ = store::update_follow_meta(&conn, &id, &show);
            }
            store::cache_put(&conn, &cache_key, &serde_json::to_string(&show).unwrap_or_default());
            show
        }
    };
    let conn = st.db.podcasts_conn();
    let total = store::count_show_episodes(&conn, &id);
    show.episode_count = Some(total as u32);
    show.followed = store::is_followed(&conn, &id);
    let episodes = store::show_episodes(&conn, &id, sort, 0, PAGE).map_err(|e| e.to_string())?;
    Ok(ShowPage {
        show,
        episodes,
        continuation: (total > PAGE).then(|| page_token("rss", PAGE)),
        sort,
    })
}

/// The next page of a show's episodes. `continuation` is the token from the previous page.
pub async fn podcast_show_more(
    state: &Arc<AppState>,
    id: String,
    continuation: String,
    sort: Option<Sort>,
) -> Result<EpisodePage, String> {
    let st = state.clone();
    let sort = sort.unwrap_or_default();
    if let Some(tok) = continuation.strip_prefix("yt:") {
        let title = store::follows(&st.db.podcasts_conn())
            .ok()
            .and_then(|f| f.into_iter().find(|s| s.id == id).map(|s| s.title))
            .unwrap_or_default();
        let (mut eps, next) = youtube::fetch_more(&st.it, client(&st)?, tok, &id, &title).await?;
        if title.is_empty() {
            // Not followed: take the show name from the rows we already stored for it.
            if let Some(name) = store::show_episodes(&st.db.podcasts_conn(), &id, Sort::Newest, 0, 1)
                .ok()
                .and_then(|v| v.into_iter().next())
                .map(|e| e.show_title)
            {
                eps.iter_mut().for_each(|e| e.show_title = name.clone());
            }
        }
        let _ = store::upsert_episodes(&st.db.podcasts_conn(), &eps);
        decorate(&st, &mut eps);
        return Ok(EpisodePage { episodes: eps, continuation: next.map(|t| page_token("yt", t)) });
    }
    let offset: usize = continuation
        .strip_prefix("rss:")
        .and_then(|o| o.parse().ok())
        .ok_or("bad continuation")?;
    let conn = st.db.podcasts_conn();
    let total = store::count_show_episodes(&conn, &id);
    let episodes = store::show_episodes(&conn, &id, sort, offset, PAGE).map_err(|e| e.to_string())?;
    let next = offset + PAGE;
    Ok(EpisodePage { episodes, continuation: (total > next).then(|| page_token("rss", next)) })
}

/// One episode with full show notes (the episode page).
pub async fn podcast_episode(state: &Arc<AppState>, id: String) -> Result<Episode, String> {
    let st = state.clone();
    // A YouTube episode is stored by its videoId; search and explore cards carry its page's
    // browseId (`MPED<videoId>`).
    let id = id.strip_prefix("MPED").map(str::to_owned).unwrap_or(id);
    if !super::is_rss_id(&id) {
        let mut e = youtube::fetch_episode(&st.it, client(&st)?, &id).await?;
        let conn = st.db.podcasts_conn();
        let _ = store::upsert_episodes(&conn, std::slice::from_ref(&e));
        if let Some(stored) = store::get_episode(&conn, &id) {
            e.published_at = stored.published_at.or(e.published_at);
            e.is_video = stored.is_video;
        }
        store::decorate(&conn, std::slice::from_mut(&mut e));
        return Ok(e);
    }
    let stored = store::get_episode(&st.db.podcasts_conn(), &id);
    stored.ok_or_else(|| "episode not found".into())
}

/// Follow a show (from search or its page). Its episodes are fetched in the background so the
/// new-episodes feed starts from what exists now.
pub async fn podcast_follow(state: &Arc<AppState>, show: Show) -> Result<Show, String> {
    let st = state.clone();
    if show.id.is_empty() {
        return Err("show id missing".into());
    }
    store::follow(&st.db.podcasts_conn(), &show).map_err(|e| e.to_string())?;
    let mut out = show.clone();
    out.followed = true;
    crate::rt::spawn(async move {
        if let Err(e) = refresh::refresh_show(&st, &show).await {
            tracing::debug!("podcasts: first sync of {} failed: {e}", show.id);
        }
    });
    Ok(out)
}


/// stream: chapters. An episode's chapters (episode page, Now Playing, scrubber marks). Cached per
/// episode; `duration_secs` is the player's length when known.
pub async fn podcast_chapters(
    state: &Arc<AppState>,
    id: String,
    duration_secs: Option<f64>,
) -> Result<super::chapters::Chapters, String> {
    Ok(super::chapters::chapters(state, &id, duration_secs).await)
}

// ── Settings › Podcasts and OPML: one body for the desktop and the iPhone ──

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PodcastSettings {
    /// Skip SponsorBlock segments in YouTube episodes (default on).
    pub sponsorblock: bool,
    /// `sponsor`, `selfpromo`, `interaction`, `intro`, `outro`, `preview`, `music_offtopic`, `filler`.
    pub sponsorblock_categories: Vec<String>,
    /// Two-letter store country for the iTunes directory (default US).
    pub country: String,
    /// Podcast Index API key/secret. Write-only: reads return empty strings and `podcastindex`.
    pub podcastindex_key: Option<String>,
    pub podcastindex_secret: Option<String>,
    pub podcastindex: bool,
}

pub fn podcast_settings(db: &Db) -> PodcastSettings {
    PodcastSettings {
        sponsorblock: super::sponsorblock::enabled(db),
        sponsorblock_categories: super::sponsorblock::categories(db),
        country: db.get_setting("podcast_country").unwrap_or_else(|| "US".into()),
        podcastindex_key: None,
        podcastindex_secret: None,
        podcastindex: itunes::podcastindex_keys(db).is_some(),
    }
}

/// Partial update: only fields present in `settings` change.
pub fn podcast_set_settings(db: &Db, settings: &serde_json::Value) -> PodcastSettings {
    if let Some(b) = settings.get("sponsorblock").and_then(|v| v.as_bool()) {
        db.set_setting("podcast_sponsorblock", if b { "true" } else { "false" });
    }
    if let Some(c) = settings.get("sponsorblockCategories").and_then(|v| v.as_array()) {
        let cats: Vec<&str> = c.iter().filter_map(|v| v.as_str()).collect();
        db.set_setting("podcast_sponsorblock_categories", &serde_json::to_string(&cats).unwrap_or_default());
    }
    if let Some(c) = settings.get("country").and_then(|v| v.as_str()) {
        db.set_setting("podcast_country", &c.to_ascii_uppercase());
    }
    for (k, key) in [("podcastindexKey", "podcastindex_key"), ("podcastindexSecret", "podcastindex_secret")] {
        if let Some(v) = settings.get(k).and_then(|v| v.as_str()) {
            db.set_setting(key, v.trim());
        }
    }
    podcast_settings(db)
}

/// The followed shows as OPML text.
pub fn podcast_opml_export(conn: &Connection) -> Result<String, String> {
    super::opml::export(&store::follows(conn).map_err(|e| e.to_string())?)
}

#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub found: usize,
    pub added: usize,
    pub already_followed: usize,
}

/// Follow every feed in OPML text; answers the report and the shows newly followed.
fn follow_opml(conn: &Connection, xml: &str) -> Result<(ImportReport, Vec<Show>), String> {
    let entries = super::opml::import(xml);
    if entries.is_empty() {
        return Err("No podcast feeds found in that file.".into());
    }
    let mut report = ImportReport { found: entries.len(), ..Default::default() };
    let mut added = Vec::new();
    for e in &entries {
        if store::is_followed(conn, &e.id) {
            report.already_followed += 1;
            continue;
        }
        let show = e.show();
        if store::follow(conn, &show).is_ok() {
            report.added += 1;
            added.push(show);
        }
    }
    Ok((report, added))
}

/// Import follows from OPML text. Titles and artwork fill in as the background sync reaches each.
pub fn podcast_opml_import(state: &Arc<AppState>, xml: &str) -> Result<ImportReport, String> {
    let (report, added) = follow_opml(&state.db.podcasts_conn(), xml)?;
    // ── stream: reliability ── first sync of imported shows: background priority.
    let st = state.clone();
    crate::rt::spawn(innertube::background(async move {
        use futures_util::StreamExt;
        futures_util::stream::iter(added)
            .for_each_concurrent(4, |show| {
                let st = st.clone();
                async move {
                    let _ = refresh::refresh_show(&st, &show).await;
                }
            })
            .await;
    }));
    Ok(report)
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn opml_import_follows_new_feeds_and_counts_known_ones() {
        let c = store::test_conn();
        let xml = r#"<?xml version="1.0"?><opml version="2.0"><head/><body>
            <outline type="rss" text="A" xmlUrl="https://a.example/feed.xml"/>
            <outline type="rss" text="B" xmlUrl="https://b.example/feed.xml"/>
        </body></opml>"#;
        let (r, added) = follow_opml(&c, xml).unwrap();
        assert_eq!((r.found, r.added, r.already_followed, added.len()), (2, 2, 0, 2));
        let (r, _) = follow_opml(&c, xml).unwrap();
        assert_eq!((r.found, r.added, r.already_followed), (2, 0, 2));
        assert!(podcast_opml_export(&c).unwrap().contains("https://a.example/feed.xml"));
        assert!(follow_opml(&c, "not opml").is_err());
    }

    #[test]
    fn settings_patch_changes_only_given_fields() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let before = podcast_settings(&db);
        let after = podcast_set_settings(&db, &serde_json::json!({ "sponsorblock": !before.sponsorblock, "sponsorblockCategories": ["intro"] }));
        assert_eq!(after.sponsorblock, !before.sponsorblock);
        assert_eq!(after.sponsorblock_categories, vec!["intro".to_string()]);
        assert_eq!(after.country, before.country);
    }
}
