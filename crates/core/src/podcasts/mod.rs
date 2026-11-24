//! Podcasts (stream E): Spotify-grade shows and episodes with no account.
//!
//! Two sources behind one model:
//! - **YouTube Music** (`MPSP…` show pages, episode rows): ad-free audio, video podcasts,
//!   SponsorBlock. Searched with the Podcasts / Episodes filters.
//! - **RSS** (any feed URL, found through the keyless iTunes Search API, or Podcast Index when the
//!   user configured keys): the long tail YouTube doesn't carry. Enclosures play straight through
//!   mpv.
//!
//! Everything personal lives in SQLite (`store`): follows, episodes seen, resume positions,
//! played state, saved episodes ("Your Episodes"), the SponsorBlock and search caches.
//!
//! How playback is wired (the only edits outside this folder are marked `stream: podcasts`):
//! - An RSS episode is a `SongItem` whose `video_id` is [`RSS_PREFIX`] + the enclosure URL, and
//!   `AppState::resolve` hands that URL to mpv as is ([`playback_data`]).
//! - `start_current` asks [`progress::resume_position`] for a start offset.
//! - `emit_now_playing` tells [`progress::track_started`]; `on_position` / `on_duration` /
//!   `flush_position` feed [`progress`], which saves the position every 5 s, marks played by the
//!   AntennaPod / Pocket Casts rules ([`rules`]), and returns SponsorBlock skips.
//! - Episodes never reach the local play counts, so they don't shape music recommendations.
//!
//! See `docs/streams/podcasts.md` for the command list and what the UI still needs.

pub mod api;
pub mod chapters;
pub mod charts; // stream: search-pop
pub mod itunes;
pub mod opml;
pub mod progress;
pub mod refresh;
pub mod rss;
pub mod rules;
pub mod sanitize;
pub mod search;
pub mod sponsorblock;
pub mod store;
pub mod upnext; // stream: podcast-upnext
pub mod video; // stream: video-pods
pub mod youtube;

#[cfg(test)]
mod live_tests;

use std::collections::HashMap;

use innertube::SongItem;
use serde::{Deserialize, Serialize};

/// A playable id for an RSS episode: this prefix + the enclosure URL.
pub const RSS_PREFIX: &str = "PODURL:";

/// True when `video_id` is an RSS enclosure rather than a YouTube video.
pub fn is_rss_id(video_id: &str) -> bool {
    video_id.starts_with(RSS_PREFIX)
}

/// The enclosure URL behind an RSS episode id.
pub fn rss_url(video_id: &str) -> Option<&str> {
    video_id.strip_prefix(RSS_PREFIX)
}

/// The play id for an enclosure URL.
pub fn rss_id(url: &str) -> String {
    format!("{RSS_PREFIX}{url}")
}

/// A queue row that is a podcast episode (YouTube's flag, or an RSS enclosure).
pub fn is_episode_item(item: &SongItem) -> bool {
    item.is_episode || is_rss_id(&item.video_id)
}

/// `AppState::resolve` for an RSS episode: mpv opens the enclosure URL directly (redirects,
/// podtrac / chartable prefixes and all). `None` for anything else.
pub fn playback_data(video_id: &str) -> Option<crate::orchestrator::PlaybackData> {
    let url = rss_url(video_id)?;
    Some(crate::orchestrator::PlaybackData {
        video_id: video_id.to_owned(),
        stream_url: url.to_owned(),
        itag: 0,
        headers: HashMap::new(),
        // Never cached in the stream table: resolve returns before the cache is consulted.
        expires_in_seconds: i64::MAX / 2,
        // No loudness metadata: speech plays as mastered.
        loudness_db: None,
        playback_ping: None,
        title: None,
        artists: None,
        duration: None,
        is_video: Some(false),
        is_episode: Some(true),
        live: false,
        thumbnail: None,
        stream_client: "rss".to_owned(),
    })
}

/// Where a show comes from. A show found on both carries both ids; `source` says which one its
/// page and episodes are read from (YouTube when it has a browse id).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShowSource {
    #[default]
    Youtube,
    Rss,
}

/// A podcast show. `id` is the YouTube browse id (`MPSP…`) or the feed URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Show {
    pub id: String,
    pub title: String,
    pub author: String,
    pub thumbnail: Option<String>,
    /// Plain text.
    pub description: Option<String>,
    pub feed_url: Option<String>,
    pub yt_browse_id: Option<String>,
    pub itunes_id: Option<i64>,
    pub episode_count: Option<u32>,
    pub source: ShowSource,
    /// Filled from the follows table on the way out.
    pub followed: bool,
    /// stream: search2 — the directory's author when this YouTube show was folded with a directory
    /// listing that names someone else (Apple's "Alex Cooper" for YouTube's "Call Her Daddy"
    /// channel). Search matches a host's name against both.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir_author: Option<String>,
    /// stream: search-pop — the show's place (1-based) on Apple's Top Shows chart for the
    /// directory's country, when it is on it. A popularity signal for search ranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_rank: Option<u32>,
    /// stream: podcast-upnext — the feed says `<itunes:type>serial</itunes:type>`: episodes are
    /// meant to be heard oldest first, so up next never goes back to an older one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub serial: bool,
}

impl Show {
    pub fn youtube(browse_id: &str) -> Self {
        Show {
            id: browse_id.to_owned(),
            yt_browse_id: Some(browse_id.to_owned()),
            source: ShowSource::Youtube,
            ..Default::default()
        }
    }

    pub fn rss(feed_url: &str) -> Self {
        Show {
            id: feed_url.to_owned(),
            feed_url: Some(feed_url.to_owned()),
            source: ShowSource::Rss,
            ..Default::default()
        }
    }

    pub fn is_youtube(&self) -> bool {
        self.id.starts_with("MPSP")
    }
}

/// Listening state of one episode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Progress {
    pub position_secs: f64,
    pub duration_secs: f64,
    /// 0..=1 for the row's progress bar; 1 once played.
    pub fraction: f64,
    pub played: bool,
    /// Unix seconds of the last save, 0 when never started.
    pub updated_at: i64,
}

/// One episode. `id` is what the player plays: a YouTube videoId or [`RSS_PREFIX`] + enclosure.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Episode {
    pub id: String,
    pub show_id: Option<String>,
    pub show_title: String,
    pub title: String,
    /// Unix seconds. Day precision for YouTube rows ("5d ago").
    pub published_at: Option<i64>,
    pub duration_secs: Option<i64>,
    /// Show notes, sanitised HTML. Timestamps are `<a data-seek="secs">` links.
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    pub video_id: Option<String>,
    pub enclosure_url: Option<String>,
    pub guid: Option<String>,
    pub is_video: bool,
    pub progress: Progress,
    /// In "Your Episodes".
    pub saved: bool,
    /// stream: chapters. The feed's Podcasting 2.0 `<podcast:chapters>` file (RSS episodes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapters_url: Option<String>,
}

impl Episode {
    /// The queue row the existing play commands take (the UI builds the same row with
    /// `episodeSong` in api.ts; this is the Rust side's, for queueing a show from Rust later).
    #[allow(dead_code)]
    pub fn song(&self) -> SongItem {
        SongItem {
            video_id: self.id.clone(),
            title: self.title.clone(),
            artists: self.show_title.clone(),
            duration: self.duration_secs.map(format_duration),
            thumbnail: self.thumbnail.clone(),
            is_episode: true,
            ..Default::default()
        }
    }
}

#[allow(dead_code)]
/// `3725` → `"1:02:05"`, `305` → `"5:05"`: the colon form every queue row uses.
pub fn format_duration(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Episode list order.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
}

/// A show page: header, first page of episodes, and the token for the next page.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowPage {
    pub show: Show,
    pub episodes: Vec<Episode>,
    pub continuation: Option<String>,
    pub sort: Sort,
}

/// One more page of a show's episodes.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodePage {
    pub episodes: Vec<Episode>,
    pub continuation: Option<String>,
}

/// Combined podcast search.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub shows: Vec<Show>,
    pub episodes: Vec<Episode>,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Setup hook (lib.rs): create the tables, start the progress tracker and the refresh loop.
pub fn init(db: std::sync::Arc<crate::db::Db>) {
    if let Err(e) = store::init(&db.podcasts_conn()) {
        tracing::error!("podcasts: cannot create tables: {e}");
    }
    // stream: search-pop — warm Apple's chart so the first search ranks with it.
    charts::prewarm(db.clone());
    progress::init(db);
    refresh::spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_ids_round_trip() {
        let id = rss_id("https://example.com/a.mp3");
        assert!(is_rss_id(&id));
        assert_eq!(rss_url(&id), Some("https://example.com/a.mp3"));
        assert!(!is_rss_id("dQw4w9WgXcQ"));
        let d = playback_data(&id).unwrap();
        assert_eq!(d.stream_url, "https://example.com/a.mp3");
        assert_eq!(d.is_episode, Some(true));
        assert!(playback_data("dQw4w9WgXcQ").is_none());
    }

    #[test]
    fn episode_song_is_an_episode_row() {
        let e = Episode {
            id: rss_id("https://x/y.mp3"),
            title: "T".into(),
            show_title: "S".into(),
            duration_secs: Some(3725),
            ..Default::default()
        };
        let s = e.song();
        assert!(s.is_episode);
        assert_eq!(s.artists, "S");
        assert_eq!(s.duration.as_deref(), Some("1:02:05"));
        assert!(is_episode_item(&s));
        assert_eq!(format_duration(305), "5:05");
    }
}
