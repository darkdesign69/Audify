//! Made for you (stream smart2): personal playlists built on this machine from the taste profile
//! and YouTube Music candidates, saved in SQLite and refreshed on a daily or weekly schedule.
//!
//! | id | playlist | built from | refreshed |
//! |---|---|---|---|
//! | `MFY_DAILY_1`..`6` | Your Mix 1–6 | top artists clustered (related + co-listen edges): favourites + radio fill (`home::mix_tracks`) | daily |
//! | `MFY_DISCOVER_WEEKLY` | Weekly Discoveries, 30 songs new to you | RRF seed voting over the radios of recent songs (the benchmark winner, `recs::vote`), everything known removed | Mondays |
//! | `MFY_RELEASE_RADAR` | New Release Mix | the newest releases of followed + top artists (`whatsnew` registry) | Fridays |
//! | `MFY_REPEAT_REWIND` | Throwback Mix | past favourites not heard in 30 days (Metrolist "forgotten favorites") + old likes | Mondays |
//! | `LIMUSIC_ON_REPEAT` | Your Rotation | most played in 30 days (Limusic's live auto-playlist, kept) | live |
//!
//! The ids are stored (SQLite rows, cover file names, shortcuts) and keep their first wording;
//! the names shown come from `title` alone and are reapplied on load.
//!
//! Each playlist is a real playlist page: `get_playlist(MFY_…)` answers it from here. Covers are
//! one generated SVG per playlist under `covers/mfy/`: artwork or a colour band, our mark
//! top-left, the name bottom-left.
//!
//! Taste comes from plays, likes, playlist adds and follows (`taste.rs`), so a Spotify import
//! (likes, playlists, followed artists) seeds everything at once. A playlist built from no taste
//! signals is rebuilt as soon as there are some.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{Datelike, Local, TimeZone};
use innertube::SongItem;
use serde::{Deserialize, Serialize};

use super::home::{ShelfItem, Shelf};
use super::library::Filter;
use crate::db::Db;
use crate::state::AppState;

pub const PREFIX: &str = "MFY_";
pub const DAILY: &str = "MFY_DAILY_";
pub const DISCOVER_WEEKLY: &str = "MFY_DISCOVER_WEEKLY";
pub const RELEASE_RADAR: &str = "MFY_RELEASE_RADAR";
pub const REPEAT_REWIND: &str = "MFY_REPEAT_REWIND";
pub const ON_REPEAT: &str = crate::state::ON_REPEAT_ID;

const DW_SIZE: usize = 30;
const RR_SIZE: usize = 30;
const REWIND_SIZE: usize = 30;
const DAILY_SIZE: usize = 50;
const MAX_DAILY: usize = 6;

/// The current cover design. Bump it whenever `cover_svg` changes look, so covers already on disk
/// are redrawn at the next launch (`needs_new_cover`) instead of waiting for the next rebuild.
/// 1 = parity3's per-playlist cover layouts.
/// 2 = a first Spotless tile (stream/logo, never merged; drawn on the dev Mac).
/// 3 = the Spotless mark (pink disc, black note; assets/brand/spotless.svg) in place of the disc
///     with three bars.
/// 5 = one layout for every playlist (artwork or band, mark, name) in Spotless colours, under the
///     current names (`title`).
pub const COVER_VERSION: u32 = 5;

pub fn is_mfy(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// The name a playlist is shown under. Stored rows get it on load (`parse`), so a list saved
/// under an older name shows the current one.
pub fn title(kind: Kind, id: &str) -> String {
    match kind {
        Kind::Daily => format!("Your Mix {}", daily_n(id)),
        Kind::DiscoverWeekly => "Weekly Discoveries".into(),
        Kind::ReleaseRadar => "New Release Mix".into(),
        Kind::RepeatRewind => "Throwback Mix".into(),
        Kind::OnRepeat => "Your Rotation".into(),
    }
}

/// A mix's number from its id (`MFY_DAILY_3` → 3).
fn daily_n(id: &str) -> usize {
    id.trim_start_matches(DAILY).parse::<usize>().unwrap_or(1).max(1)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Daily,
    DiscoverWeekly,
    ReleaseRadar,
    RepeatRewind,
    OnRepeat,
}

/// A stored Made for you playlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mfy {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub description: String,
    /// The generated cover (an SVG file path), else the artwork it was made from.
    pub cover: Option<String>,
    pub tracks: Vec<SongItem>,
    pub generated_at: i64,
    /// Taste signals on record when it was built (0 = cold; rebuilt once there are some).
    pub signals: usize,
    /// The cover design `cover` was drawn with (`COVER_VERSION`; 0 = before versioning). An
    /// older one is redrawn on its own at the next refresh, without rebuilding the tracks.
    #[serde(default)]
    pub cover_version: u32,
}

/// A Made for you card for Home / Search / the library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MfyCard {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub description: String,
    pub thumbnail: Option<String>,
    pub track_count: usize,
    pub generated_at: i64,
    /// When the next scheduled refresh is due (unix secs).
    pub next_refresh: i64,
}

impl Mfy {
    pub fn card(&self) -> MfyCard {
        MfyCard {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            description: self.description.clone(),
            thumbnail: cover_url(self.cover.as_deref()),
            track_count: self.tracks.len(),
            generated_at: self.generated_at,
            next_refresh: next_period(self.kind, self.generated_at),
        }
    }

    pub fn shelf_item(&self) -> ShelfItem {
        let c = self.card();
        ShelfItem {
            kind: "playlist".into(),
            id: c.id,
            title: c.title,
            subtitle: Some(c.description),
            thumbnail: c.thumbnail,
            explicit: false,
            song: None,
            params: None,
            color: None,
            progress: None,
            new_episode: false,
        }
    }
}

pub fn migrate(db: &Db) {
    db.with_conn(|c| {
        let _ = c.execute_batch(
            "CREATE TABLE IF NOT EXISTS smart_mfy (
                id           TEXT PRIMARY KEY,
                json         TEXT NOT NULL,
                generated_at INTEGER NOT NULL,
                position     INTEGER NOT NULL DEFAULT 0
            );",
        );
    });
}

// --- schedule ------------------------------------------------------------------------------------

/// Local midnight of the day `at` falls on.
fn local_midnight(at: i64) -> chrono::DateTime<Local> {
    let t = Local.timestamp_opt(at, 0).single().unwrap_or_else(Local::now);
    Local
        .with_ymd_and_hms(t.year(), t.month(), t.day(), 0, 0, 0)
        .earliest()
        .unwrap_or(t)
}

/// Start of the refresh period `at` falls in: daily for the mixes / Your Rotation, the last Monday
/// for Weekly Discoveries and Throwback Mix, the last Friday for New Release Mix.
pub fn period_start(kind: Kind, at: i64) -> i64 {
    let day = local_midnight(at);
    let back = match kind {
        Kind::Daily | Kind::OnRepeat => 0,
        Kind::DiscoverWeekly | Kind::RepeatRewind => day.weekday().num_days_from_monday() as i64,
        Kind::ReleaseRadar => (day.weekday().num_days_from_monday() as i64 + 7 - 4) % 7,
    };
    (day - chrono::Duration::days(back)).timestamp()
}

pub fn next_period(kind: Kind, generated_at: i64) -> i64 {
    let days = match kind {
        Kind::Daily | Kind::OnRepeat => 1,
        _ => 7,
    };
    // Local midnight `days` after the period start (DST-safe: re-anchored on the date).
    let start = Local.timestamp_opt(period_start(kind, generated_at), 0).single();
    match start {
        Some(s) => local_midnight((s + chrono::Duration::days(days) + chrono::Duration::hours(3)).timestamp())
            .timestamp(),
        None => generated_at + days * 86_400,
    }
}

/// Whether a stored playlist needs rebuilding now.
pub fn is_stale(m: &Mfy, now: i64, signals: usize) -> bool {
    if m.generated_at < period_start(m.kind, now) {
        return true;
    }
    // Built cold (or came out empty) and there is taste now: an import just landed.
    (m.signals == 0 || m.tracks.is_empty()) && signals > 0 && now - m.generated_at > 60
}

// --- store ---------------------------------------------------------------------------------------

pub fn load_all(db: &Db) -> Vec<Mfy> {
    db.with_conn(|c| {
        let Ok(mut st) = c.prepare("SELECT json FROM smart_mfy ORDER BY position") else {
            return Vec::new();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|j| parse(&j)).collect())
            .unwrap_or_default()
    })
}

pub fn load(db: &Db, id: &str) -> Option<Mfy> {
    db.with_conn(|c| {
        c.query_row("SELECT json FROM smart_mfy WHERE id = ?1", [id], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|j| parse(&j))
    })
}

/// A stored row, under its current name.
fn parse(json: &str) -> Option<Mfy> {
    let mut m: Mfy = serde_json::from_str(json).ok()?;
    m.title = title(m.kind, &m.id);
    Some(m)
}

fn position(kind: Kind, id: &str) -> i64 {
    match kind {
        Kind::Daily => id.trim_start_matches(DAILY).parse::<i64>().unwrap_or(9),
        Kind::DiscoverWeekly => 10,
        Kind::ReleaseRadar => 11,
        Kind::OnRepeat => 12,
        Kind::RepeatRewind => 13,
    }
}

pub fn save(db: &Db, m: &Mfy) {
    let Ok(json) = serde_json::to_string(m) else { return };
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_mfy(id, json, generated_at, position) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json,
                 generated_at = excluded.generated_at, position = excluded.position",
            rusqlite::params![m.id, json, m.generated_at, position(m.kind, &m.id)],
        );
    });
}

fn delete(db: &Db, id: &str) {
    db.with_conn(|c| {
        let _ = c.execute("DELETE FROM smart_mfy WHERE id = ?1", [id]);
    });
}

// --- local history helpers -------------------------------------------------------------------

/// Every song this machine knows the user has: heard, liked, or in one of their playlists.
pub fn known_songs(db: &Db) -> Vec<SongItem> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    db.with_conn(|c| {
        for sql in [
            "SELECT song_json FROM listen_songs",
            "SELECT song_json FROM liked_songs",
            "SELECT song_json FROM local_playlist_tracks",
        ] {
            if let Ok(mut st) = c.prepare(sql) {
                if let Ok(rows) = st.query_map([], |r| r.get::<_, String>(0)) {
                    for j in rows.flatten() {
                        if let Ok(s) = serde_json::from_str::<SongItem>(&j) {
                            if seen.insert(s.video_id.clone()) {
                                out.push(s);
                            }
                        }
                    }
                }
            }
        }
    });
    out
}

/// Liked songs, most recently liked first.
fn liked_songs(db: &Db) -> Vec<(SongItem, i64)> {
    db.with_conn(|c| {
        let Ok(mut st) =
            c.prepare("SELECT song_json, added_at FROM liked_songs ORDER BY added_at DESC")
        else {
            return Vec::new();
        };
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|(j, at)| serde_json::from_str(&j).ok().map(|s| (s, at)))
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Weekly Discoveries seeds: the 5 most recently heard songs, topped up with recent likes and then
/// playlist adds (an import with no plays yet).
pub fn dw_seeds(db: &Db, n: usize) -> Vec<SongItem> {
    let mut out: Vec<SongItem> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |s: SongItem, out: &mut Vec<SongItem>| {
        if out.len() < n
            && !crate::local::is_local_song(&s.video_id)
            && seen.insert(s.video_id.clone())
        {
            out.push(s);
        }
    };
    for h in super::home::recent_songs(db, n * 2) {
        push(h.song, &mut out);
    }
    for (s, _) in liked_songs(db).into_iter().take(n * 2) {
        push(s, &mut out);
    }
    let adds: Vec<SongItem> = db.with_conn(|c| {
        let Ok(mut st) = c.prepare(
            "SELECT song_json FROM local_playlist_tracks ORDER BY added_at DESC LIMIT 20",
        ) else {
            return Vec::new();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|j| serde_json::from_str(&j).ok()).collect())
            .unwrap_or_default()
    });
    for s in adds {
        push(s, &mut out);
    }
    out
}

/// Throwback Mix (pure over rows): songs played at least 3 times but not in the last 30 days,
/// most played first; then old likes not heard lately. Private plays are excluded by the query.
pub fn rewind_pick(
    plays: Vec<(SongItem, i64, i64)>,
    likes: Vec<(SongItem, i64)>,
    now: i64,
    n: usize,
) -> Vec<SongItem> {
    let month = 30 * 86_400;
    let heard_lately: HashSet<String> = plays
        .iter()
        .filter(|(_, _, last)| now - last < month)
        .map(|(s, _, _)| s.video_id.clone())
        .collect();
    let mut old: Vec<(SongItem, i64, i64)> = plays
        .into_iter()
        .filter(|(_, count, last)| *count >= 3 && now - last >= month)
        .collect();
    old.sort_by_key(|(_, count, last)| (std::cmp::Reverse(*count), std::cmp::Reverse(*last)));
    let mut out: Vec<SongItem> = Vec::new();
    let mut seen = HashSet::new();
    for (s, _, _) in old {
        if seen.insert(s.video_id.clone()) {
            out.push(s);
        }
    }
    for (s, at) in likes {
        if out.len() >= n {
            break;
        }
        if now - at >= 3 * month
            && !heard_lately.contains(&s.video_id)
            && seen.insert(s.video_id.clone())
        {
            out.push(s);
        }
    }
    out.truncate(n);
    out
}

fn play_history(db: &Db) -> Vec<(SongItem, i64, i64)> {
    db.with_conn(|c| {
        let Ok(mut st) = c.prepare(
            "SELECT s.song_json, COUNT(*), MAX(l.played_at)
             FROM listens l JOIN listen_songs s ON s.video_id = l.video_id
             WHERE l.id NOT IN (SELECT listen_id FROM smart_private_listens)
               AND l.video_id NOT LIKE 'LOCAL:%'
             GROUP BY l.video_id",
        ) else {
            return Vec::new();
        };
        st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map(|rows| {
            rows.flatten()
                .filter_map(|(j, n, last)| serde_json::from_str(&j).ok().map(|s| (s, n, last)))
                .collect()
        })
        .unwrap_or_default()
    })
}

fn clean(mut v: Vec<SongItem>) -> Vec<SongItem> {
    for s in &mut v {
        s.autoplay = false;
        s.queued = false;
        s.queued_end = false;
    }
    v
}

// --- generators ----------------------------------------------------------------------------------

/// Weekly Discoveries: the benchmark's winning discovery ranker (RRF seed voting over YouTube radios
/// of recent songs, taste nudge, artist spread), with everything already known left out.
pub async fn build_discover_weekly(state: &AppState, filter: &mut Filter) -> Vec<SongItem> {
    let db = &state.db;
    let seeds = dw_seeds(db, 5);
    if seeds.is_empty() {
        return Vec::new();
    }
    let radios = futures_util::future::join_all(
        seeds.iter().map(|s| super::recs::song_radio(state, &s.video_id)),
    )
    .await;
    let radios: Vec<(SongItem, Vec<SongItem>)> = seeds.into_iter().zip(radios).collect();
    if filter.explicit_on() {
        // Radio rows carry no badge: ask Deezer about a batch first (cached for good).
        let all: Vec<SongItem> = radios.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        super::explicit::resolve_unknown(db, &all, filter, 25).await;
    }
    let known = known_songs(db);
    let taste = super::taste::profile(db);
    let recs = super::recs::vote(&known, &radios, filter, &taste, DW_SIZE * 2);
    // One song per artist first (a discovery list wants that spread), then a second.
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut first = Vec::new();
    let mut second = Vec::new();
    for r in recs {
        let n = per.entry(super::shuffle::artist_key(&r.song)).or_default();
        *n += 1;
        match *n {
            1 => first.push(r.song),
            2 => second.push(r.song),
            _ => {}
        }
    }
    first.extend(second);
    first.truncate(DW_SIZE);
    // ── stream: canonical ── the radios are canonical already; this catches anything left.
    let first = crate::canonical::canonicalize(state, first, None).await;
    clean(first)
}

/// New Release Mix: the newest releases of followed and top artists, a couple of tracks each.
pub async fn build_release_radar(state: &AppState, filter: &Filter) -> Vec<SongItem> {
    let releases = super::whatsnew::radar_releases(state, 15).await;
    let Some(client) = state.clients.get(innertube::METADATA_CLIENT) else { return Vec::new() };
    let pages = futures_util::future::join_all(releases.iter().map(|r| async move {
        let key = format!("album:{}", r.id);
        if let Some(v) = super::cache_get::<Vec<SongItem>>(&state.db, &key, 7 * 86_400) {
            return v;
        }
        match state.it.album(client, &r.id).await {
            Ok(p) => {
                super::cache_put(&state.db, &key, &p.items);
                p.items
            }
            Err(e) => {
                tracing::warn!(error = %e, "smart: release album failed");
                Vec::new()
            }
        }
    }))
    .await;
    let mut out = Vec::new();
    for mut tracks in pages {
        filter.retain(&mut tracks, None);
        // A single (up to 3 tracks) whole; an album its first two, like Spotify's focus tracks.
        let take = if tracks.len() <= 3 { tracks.len() } else { 2 };
        out.extend(tracks.into_iter().take(take));
        if out.len() >= RR_SIZE {
            break;
        }
    }
    out.truncate(RR_SIZE);
    let out = crate::canonical::to_songs(state, out).await; // ── stream: canonical ──
    clean(out)
}

pub fn build_repeat_rewind(db: &Db, filter: &Filter) -> Vec<SongItem> {
    let mut out = rewind_pick(
        play_history(db),
        liked_songs(db),
        super::now_secs(),
        REWIND_SIZE + 10,
    );
    filter.retain(&mut out, None);
    out.truncate(REWIND_SIZE);
    clean(out)
}

fn names_line(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        _ => format!("{} and more", names.join(", ")),
    }
}

/// Build every stale Made for you playlist (all of them with `force`). Returns whether anything
/// changed. One build at a time.
pub async fn refresh(state: &AppState, force: bool) -> bool {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return false;
    }
    let changed = refresh_inner(state, force).await;
    RUNNING.store(false, Ordering::SeqCst);
    if changed {
        use crate::host::HostExt;
        let _ = state.host.emit("made-for-you-changed", ());
    }
    changed
}

async fn refresh_inner(state: &AppState, force: bool) -> bool {
    let db = &state.db;
    let now = super::now_secs();
    let signals = super::taste::load_signals(db).len();
    let stored: HashMap<String, Mfy> = load_all(db).into_iter().map(|m| (m.id.clone(), m)).collect();
    let stale = |id: &str, _kind: Kind| match stored.get(id) {
        // ── stream: canonical ── a list built before the canonical pass, holding music videos.
        Some(m) if m.tracks.iter().any(|s| s.is_video && !s.is_episode) => true,
        Some(m) => force || is_stale(m, now, signals),
        None => true,
    };
    let mut filter = Filter::load(db);
    let mut changed = false;
    let mut covers = Covers::new(state);

    // The mixes: rebuilt together (the clusters move as a set).
    if (1..=MAX_DAILY).any(|n| stale(&format!("{DAILY}{n}"), Kind::Daily)) && signals > 0 {
        let taste = super::taste::profile(db);
        let infos = super::home::top_artist_infos(state, &taste).await;
        let clusters = super::home::mix_clusters(db, &taste, &infos);
        let mut built = 0;
        for members in &clusters {
            let tracks = super::home::mix_tracks(
                state,
                &format!("dailymix:{}", members.join(",")),
                DAILY_SIZE,
            )
            .await
            .unwrap_or_default();
            if tracks.len() < 5 {
                continue;
            }
            built += 1;
            let names: Vec<String> = members
                .iter()
                .filter_map(|k| taste.artist_names.get(k).cloned())
                .filter(|n| !n.is_empty())
                .take(3)
                .collect();
            let art = infos
                .iter()
                .find(|i| members.contains(&i.id))
                .and_then(|i| i.thumbnail.clone())
                .or_else(|| tracks.first().and_then(|s| s.thumbnail.clone()));
            let id = format!("{DAILY}{built}");
            let title = title(Kind::Daily, &id);
            let cover = covers.make(&id, &title, art.as_deref(), built - 1).await;
            save(
                db,
                &Mfy {
                    id,
                    kind: Kind::Daily,
                    title,
                    description: names_line(&names),
                    cover,
                    tracks,
                    generated_at: now,
                    signals,
                    cover_version: COVER_VERSION,
                },
            );
        }
        for n in built + 1..=MAX_DAILY {
            delete(db, &format!("{DAILY}{n}"));
        }
        changed = true;
    }

    if stale(DISCOVER_WEEKLY, Kind::DiscoverWeekly) {
        let tracks = build_discover_weekly(state, &mut filter).await;
        if tracks.is_empty() {
            delete(db, DISCOVER_WEEKLY);
        } else {
            let art = tracks.first().and_then(|s| s.thumbnail.clone());
            let cover = covers.make(DISCOVER_WEEKLY, &title(Kind::DiscoverWeekly, DISCOVER_WEEKLY), art.as_deref(), 6).await;
            save(
                db,
                &Mfy {
                    id: DISCOVER_WEEKLY.into(),
                    kind: Kind::DiscoverWeekly,
                    title: title(Kind::DiscoverWeekly, DISCOVER_WEEKLY),
                    description: "Music new to you, picked from what you play. New every Monday."
                        .into(),
                    cover,
                    tracks,
                    generated_at: now,
                    signals,
                    cover_version: COVER_VERSION,
                },
            );
        }
        changed = true;
    }

    if stale(RELEASE_RADAR, Kind::ReleaseRadar) {
        let tracks = build_release_radar(state, &filter).await;
        if tracks.is_empty() {
            delete(db, RELEASE_RADAR);
        } else {
            let art = tracks.first().and_then(|s| s.thumbnail.clone());
            let cover = covers.make(RELEASE_RADAR, &title(Kind::ReleaseRadar, RELEASE_RADAR), art.as_deref(), 7).await;
            save(
                db,
                &Mfy {
                    id: RELEASE_RADAR.into(),
                    kind: Kind::ReleaseRadar,
                    title: title(Kind::ReleaseRadar, RELEASE_RADAR),
                    description:
                        "The latest from artists you follow and play most. New every Friday."
                            .into(),
                    cover,
                    tracks,
                    generated_at: now,
                    signals,
                    cover_version: COVER_VERSION,
                },
            );
        }
        changed = true;
    }

    if stale(REPEAT_REWIND, Kind::RepeatRewind) {
        // ── stream: canonical ── old plays of music videos come back as their songs.
        let tracks = crate::canonical::to_songs(state, build_repeat_rewind(db, &filter)).await;
        if tracks.len() < 5 {
            delete(db, REPEAT_REWIND);
        } else {
            let art = tracks.first().and_then(|s| s.thumbnail.clone());
            let cover = covers.make(REPEAT_REWIND, &title(Kind::RepeatRewind, REPEAT_REWIND), art.as_deref(), 8).await;
            save(
                db,
                &Mfy {
                    id: REPEAT_REWIND.into(),
                    kind: Kind::RepeatRewind,
                    title: title(Kind::RepeatRewind, REPEAT_REWIND),
                    description: "Past favourites you haven't played in a while.".into(),
                    cover,
                    tracks,
                    generated_at: now,
                    signals,
                    cover_version: COVER_VERSION,
                },
            );
        }
        changed = true;
    }

    // Your Rotation stays Limusic's live list; only its card (cover, count) is kept here.
    if stale(ON_REPEAT, Kind::OnRepeat) {
        let tracks: Vec<SongItem> = db
            .top_plays(now - crate::state::ON_REPEAT_WINDOW_SECS, crate::state::ON_REPEAT_LIMIT)
            .into_iter()
            .filter_map(|(j, _)| serde_json::from_str(&j).ok())
            .collect();
        if tracks.len() < 3 {
            delete(db, ON_REPEAT);
        } else {
            let cover = covers.make("MFY_ON_REPEAT", &title(Kind::OnRepeat, ON_REPEAT), None, 9).await;
            save(
                db,
                &Mfy {
                    id: ON_REPEAT.into(),
                    kind: Kind::OnRepeat,
                    title: title(Kind::OnRepeat, ON_REPEAT),
                    description: "The songs you can't stop playing right now.".into(),
                    cover,
                    tracks: Vec::new(),
                    generated_at: now,
                    signals,
                    cover_version: COVER_VERSION,
                },
            );
        }
        changed = true;
    }

    // Covers drawn with an older design: redraw the image only. The tracks, build time and
    // schedule stay, so this never stands in for (or delays) the rebuilds above.
    for mut m in load_all(db).into_iter().filter(needs_new_cover) {
        let (label, n) = cover_args(&m);
        let art = cover_art(m.cover.as_deref())
            .or_else(|| m.tracks.first().and_then(|s| s.thumbnail.clone()));
        let art = if m.kind == Kind::OnRepeat { None } else { art };
        let cover_id = if m.kind == Kind::OnRepeat { "MFY_ON_REPEAT" } else { m.id.as_str() };
        let cover = covers.make(cover_id, &label, art.as_deref(), n).await;
        if cover.is_some() {
            m.cover = cover;
        }
        m.cover_version = COVER_VERSION;
        save(db, &m);
        changed = true;
    }
    changed
}

/// Whether a stored playlist's cover was drawn with an older design.
pub fn needs_new_cover(m: &Mfy) -> bool {
    m.cover_version < COVER_VERSION
}

/// The `cover_svg` label and colour index a stored playlist's cover is drawn with (the same as
/// its build in `refresh_inner`).
fn cover_args(m: &Mfy) -> (String, usize) {
    let n = match m.kind {
        Kind::Daily => daily_n(&m.id) - 1,
        Kind::DiscoverWeekly => 6,
        Kind::ReleaseRadar => 7,
        Kind::RepeatRewind => 8,
        Kind::OnRepeat => 9,
    };
    (title(m.kind, &m.id), n)
}

/// The artwork an existing cover was made from, so a redraw needs no network: the `data:` image
/// embedded in the old SVG file (a mix's artist photo), or the cover itself when it is a
/// plain artwork URL (written when saving the SVG failed).
fn cover_art(cover: Option<&str>) -> Option<String> {
    let cover = cover?;
    if cover.starts_with("http") {
        return Some(cover.to_owned());
    }
    embedded_image(&std::fs::read_to_string(unversioned(cover)).ok()?)
}

/// The first embedded `data:` image of a cover SVG.
fn embedded_image(svg: &str) -> Option<String> {
    let start = svg.find("<image href=\"data:")? + "<image href=\"".len();
    let len = svg[start..].find('"')?;
    Some(svg[start..start + len].replace("&quot;", "\"").replace("&lt;", "<").replace("&amp;", "&"))
}

/// Cards for Home / Search, best first. Builds what is missing (first run, right after an
/// import); a stale set is served as is and rebuilt in the background.
pub async fn cards(state: &std::sync::Arc<AppState>) -> Vec<MfyCard> {
    let db = &state.db;
    let now = super::now_secs();
    let all = load_all(db);
    let signals = super::taste::load_signals(db).len();
    if all.is_empty() && signals > 0 {
        refresh(state, false).await;
        return load_all(db).iter().map(Mfy::card).collect();
    }
    if all.iter().any(|m| is_stale(m, now, signals) || needs_new_cover(m)) {
        let st = state.clone();
        crate::rt::spawn(async move {
            // ── stream: reliability ── nobody waits on a stale-set rebuild: paced, low priority.
            innertube::background(refresh(&st, false)).await;
        });
    }
    let mut cards: Vec<MfyCard> = all.iter().map(Mfy::card).collect();
    // Your Rotation's live count.
    for c in &mut cards {
        if c.kind == Kind::OnRepeat {
            c.track_count = db
                .top_plays(now - crate::state::ON_REPEAT_WINDOW_SECS, crate::state::ON_REPEAT_LIMIT)
                .len();
        }
    }
    cards
}

/// Home's "Made for you" shelf from what is stored (no network on the Home path).
pub fn shelf(db: &Db) -> Option<Shelf> {
    let items: Vec<ShelfItem> = load_all(db).iter().map(Mfy::shelf_item).collect();
    (!items.is_empty()).then(|| Shelf {
        id: "made_for_you".into(),
        title: "Made for you".into(),
        subtitle: None,
        layout: "cards".into(),
        source: "local".into(),
        items,
    })
}

/// The playlist page of a Made for you id (`get_playlist`).
pub fn page(db: &Db, id: &str) -> Option<innertube::PlaylistPage> {
    let m = load(db, id)?;
    let tracks = m.tracks.clone();
    Some(innertube::PlaylistPage {
        title: Some(m.title.clone()),
        subtitle: None,
        thumbnail: cover_url(m.cover.as_deref()),
        description: Some(m.description.clone()),
        privacy: None,
        // The page header draws `cover` before anything else (no mosaic of the tracks).
        cover: cover_url(m.cover.as_deref()),
        items: tracks,
        continuation: None,
        owned: false,
        collaborative: false,
        sort_menu: None,
        suggestions: None,
    })
}

/// Background schedule: 30 s after launch, then hourly.
pub fn spawn() {
    crate::rt::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            if let Some(st) = crate::state::global() {
                // ── stream: reliability ── the scheduled rebuild is background work.
                innertube::background(async {
                    super::whatsnew::refresh(&st, false).await;
                    refresh(&st, false).await;
                })
                .await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}

// --- covers --------------------------------------------------------------------------------------

/// Figtree at weight 800 (OFL), outlines as SVG paths in a 1000-unit em, baseline at y 0.
/// Generated with fontTools from the bundled `@fontsource-variable/figtree`. An SVG shown through
/// `<img>` can't load web fonts, so the covers draw these paths instead of `<text>`.
const GLYPHS: &[(char, u16, &str)] = &[
    (' ', 234, ""),
    ('!', 319, "M85 -239 73 -700H246L234 -239ZM160 16Q122 16 95 -11Q68 -39 68 -76Q68 -113 95 -140Q122 -168 160 -168Q197 -168 224 -140Q251 -113 251 -76Q251 -39 224 -11Q197 16 160 16Z"),
    ('#', 632, "M312 0 422 -700H534L424 0ZM10 -175 26 -284H588L572 -175ZM102 0 212 -700H324L214 0ZM40 -390 60 -500H622L602 -390Z"),
    ('&', 649, "M251 13Q189 13 140 -12Q91 -36 64 -78Q36 -121 36 -177Q36 -211 49 -242Q62 -272 85 -299Q108 -326 138 -347L288 -450Q300 -458 316 -470Q332 -482 345 -500Q359 -518 359 -541Q359 -565 340 -578Q321 -591 294 -591Q266 -591 248 -578Q231 -564 231 -541Q231 -517 246 -499Q261 -482 278 -459L416 -293L659 0H481L309 -205L160 -386Q130 -424 113 -451Q96 -478 89 -500Q82 -522 82 -543Q82 -592 109 -630Q137 -669 183 -691Q229 -713 286 -713Q349 -713 398 -690Q447 -668 474 -629Q502 -589 502 -537Q502 -495 481 -460Q460 -425 417 -389Q375 -352 310 -308L240 -259Q227 -250 217 -238Q208 -226 202 -212Q196 -197 196 -181Q196 -153 214 -135Q233 -117 263 -117Q300 -117 339 -128Q378 -140 411 -162Q444 -185 464 -219Q483 -253 483 -299H626Q626 -209 578 -138Q530 -67 446 -27Q361 13 251 13Z"),
    ('\'', 266, "M77 -450 60 -716H206L189 -450Z"),
    ('(', 396, "M210 100Q164 39 126 -32Q88 -104 66 -184Q44 -264 44 -350Q44 -437 66 -516Q88 -596 126 -668Q164 -739 210 -800H362Q316 -736 278 -663Q240 -590 218 -512Q196 -433 196 -350Q196 -268 218 -189Q240 -110 278 -38Q316 35 362 100Z"),
    (')', 396, "M186 100H34Q81 35 119 -38Q156 -110 178 -189Q200 -268 200 -350Q200 -433 178 -512Q156 -590 119 -663Q81 -736 34 -800H186Q233 -739 270 -668Q308 -596 330 -516Q352 -437 352 -350Q352 -264 330 -184Q308 -104 270 -32Q233 39 186 100Z"),
    ('+', 636, "M256 -65V-531H380V-65ZM85 -236V-360H551V-236Z"),
    (',', 272, "M23 114 87 -134H245L139 114Z"),
    ('-', 404, "M52 -232V-364H352V-232Z"),
    ('.', 267, "M134 15Q96 15 69 -12Q42 -39 42 -77Q42 -114 69 -141Q96 -168 134 -168Q171 -168 198 -141Q225 -114 225 -77Q225 -39 198 -12Q171 15 134 15Z"),
    ('/', 452, "M15 84 291 -747H437L163 84Z"),
    ('0', 652, "M325 12Q239 12 172 -34Q105 -81 68 -162Q30 -244 30 -350Q30 -456 68 -538Q105 -619 171 -666Q238 -712 324 -712Q411 -712 478 -666Q545 -619 583 -538Q622 -456 622 -350Q622 -244 584 -162Q546 -81 479 -34Q412 12 325 12ZM326 -134Q366 -134 397 -162Q429 -190 447 -238Q465 -287 465 -350Q465 -414 447 -463Q429 -512 397 -540Q365 -568 324 -568Q284 -568 253 -540Q222 -512 204 -463Q187 -414 187 -350Q187 -287 204 -238Q222 -189 254 -162Q285 -134 326 -134Z"),
    ('1', 426, "M185 0V-500Q152 -479 116 -468Q79 -457 41 -460V-606Q68 -604 97 -618Q125 -632 153 -655Q181 -678 203 -700H338V0Z"),
    ('2', 588, "M48 0V-116Q48 -143 68 -179Q88 -214 123 -251Q159 -288 204 -319L292 -379Q312 -392 332 -410Q352 -427 365 -448Q378 -468 378 -489Q378 -511 368 -528Q357 -546 336 -555Q316 -565 283 -565Q258 -565 239 -555Q219 -544 206 -527Q194 -509 188 -488L22 -516Q32 -568 65 -613Q99 -658 155 -685Q210 -713 282 -713Q364 -713 422 -684Q480 -656 512 -606Q543 -556 543 -490Q543 -420 504 -369Q466 -318 403 -275L326 -222Q302 -206 282 -186Q262 -166 247 -141H551V0Z"),
    ('3', 561, "M286 12Q220 12 166 -8Q111 -28 76 -67Q40 -105 29 -161L181 -198Q187 -186 197 -169Q206 -152 225 -139Q244 -127 276 -127Q318 -127 342 -149Q366 -172 366 -209Q366 -231 355 -251Q345 -270 318 -281Q290 -293 240 -293H188V-423H240Q303 -423 329 -444Q355 -465 355 -501Q355 -522 345 -538Q335 -554 317 -564Q299 -573 276 -573Q259 -573 241 -567Q223 -561 208 -546Q193 -531 185 -503L35 -539Q46 -596 84 -635Q121 -673 173 -693Q225 -712 280 -712Q346 -712 399 -688Q453 -663 484 -619Q516 -576 516 -518Q516 -462 484 -423Q453 -384 404 -361Q460 -333 494 -289Q527 -245 527 -188Q527 -124 493 -79Q459 -34 404 -11Q349 12 286 12Z"),
    ('4', 639, "M345 0V-123H18V-252L324 -700H497V-260H609V-123H497V0ZM179 -260H345V-503Z"),
    ('5', 584, "M295 12Q243 12 190 -6Q137 -24 95 -58Q54 -92 37 -142L189 -178Q204 -152 230 -140Q255 -127 284 -127Q328 -127 356 -154Q383 -180 383 -225Q383 -266 356 -294Q330 -323 284 -323Q256 -323 235 -310Q213 -297 203 -279L58 -311L104 -700H510V-554H240L220 -411Q246 -426 279 -434Q312 -443 351 -443Q405 -442 450 -415Q496 -388 523 -341Q550 -295 550 -234Q550 -161 517 -105Q484 -50 427 -19Q370 12 295 12Z"),
    ('6', 586, "M299 12Q229 12 179 -13Q129 -38 97 -82Q65 -126 49 -184Q34 -243 34 -311Q34 -387 50 -459Q65 -531 100 -588Q134 -645 190 -678Q247 -712 328 -712Q385 -712 438 -696Q491 -680 534 -650L456 -532Q425 -553 390 -562Q355 -572 328 -572Q262 -572 222 -512Q182 -451 183 -318L151 -337Q174 -389 220 -421Q266 -453 329 -453Q396 -453 447 -421Q498 -389 527 -335Q556 -281 556 -214Q556 -149 525 -98Q493 -47 436 -17Q378 12 299 12ZM300 -123Q327 -123 349 -136Q371 -149 384 -171Q396 -194 396 -222Q396 -251 384 -274Q371 -297 349 -310Q327 -324 300 -324Q272 -324 250 -310Q227 -297 214 -274Q201 -251 201 -222Q201 -194 214 -171Q227 -149 250 -136Q272 -123 300 -123Z"),
    ('7', 564, "M99 0 356 -552H29V-700H534V-584L277 0Z"),
    ('8', 611, "M304 12Q217 12 157 -12Q97 -37 66 -80Q36 -124 36 -182Q36 -245 66 -293Q96 -340 156 -367Q108 -391 83 -430Q57 -470 57 -522Q57 -578 86 -621Q115 -664 170 -688Q225 -712 305 -712Q384 -712 439 -688Q495 -664 524 -621Q554 -578 554 -522Q554 -470 528 -430Q503 -391 454 -367Q513 -341 544 -293Q575 -245 575 -182Q575 -124 544 -80Q513 -37 453 -12Q394 12 304 12ZM305 -121Q351 -121 381 -145Q410 -168 410 -204Q410 -246 381 -272Q351 -297 305 -297Q275 -297 251 -286Q228 -274 214 -253Q201 -232 201 -204Q201 -180 214 -161Q228 -143 251 -132Q275 -121 305 -121ZM305 -420Q345 -420 371 -444Q397 -467 397 -503Q397 -539 371 -562Q345 -584 305 -584Q265 -584 239 -562Q214 -539 214 -503Q214 -467 239 -444Q265 -420 305 -420Z"),
    ('9', 586, "M288 -711Q358 -711 408 -686Q458 -662 490 -618Q522 -574 537 -515Q553 -457 553 -388Q553 -312 537 -240Q521 -169 487 -111Q453 -54 396 -21Q340 13 259 13Q202 13 149 -3Q96 -19 53 -49L130 -167Q161 -146 196 -137Q231 -128 258 -128Q324 -128 365 -188Q405 -248 403 -381L436 -362Q412 -311 367 -278Q321 -246 257 -246Q191 -246 140 -278Q88 -310 59 -365Q30 -419 30 -485Q30 -551 61 -602Q93 -653 151 -682Q209 -711 288 -711ZM287 -576Q259 -576 237 -563Q215 -550 203 -528Q190 -506 190 -477Q190 -449 203 -426Q215 -402 237 -389Q259 -375 287 -375Q315 -375 337 -389Q359 -402 372 -426Q385 -449 385 -477Q385 -506 372 -528Q359 -550 337 -563Q315 -576 287 -576Z"),
    (':', 324, "M162 -317Q124 -317 97 -344Q70 -371 70 -409Q70 -446 97 -473Q124 -500 162 -500Q199 -500 226 -473Q253 -446 253 -409Q253 -371 226 -344Q199 -317 162 -317ZM162 1Q125 1 97 -26Q70 -53 70 -90Q70 -128 97 -155Q125 -182 162 -182Q199 -182 226 -155Q254 -128 254 -90Q254 -53 226 -26Q199 1 162 1Z"),
    ('?', 518, "M157 -207 151 -382 234 -389Q281 -393 310 -416Q339 -438 339 -484Q339 -523 312 -548Q285 -572 242 -572Q212 -572 190 -557Q168 -542 159 -519L19 -557Q39 -628 97 -670Q156 -712 242 -712Q314 -712 369 -683Q424 -654 456 -603Q488 -551 488 -484Q488 -433 465 -392Q443 -352 402 -326Q361 -300 306 -292L304 -207ZM230 15Q193 15 166 -12Q139 -39 139 -77Q139 -114 166 -141Q193 -168 230 -168Q268 -168 295 -141Q322 -114 322 -77Q322 -39 295 -12Q268 15 230 15Z"),
    ('A', 732, "M8 0 280 -700H452L724 0H554L503 -133H229L178 0ZM281 -275H450L366 -499Z"),
    ('B', 613, "M68 0V-700H313Q385 -700 440 -678Q495 -656 526 -611Q557 -566 557 -497Q557 -460 538 -428Q518 -395 484 -369Q532 -344 557 -303Q583 -263 583 -204Q583 -138 553 -93Q523 -47 470 -24Q417 0 346 0ZM225 -142H328Q369 -142 393 -163Q417 -185 417 -219Q417 -255 392 -275Q367 -296 323 -296H225ZM225 -425H308Q346 -425 369 -445Q392 -464 392 -499Q392 -535 368 -555Q343 -574 302 -574H225Z"),
    ('C', 727, "M384 12Q280 12 201 -34Q121 -81 76 -162Q30 -244 30 -350Q30 -456 75 -538Q121 -619 200 -666Q280 -712 384 -712Q460 -712 523 -685Q587 -659 634 -611Q680 -563 705 -500L554 -449Q539 -484 514 -510Q488 -536 456 -550Q423 -564 384 -564Q329 -564 286 -537Q243 -510 219 -462Q195 -413 195 -350Q195 -287 219 -239Q244 -190 287 -163Q331 -135 387 -135Q428 -135 460 -150Q492 -165 516 -192Q539 -219 556 -254L707 -203Q683 -139 636 -91Q589 -42 525 -15Q461 12 384 12Z"),
    ('D', 705, "M68 0V-700H297Q408 -700 493 -655Q579 -610 627 -532Q675 -453 675 -350Q675 -248 627 -169Q579 -90 493 -45Q408 0 297 0ZM229 -152H297Q343 -152 382 -166Q421 -181 450 -208Q479 -234 495 -271Q511 -307 511 -350Q511 -393 495 -429Q479 -466 450 -492Q421 -519 382 -534Q343 -548 297 -548H229Z"),
    ('E', 587, "M68 0V-700H524V-552H229V-424H476V-281H229V-148H529V0Z"),
    ('F', 559, "M68 0V-700H532V-552H229V-417H474V-270H229V0Z"),
    ('G', 735, "M397 12Q289 12 207 -34Q124 -81 77 -162Q30 -244 30 -350Q30 -456 77 -538Q124 -619 207 -666Q289 -712 397 -712Q464 -712 519 -692Q573 -673 614 -639Q655 -605 682 -561L549 -481Q534 -505 509 -524Q485 -543 456 -554Q427 -564 397 -564Q338 -564 292 -537Q247 -510 221 -462Q195 -413 195 -350Q195 -287 220 -238Q245 -190 292 -162Q339 -134 401 -134Q444 -134 479 -150Q514 -166 535 -194Q556 -222 560 -260H402V-384H715V-289Q713 -191 671 -124Q629 -56 559 -22Q488 12 397 12Z"),
    ('H', 749, "M68 0V-701H229V-431H520V-700H681V0H520V-283H229V0Z"),
    ('I', 297, "M68 0V-700H229V0Z"),
    ('J', 570, "M247 15Q189 15 145 -4Q102 -23 72 -54Q43 -84 28 -118Q12 -151 9 -180L149 -227Q162 -181 187 -157Q212 -133 249 -133Q277 -133 300 -145Q323 -158 337 -184Q351 -211 351 -252V-700H512V-210Q512 -159 493 -117Q474 -76 439 -46Q404 -17 355 -1Q306 15 247 15Z"),
    ('K', 692, "M68 0V-700H228V-445L463 -700H673L380 -384L683 0H484L274 -270L228 -221V0Z"),
    ('L', 545, "M68 0V-700H229V-148H527V0Z"),
    ('M', 863, "M68 0V-700H245L431 -424L618 -700H795V0H634V-450L432 -154L229 -450V0Z"),
    ('N', 773, "M68 0V-700H237L544 -273V-700H705V0H544L229 -439V0Z"),
    ('O', 792, "M398 12Q290 12 207 -34Q124 -81 77 -162Q30 -244 30 -349Q30 -455 77 -537Q124 -618 206 -665Q289 -711 396 -711Q503 -711 586 -665Q668 -618 715 -537Q762 -455 762 -349Q762 -244 715 -162Q669 -81 587 -34Q505 12 398 12ZM398 -136Q457 -136 502 -163Q547 -190 572 -239Q597 -287 597 -349Q597 -412 571 -460Q545 -509 500 -536Q455 -564 396 -564Q337 -564 292 -536Q247 -509 221 -460Q195 -412 195 -349Q195 -287 221 -238Q247 -190 293 -163Q339 -136 398 -136Z"),
    ('P', 614, "M68 0V-700H343Q419 -700 478 -670Q537 -639 571 -586Q604 -532 604 -461Q604 -392 571 -337Q539 -283 481 -252Q423 -221 348 -221H229V0ZM229 -369H344Q387 -369 413 -395Q439 -420 439 -461Q439 -502 412 -528Q384 -553 339 -553H229Z"),
    ('Q', 793, "M650 43 578 -31Q539 -10 494 1Q448 12 398 12Q290 12 207 -34Q124 -81 77 -162Q30 -244 30 -349Q30 -429 57 -495Q84 -561 133 -610Q182 -659 249 -686Q315 -712 396 -712Q503 -712 586 -666Q668 -619 715 -537Q762 -455 762 -349Q762 -282 743 -223Q723 -165 687 -119L750 -55ZM398 -136Q416 -136 431 -139Q446 -143 461 -149L388 -223L491 -320L570 -239Q583 -262 590 -290Q597 -317 597 -349Q597 -413 571 -461Q546 -510 500 -537Q455 -564 396 -564Q337 -564 292 -537Q246 -510 221 -461Q195 -413 195 -349Q195 -287 221 -238Q247 -190 293 -163Q339 -136 398 -136Z"),
    ('R', 654, "M68 0V-700H343Q419 -700 478 -670Q537 -641 571 -589Q604 -536 604 -468Q604 -399 569 -346Q533 -292 474 -264L632 0H448L314 -235Q308 -235 302 -235Q295 -235 289 -235H229V0ZM229 -383H338Q366 -383 388 -394Q410 -405 423 -424Q435 -444 435 -468Q435 -505 408 -529Q380 -553 335 -553H229Z"),
    ('S', 603, "M308 12Q257 12 210 -1Q164 -15 127 -38Q89 -62 65 -92Q41 -121 35 -155L194 -200Q205 -171 232 -149Q260 -128 302 -127Q343 -126 371 -146Q398 -166 398 -198Q398 -223 379 -242Q359 -261 323 -271L224 -296Q172 -309 133 -337Q95 -365 73 -405Q52 -446 52 -498Q52 -599 119 -656Q186 -712 308 -712Q375 -712 425 -694Q475 -675 511 -639Q546 -603 567 -549L409 -502Q401 -529 374 -551Q348 -573 306 -573Q264 -573 240 -554Q217 -535 217 -499Q217 -474 236 -458Q255 -441 288 -433L387 -408Q472 -387 519 -329Q567 -272 567 -199Q567 -133 536 -86Q506 -39 448 -13Q390 12 308 12Z"),
    ('T', 599, "M220 0V-552H16V-700H584V-552H380V0Z"),
    ('U', 702, "M349 13Q259 13 194 -21Q128 -55 93 -118Q58 -180 58 -266V-700H219V-266Q219 -226 235 -197Q251 -168 281 -152Q310 -136 351 -136Q391 -136 421 -152Q451 -168 467 -197Q483 -226 483 -266V-700H644V-266Q644 -180 608 -118Q573 -55 506 -21Q440 13 349 13Z"),
    ('V', 736, "M282 0 8 -700H181L369 -193L555 -700H729L456 0Z"),
    ('W', 996, "M212 0 8 -700H172L298 -248L418 -700H578L700 -256L825 -700H989L784 0H616L498 -450L379 0Z"),
    ('X', 709, "M10 0 265 -360 16 -700H208L355 -489L501 -700H693L445 -360L699 0H505L355 -227L204 0Z"),
    ('Y', 666, "M253 0V-296L6 -700H189L333 -457L477 -700H660L414 -297V0Z"),
    ('Z', 628, "M34 0V-152L385 -548H37V-700H594V-548L243 -152H591V0Z"),
    ('a', 540, "M212 12Q123 12 74 -28Q25 -68 25 -141Q25 -220 76 -259Q127 -298 219 -298H341Q336 -338 318 -360Q299 -382 258 -382Q230 -382 208 -370Q188 -356 173 -334L36 -379Q54 -416 83 -446Q112 -476 156 -494Q198 -512 256 -512Q334 -512 388 -483Q442 -454 469 -402Q496 -350 496 -279L494 0H348L346 -51Q326 -20 292 -4Q258 12 212 12ZM246 -106Q274 -106 296 -118Q318 -130 331 -150Q344 -171 344 -195V-198H278Q228 -198 206 -184Q184 -170 184 -145Q184 -126 200 -116Q216 -106 246 -106Z"),
    ('b', 600, "M349 12Q301 12 265 -8Q229 -27 205 -64L199 0H51V-700H207V-440Q231 -475 267 -494Q302 -512 349 -512Q419 -512 470 -480Q521 -449 548 -390Q575 -332 575 -250Q575 -169 548 -110Q521 -51 470 -20Q419 12 349 12ZM309 -132Q339 -132 362 -147Q385 -162 399 -189Q413 -215 413 -250Q413 -285 400 -311Q387 -338 363 -353Q340 -368 310 -368Q280 -368 256 -353Q233 -338 220 -311Q207 -284 207 -250Q207 -215 220 -189Q233 -162 256 -147Q279 -132 309 -132Z"),
    ('c', 548, "M300 12Q220 12 158 -22Q95 -55 60 -114Q25 -174 25 -250Q25 -327 60 -386Q95 -445 157 -478Q218 -512 298 -512Q375 -512 439 -473Q503 -434 531 -360L383 -310Q372 -334 347 -350Q321 -365 290 -365Q259 -365 235 -351Q212 -336 198 -310Q185 -284 185 -250Q185 -216 199 -190Q212 -165 237 -150Q261 -135 292 -135Q323 -135 348 -151Q373 -167 385 -193L533 -143Q504 -68 441 -28Q377 12 300 12Z"),
    ('d', 600, "M251 12Q181 12 130 -20Q80 -51 52 -110Q25 -169 25 -250Q25 -332 52 -390Q80 -449 130 -480Q181 -512 251 -512Q298 -512 334 -494Q369 -475 393 -441V-700H549V0H400L396 -63Q373 -27 336 -8Q299 12 251 12ZM292 -132Q320 -132 342 -145Q363 -158 377 -182Q390 -207 393 -237V-263Q390 -293 377 -317Q363 -341 341 -354Q319 -368 291 -368Q261 -368 238 -353Q214 -338 201 -311Q187 -285 187 -250Q187 -215 201 -189Q215 -162 239 -147Q262 -132 292 -132Z"),
    ('e', 560, "M290 12Q213 12 153 -21Q93 -54 59 -114Q25 -172 25 -250Q25 -328 59 -386Q94 -446 154 -479Q214 -512 292 -512Q364 -512 421 -478Q478 -444 512 -377Q545 -310 545 -210H187Q192 -166 222 -141Q252 -116 295 -116Q332 -116 357 -132Q382 -148 392 -174L530 -126Q511 -80 474 -48Q438 -18 391 -3Q344 12 290 12ZM191 -314H382Q378 -344 363 -361Q348 -378 328 -386Q308 -394 288 -394Q268 -394 248 -386Q227 -379 211 -362Q196 -344 191 -314Z"),
    ('f', 404, "M101 0V-377H22V-500H101V-546Q101 -628 144 -675Q187 -722 276 -722Q309 -722 345 -710Q381 -699 407 -684L349 -574Q337 -582 325 -586Q313 -589 303 -589Q279 -589 268 -573Q257 -556 257 -518V-500H370V-377H257V0Z"),
    ('g', 608, "M287 214Q226 214 180 206Q135 198 103 185Q71 172 50 156L111 32Q127 39 149 50Q171 60 203 68Q235 76 280 76Q315 76 342 62Q370 49 385 20Q401 -8 401 -51V-97Q377 -60 338 -40Q300 -19 251 -19Q181 -19 130 -48Q80 -76 52 -129Q25 -182 25 -255Q25 -335 52 -392Q80 -449 130 -481Q181 -512 251 -512Q300 -512 338 -490Q377 -467 401 -425V-500H557V-57Q557 27 525 88Q493 149 433 181Q373 214 287 214ZM297 -149Q327 -149 348 -163Q370 -178 382 -203Q393 -228 393 -260Q393 -294 381 -319Q369 -344 348 -358Q326 -372 296 -372Q267 -372 244 -358Q222 -344 209 -319Q197 -294 196 -260Q197 -228 210 -203Q222 -178 245 -163Q268 -149 297 -149Z"),
    ('h', 575, "M53 0V-700H209V-440Q234 -476 269 -494Q304 -512 347 -512Q408 -512 449 -488Q490 -463 510 -413Q531 -363 531 -285V0H375V-256Q375 -296 366 -319Q357 -343 340 -354Q323 -365 300 -366Q255 -367 232 -342Q209 -316 209 -264V0Z"),
    ('i', 276, "M60 0V-500H216V0ZM138 -564Q103 -564 76 -591Q50 -617 50 -652Q50 -689 76 -714Q103 -740 138 -740Q174 -740 200 -714Q226 -689 226 -652Q226 -617 200 -591Q174 -564 138 -564Z"),
    ('j', 301, "M160 -560Q124 -560 97 -587Q70 -614 70 -650Q70 -687 97 -714Q124 -741 160 -741Q197 -741 224 -714Q251 -687 251 -650Q251 -614 224 -587Q197 -560 160 -560ZM49 215Q-16 215 -66 182L-2 53Q10 60 22 65Q34 70 44 70Q82 70 82 18V-500H238V29Q238 117 189 166Q140 215 49 215Z"),
    ('k', 562, "M51 0V-700H207V-332L358 -500H541L352 -284L554 0H373L253 -171L207 -120V0Z"),
    ('l', 258, "M51 0V-700H207V0Z"),
    ('m', 881, "M51 0V-500H198L204 -437Q228 -474 263 -493Q299 -512 342 -512Q404 -512 444 -487Q485 -461 504 -409Q527 -459 567 -486Q607 -512 657 -512Q747 -512 792 -457Q838 -402 837 -286V0H681V-256Q681 -296 673 -319Q664 -343 648 -354Q632 -365 610 -366Q567 -367 544 -342Q522 -316 522 -264V0H366V-256Q366 -296 358 -319Q349 -343 333 -354Q317 -365 295 -366Q252 -367 229 -342Q207 -316 207 -264V0Z"),
    ('n', 573, "M51 0V-500H198L204 -436Q229 -472 265 -492Q301 -512 346 -512Q406 -512 447 -488Q488 -463 509 -413Q529 -363 529 -285V0H373V-256Q373 -296 364 -319Q355 -343 338 -354Q322 -365 298 -366Q253 -367 230 -342Q207 -316 207 -264V0Z"),
    ('o', 582, "M293 12Q214 12 154 -22Q94 -55 59 -114Q25 -173 25 -250Q25 -327 59 -386Q92 -445 153 -478Q213 -512 290 -512Q369 -512 429 -478Q490 -445 523 -386Q557 -327 557 -250Q557 -173 523 -114Q490 -55 430 -22Q370 12 293 12ZM293 -132Q323 -132 347 -147Q370 -162 384 -189Q397 -215 397 -250Q397 -285 384 -311Q370 -338 346 -353Q322 -368 291 -368Q260 -368 236 -353Q212 -338 199 -311Q185 -285 185 -250Q185 -215 199 -189Q212 -162 237 -147Q261 -132 293 -132Z"),
    ('p', 605, "M56 200V-500H203L209 -436Q233 -473 270 -492Q306 -512 354 -512Q424 -512 475 -480Q526 -449 553 -390Q580 -332 580 -250Q580 -169 553 -110Q526 -51 475 -20Q424 12 354 12Q307 12 272 -7Q236 -25 212 -60V200ZM314 -132Q344 -132 367 -147Q390 -162 404 -189Q418 -215 418 -250Q418 -285 405 -311Q392 -338 368 -353Q345 -368 315 -368Q285 -368 261 -353Q238 -338 225 -311Q212 -284 212 -250Q212 -215 225 -189Q238 -162 261 -147Q284 -132 314 -132Z"),
    ('q', 598, "M393 200V-60Q370 -25 334 -7Q298 12 251 12Q181 12 130 -20Q80 -51 52 -110Q25 -169 25 -250Q25 -332 52 -390Q80 -449 130 -480Q181 -512 251 -512Q300 -512 337 -492Q373 -472 397 -435L402 -500H549V200ZM292 -132Q321 -132 344 -147Q367 -162 380 -189Q393 -215 393 -250Q393 -285 380 -311Q367 -338 344 -353Q321 -368 291 -368Q261 -368 238 -353Q215 -338 201 -311Q188 -285 187 -250Q188 -215 202 -189Q215 -162 239 -147Q262 -132 292 -132Z"),
    ('r', 398, "M51 0V-500H207V-407Q238 -454 286 -478Q334 -502 384 -502V-350Q338 -350 297 -340Q257 -330 232 -310Q207 -289 207 -257V0Z"),
    ('s', 479, "M239 12Q190 12 148 -2Q105 -16 74 -43Q42 -70 28 -108L156 -162Q168 -136 192 -124Q214 -111 239 -111Q264 -111 278 -119Q292 -128 292 -143Q292 -159 277 -168Q263 -177 236 -184L192 -194Q123 -212 85 -254Q48 -296 48 -351Q48 -428 100 -470Q154 -512 248 -512Q300 -512 341 -498Q382 -484 409 -458Q436 -432 449 -399L325 -348Q318 -368 296 -378Q274 -389 249 -389Q227 -389 215 -379Q203 -370 203 -354Q203 -342 217 -333Q232 -324 260 -318L312 -304Q360 -293 392 -268Q424 -244 440 -211Q456 -178 456 -142Q456 -94 428 -60Q400 -26 352 -7Q303 12 239 12Z"),
    ('t', 414, "M266 12Q177 12 134 -30Q90 -73 90 -152V-369H12V-500H90V-656H246V-500H360V-369H246V-191Q246 -161 257 -147Q268 -133 291 -133Q300 -133 312 -137Q324 -142 334 -149L388 -35Q364 -15 331 -2Q297 12 266 12Z"),
    ('u', 573, "M227 12Q167 12 126 -12Q85 -37 65 -87Q44 -137 44 -215V-500H200V-244Q200 -205 209 -181Q218 -157 235 -146Q252 -135 275 -134Q320 -134 343 -159Q366 -185 366 -236V-500H522V0H375L369 -64Q345 -28 308 -8Q272 12 227 12Z"),
    ('v', 573, "M209 0 6 -500H167L287 -178L406 -500H567L364 0Z"),
    ('w', 859, "M180 0 10 -500H169L261 -208L353 -500H507L596 -215L690 -500H849L678 0H525L429 -313L333 0Z"),
    ('x', 564, "M10 0 197 -259 23 -500H207L283 -386L358 -500H542L369 -258L554 0H369L283 -132L196 0Z"),
    ('y', 599, "M177 212Q145 212 110 200Q74 188 50 174L108 47Q122 57 136 62Q149 67 160 67Q179 67 194 58Q209 48 218 27L232 -8L2 -500H175L303 -183L424 -500H597L376 30Q354 84 330 125Q306 166 271 189Q235 212 177 212Z"),
    ('z', 471, "M26 0V-110L250 -373H26V-500H444V-390L219 -126H445V0Z"),
];

/// Cover backgrounds (by playlist number), shown when there is no artwork.
const BANDS: [&str; 10] = [
    "#3b3f8f", "#7a3b69", "#2f6f73", "#8a5a2b", "#4b6b3a", "#5a4a8a", "#8c3f3f", "#2f5d8a",
    "#6b5b3e", "#3f6f5a",
];

/// Our mark, top-left on every cover: the disc and note of the Spotless master
/// (assets/brand/spotless.svg), 48 px. The next line is rewritten by scripts/brand/gen-brand.py ship.
const SPOTLESS_MARK: &str = r##"<g transform="translate(22.00 22.00) scale(0.6000)"><circle cx="50" cy="50" r="40" fill="#FF2D7A"/><path fill="#000000" d="M43.70 62.08A9.520 6.944 -22.0 0 1 37.48 72.09A9.520 6.944 -22.0 0 1 26.05 69.22A9.520 6.944 -22.0 0 1 32.27 59.21A9.520 6.944 -22.0 0 1 43.70 62.08ZM66.66 55.92A9.520 6.944 -22.0 0 1 60.44 65.93A9.520 6.944 -22.0 0 1 49.01 63.06A9.520 6.944 -22.0 0 1 55.23 53.05A9.520 6.944 -22.0 0 1 66.66 55.92ZM39.71 32.61L44.08 32.61L44.08 64.05L39.71 64.72ZM62.67 26.45L67.04 26.45L67.04 57.89L62.67 58.56ZM39.71 28.13L67.04 20.80L67.04 29.76L39.71 37.09Z"/></g>"##; // gen-brand

fn glyph(c: char) -> (u16, &'static str) {
    GLYPHS.iter().find(|g| g.0 == c).map(|g| (g.1, g.2)).unwrap_or((234, ""))
}

/// Width of `s` at `size` px.
fn text_width(s: &str, size: f64) -> f64 {
    s.chars().map(|c| glyph(c).0 as f64).sum::<f64>() * size / 1000.0
}

/// The largest size up to `max` at which `s` fits `width`.
fn fit(s: &str, width: f64, max: f64) -> f64 {
    let w1 = text_width(s, 1000.0);
    if w1 <= 0.0 { max } else { (width * 1000.0 / w1).min(max) }
}

/// `s` as outline paths, left edge at `x`, baseline at `y`.
fn text_paths(s: &str, x: f64, y: f64, size: f64, fill: &str) -> String {
    let k = size / 1000.0;
    let mut out = format!(
        r#"<g fill="{fill}" transform="translate({x:.1} {y:.1}) scale({k:.4})">"#
    );
    let mut ax = 0u32;
    for c in s.chars() {
        let (adv, d) = glyph(c);
        if !d.is_empty() {
            out.push_str(&format!(r#"<path transform="translate({ax} 0)" d="{d}"/>"#));
        }
        ax += adv as u32;
    }
    out.push_str("</g>");
    out
}

/// Full-bleed artwork (or a band-colour gradient), with an optional bottom shade.
fn backdrop(image: Option<&str>, band: &str, shade: bool) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    match image {
        Some(src) => format!(
            r#"<rect width="640" height="640" fill="{band}"/><image href="{}" x="0" y="0" width="640" height="640" preserveAspectRatio="xMidYMid slice"/>{}"#,
            esc(src),
            if shade { r#"<rect width="640" height="640" fill="url(#shade)"/>"# } else { "" }
        ),
        None => format!(
            r#"<rect width="640" height="640" fill="{band}"/><rect width="640" height="640" fill="url(#glow)"/>"#
        ),
    }
}

/// The cover SVG (pure). `image` is an embeddable `data:` URL. One layout for every playlist:
/// artwork (or the band colour), our mark top-left, the name bottom-left as Figtree paths.
pub fn cover_svg(label: &str, image: Option<&str>, n: usize) -> String {
    let band = BANDS[n % BANDS.len()];
    let title_esc =
        label.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut body = backdrop(image, band, true);
    body.push_str(SPOTLESS_MARK);
    body.push_str(&text_paths(label, 28.0, 600.0, fit(label, 584.0, 64.0), "#fff"));
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 640" width="640" height="640"><title>{title_esc}</title><defs><linearGradient id="shade" x1="0" y1="0" x2="0" y2="1"><stop offset="0.5" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity="0.45"/></linearGradient><radialGradient id="glow" cx="0.75" cy="0.2" r="0.9"><stop offset="0" stop-color="#fff" stop-opacity="0.28"/><stop offset="1" stop-color="#000" stop-opacity="0.25"/></radialGradient></defs>{body}</svg>"##
    )
}

struct Covers<'a> {
    state: &'a AppState,
    http: reqwest::Client,
}

impl<'a> Covers<'a> {
    fn new(state: &'a AppState) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_default();
        Self { state, http }
    }

    async fn data_url(&self, url: &str) -> Option<String> {
        use base64::Engine;
        let url = crate::smart::mfy::sized(url, 640);
        let resp = self.http.get(&url).send().await.ok()?.error_for_status().ok()?;
        let mime = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .filter(|m| m.starts_with("image/"))
            .unwrap_or("image/jpeg")
            .to_owned();
        let bytes = resp.bytes().await.ok()?;
        if bytes.len() > 2_000_000 {
            return None;
        }
        Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ))
    }

    /// Write the cover and return its path (`None`: the artwork URL itself, when writing fails).
    async fn make(&mut self, id: &str, label: &str, art: Option<&str>, n: usize) -> Option<String> {
        let img = match art {
            Some(a) if a.starts_with("http") => self.data_url(a).await,
            // Already embeddable (a redraw reusing the old cover's image).
            Some(a) if a.starts_with("data:image/") => Some(a.to_owned()),
            _ => None,
        };
        let svg = cover_svg(label, img.as_deref(), n);
        let dir = crate::local::covers_dir(&*self.state.host).join("mfy");
        let Ok(path) = write_cover(&dir, id, &svg) else {
            return art.map(str::to_owned);
        };
        let p = path.to_string_lossy().to_string();
        self.state.host.allow_asset_file(std::path::Path::new(&p));
        Some(p)
    }
}

// --- cover files ---------------------------------------------------------------------------------
//
// One file per playlist under a stable name (`MFY_DAILY_1.svg`), overwritten in place on every
// rebuild or redraw. Anything that kept the URL — Home pre-painted from `localStorage`, the page
// cache, a library row — still finds the file. The UI gets the path plus `?v=<mtime>` so the
// webview doesn't serve a redrawn cover from its image cache (the asset protocol reads only the
// URI path, so the query never reaches the file system). Before this, files were named
// `<id>-<unix secs>.svg` and the old one deleted on each redraw, which left every cached URL
// pointing at a missing file.

/// The file-name stem of a playlist's cover (its id, reduced to `[A-Za-z0-9_]`).
fn cover_stem(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect()
}

/// A playlist's cover file name: the same for every rebuild.
pub fn cover_file_name(id: &str) -> String {
    format!("{}.svg", cover_stem(id))
}

/// The stem of a pre-coverfix dated cover name (`<stem>-<unix secs>.svg`), else `None`.
fn legacy_stem(name: &str) -> Option<&str> {
    let (stem, ts) = name.strip_suffix(".svg")?.rsplit_once('-')?;
    (!stem.is_empty() && !ts.is_empty() && ts.bytes().all(|b| b.is_ascii_digit())).then_some(stem)
}

/// Write `svg` as `id`'s cover in `dir` (temp file + rename, so a reader never sees half a file)
/// and drop that playlist's old dated files.
fn write_cover(dir: &std::path::Path, id: &str, svg: &str) -> std::io::Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stem = cover_stem(id);
    let path = dir.join(cover_file_name(id));
    let tmp = dir.join(format!(".{stem}.svg.tmp"));
    std::fs::write(&tmp, svg)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if legacy_stem(&e.file_name().to_string_lossy()) == Some(stem.as_str()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(path)
}

/// `path?v=<stamp>` (the stamp is the file's mtime in ms; a missing file gets no stamp).
fn versioned(path: &str, stamp: Option<u64>) -> String {
    match stamp {
        Some(v) => format!("{path}?v={v}"),
        None => path.to_owned(),
    }
}

/// `path` without the `?v=<digits>` that `versioned` adds (a `?` elsewhere is part of the name).
fn unversioned(path: &str) -> &str {
    match path.rsplit_once("?v=") {
        Some((p, v)) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => p,
        _ => path,
    }
}

/// The cover as the UI gets it: a local file with a cache-busting `?v=`, a URL unchanged.
fn cover_url(cover: Option<&str>) -> Option<String> {
    let cover = cover?;
    if cover.contains("://") || cover.starts_with("data:") {
        return Some(cover.to_owned());
    }
    let path = unversioned(cover);
    let stamp = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    Some(versioned(path, stamp))
}

/// Startup: move stored covers from the old dated names to the stable ones (a rename, no
/// redraw), then sweep dated files nothing points at any more. A cover whose file is gone
/// altogether is queued for a redraw (`cover_version = 0`).
pub fn migrate_cover_files(db: &Db, dir: &std::path::Path) {
    for mut m in load_all(db) {
        let Some(cover) = m.cover.clone() else { continue };
        let cur = std::path::PathBuf::from(unversioned(&cover));
        if cover.contains("://") || cur.parent() != Some(dir) {
            continue;
        }
        let name = cur.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Some(stem) = legacy_stem(&name).map(str::to_owned) else {
            if !cur.exists() {
                m.cover_version = 0;
                save(db, &m);
            }
            continue;
        };
        let stable = dir.join(format!("{stem}.svg"));
        if cur.exists() && !stable.exists() {
            let _ = std::fs::rename(&cur, &stable);
        }
        if stable.exists() {
            m.cover = Some(stable.to_string_lossy().to_string());
        } else {
            m.cover_version = 0;
        }
        save(db, &m);
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if let Some(stem) = legacy_stem(&name) {
            if dir.join(format!("{stem}.svg")).exists() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// A googleusercontent / ytimg thumbnail at `px` (the `=w..-h..` suffix), else unchanged.
pub fn sized(url: &str, px: u32) -> String {
    if url.contains("googleusercontent.com") || url.contains("ggpht.com") {
        if let Some(i) = url.rfind('=') {
            return format!("{}=w{px}-h{px}-l90-rj", &url[..i]);
        }
    }
    url.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    fn s(id: &str) -> SongItem {
        song(id, id, "Artist", None, "3:00")
    }

    #[test]
    fn periods_follow_the_schedule() {
        // 2026-11-18 is a Wednesday.
        let wed = Local.with_ymd_and_hms(2026, 11, 18, 15, 0, 0).unwrap().timestamp();
        let mon = Local.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).unwrap().timestamp();
        let fri = Local.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap().timestamp();
        let today = Local.with_ymd_and_hms(2026, 11, 18, 0, 0, 0).unwrap().timestamp();
        assert_eq!(period_start(Kind::DiscoverWeekly, wed), mon);
        assert_eq!(period_start(Kind::ReleaseRadar, wed), fri);
        assert_eq!(period_start(Kind::Daily, wed), today);
        let next_mon = Local.with_ymd_and_hms(2026, 11, 23, 0, 0, 0).unwrap().timestamp();
        assert_eq!(next_period(Kind::DiscoverWeekly, wed), next_mon);
        let next_fri = Local.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap().timestamp();
        assert_eq!(next_period(Kind::ReleaseRadar, wed), next_fri);
        // A Friday itself starts its own New Release Mix week.
        let fri_noon = Local.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap().timestamp();
        assert_eq!(period_start(Kind::ReleaseRadar, fri_noon), next_fri);
    }

    #[test]
    fn staleness() {
        let now = Local.with_ymd_and_hms(2026, 11, 18, 15, 0, 0).unwrap().timestamp();
        let m = |kind, at, signals, tracks: usize| Mfy {
            id: "x".into(),
            kind,
            title: String::new(),
            description: String::new(),
            cover: None,
            tracks: (0..tracks).map(|i| s(&i.to_string())).collect(),
            generated_at: at,
            signals,
            cover_version: COVER_VERSION,
        };
        // Built Monday morning: fresh all week.
        let mon = Local.with_ymd_and_hms(2026, 11, 16, 9, 0, 0).unwrap().timestamp();
        assert!(!is_stale(&m(Kind::DiscoverWeekly, mon, 10, 30), now, 10));
        // A mix from yesterday is stale.
        assert!(is_stale(&m(Kind::Daily, now - 86_400, 10, 30), now, 10));
        // Built cold, then an import landed: rebuild.
        assert!(is_stale(&m(Kind::DiscoverWeekly, now - 3600, 0, 0), now, 25));
        assert!(!is_stale(&m(Kind::DiscoverWeekly, now - 3600, 0, 0), now, 0));
    }

    #[test]
    fn rewind_brings_back_old_favourites_only() {
        let now = 100 * 86_400;
        let day = 86_400;
        let plays = vec![
            (s("old_fav"), 9, now - 60 * day),
            (s("current"), 20, now - day),
            (s("once"), 1, now - 90 * day),
            (s("old_ok"), 3, now - 40 * day),
        ];
        let likes = vec![(s("old_like"), now - 95 * day), (s("new_like"), now - 2 * day), (s("current"), 0)];
        let out = rewind_pick(plays, likes, now, 30);
        let ids: Vec<&str> = out.iter().map(|x| x.video_id.as_str()).collect();
        assert_eq!(ids, ["old_fav", "old_ok", "old_like"]);
    }

    #[test]
    fn cover_is_valid_svg_with_the_label() {
        let svg = cover_svg("Your Mix 1 & <you>", Some("data:image/jpeg;base64,AAA"), 0);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Your Mix 1 &amp; &lt;you&gt;"));
        assert!(svg.contains("data:image/jpeg;base64,AAA"));
        assert!(svg.contains(BANDS[0]));
        let plain = cover_svg("My Mix", None, 9);
        assert!(plain.contains(BANDS[9]) && !plain.contains("<image"));
    }

    #[test]
    fn cover_text_is_outlines_not_fonts() {
        // An SVG in an <img> can't load Figtree, so no <text>, no font-family.
        let kinds = [
            (Kind::Daily, "MFY_DAILY_2"),
            (Kind::DiscoverWeekly, DISCOVER_WEEKLY),
            (Kind::ReleaseRadar, RELEASE_RADAR),
            (Kind::RepeatRewind, REPEAT_REWIND),
            (Kind::OnRepeat, ON_REPEAT),
        ];
        for (n, (kind, id)) in kinds.into_iter().enumerate() {
            let label = title(kind, id);
            let svg = cover_svg(&label, Some("data:image/jpeg;base64,AAA"), n);
            assert!(!svg.contains("<text") && !svg.contains("font-family"), "{label}");
            // The name, glyph for glyph, and nothing else drawn as text.
            let drawn = label.chars().filter(|c| *c != ' ').count();
            assert_eq!(svg.matches("<path").count(), drawn + 1, "{label} (+1: the mark)");
            for c in label.chars() {
                assert!(!glyph(c).1.is_empty() || c == ' ', "{c}");
            }
        }
    }

    #[test]
    fn old_covers_are_redrawn_from_their_own_image() {
        // A row saved before cover versioning reads as version 0: its cover is redrawn.
        // Its old name is replaced on load.
        let json = r#"{"id":"MFY_DAILY_2","kind":"daily","title":"Mix 2 (old name)","description":"",
            "cover":null,"tracks":[],"generatedAt":5,"signals":3}"#;
        let old = parse(json).unwrap();
        assert_eq!(old.title, "Your Mix 2");
        assert_eq!(old.cover_version, 0);
        assert!(needs_new_cover(&old));
        assert_eq!(cover_args(&old), ("Your Mix 2".to_string(), 1));
        let fresh = Mfy { cover_version: COVER_VERSION, ..old.clone() };
        assert!(!needs_new_cover(&fresh));
        // Only the cover is due: the list itself isn't stale because of it.
        assert_eq!(is_stale(&old, 6, 3), is_stale(&fresh, 6, 3));
        // The redraw reuses the artwork embedded in the old SVG (no network).
        let img = "data:image/jpeg;base64,AAA+/=";
        let svg = cover_svg("Your Mix 2", Some(img), 1);
        assert_eq!(embedded_image(&svg).as_deref(), Some(img));
        assert_eq!(embedded_image(&cover_svg("Your Rotation", None, 9)), None);
        let dir = std::env::temp_dir().join(format!("mfy-cover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("MFY_DAILY_2-1.svg");
        std::fs::write(&path, &svg).unwrap();
        assert_eq!(cover_art(path.to_str()).as_deref(), Some(img));
        assert_eq!(cover_art(Some("https://x/y.jpg")).as_deref(), Some("https://x/y.jpg"));
        assert_eq!(cover_art(Some("/nope/missing.svg")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cover_files_keep_one_stable_name() {
        assert_eq!(cover_file_name("MFY_DAILY_1"), "MFY_DAILY_1.svg");
        assert_eq!(cover_file_name("MFY_../x"), "MFY_x.svg");
        assert_eq!(legacy_stem("MFY_DAILY_1-1794747288.svg"), Some("MFY_DAILY_1"));
        assert_eq!(legacy_stem("MFY_DAILY_1.svg"), None);
        assert_eq!(legacy_stem("MFY_DAILY_1-.svg"), None);
        assert_eq!(legacy_stem("MFY_DAILY_1-12a.svg"), None);
        assert_eq!(legacy_stem("-12.svg"), None);

        let dir = std::env::temp_dir().join(format!("mfy-coverfix-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Dated leftovers of this playlist and a neighbour whose id shares the prefix.
        std::fs::write(dir.join("MFY_DAILY_1-100.svg"), "old").unwrap();
        std::fs::write(dir.join("MFY_DAILY_10-100.svg"), "other").unwrap();
        let a = write_cover(&dir, "MFY_DAILY_1", "<svg>1</svg>").unwrap();
        let b = write_cover(&dir, "MFY_DAILY_1", "<svg>2</svg>").unwrap();
        assert_eq!(a, b, "a redraw keeps the same file, so cached URLs stay valid");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "<svg>2</svg>");
        assert!(!dir.join("MFY_DAILY_1-100.svg").exists());
        assert!(dir.join("MFY_DAILY_10-100.svg").exists(), "another playlist's file is left alone");
        assert!(!dir.join(".MFY_DAILY_1.svg.tmp").exists());

        // The UI URL: the path plus an mtime stamp; the query is ignored when reading it back.
        let p = b.to_string_lossy().to_string();
        let url = cover_url(Some(&p)).unwrap();
        assert!(url.starts_with(&format!("{p}?v=")), "{url}");
        assert_eq!(cover_url(Some(&url)).unwrap(), url, "re-stamping is idempotent");
        assert_eq!(cover_url(Some("/nope/x.svg")).as_deref(), Some("/nope/x.svg"));
        assert_eq!(cover_url(Some("https://x/y.jpg")).as_deref(), Some("https://x/y.jpg"));
        assert_eq!(versioned("/a.svg", Some(7)), "/a.svg?v=7");
        assert_eq!(unversioned("/a.svg?v=7"), "/a.svg");
        assert_eq!(unversioned("/why?.svg"), "/why?.svg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn startup_moves_dated_covers_to_stable_names() {
        let db = super::super::test_db();
        let dir = std::env::temp_dir().join(format!("mfy-coverfix-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |id: &str, cover: Option<String>| Mfy {
            id: id.into(),
            kind: Kind::Daily,
            title: id.into(),
            description: String::new(),
            cover,
            tracks: vec![],
            generated_at: 5,
            signals: 3,
            cover_version: COVER_VERSION,
        };
        let dated = |n: &str| dir.join(n).to_string_lossy().to_string();
        // 1: the dated file exists -> renamed. 2: its file is gone -> redraw queued.
        // 3: already stable. Plus an orphan dated file next to a stable one.
        std::fs::write(dir.join("MFY_DAILY_1-100.svg"), "one").unwrap();
        std::fs::write(dir.join("MFY_DAILY_3.svg"), "three").unwrap();
        std::fs::write(dir.join("MFY_DAILY_3-90.svg"), "stale").unwrap();
        save(&db, &mk("MFY_DAILY_1", Some(dated("MFY_DAILY_1-100.svg"))));
        save(&db, &mk("MFY_DAILY_2", Some(dated("MFY_DAILY_2-100.svg"))));
        save(&db, &mk("MFY_DAILY_3", Some(dated("MFY_DAILY_3.svg"))));
        migrate_cover_files(&db, &dir);

        let one = load(&db, "MFY_DAILY_1").unwrap();
        assert_eq!(one.cover.as_deref(), Some(dated("MFY_DAILY_1.svg").as_str()));
        assert_eq!(std::fs::read_to_string(dir.join("MFY_DAILY_1.svg")).unwrap(), "one");
        assert!(!needs_new_cover(&one));
        assert!(needs_new_cover(&load(&db, "MFY_DAILY_2").unwrap()));
        assert!(!needs_new_cover(&load(&db, "MFY_DAILY_3").unwrap()));
        assert!(!dir.join("MFY_DAILY_1-100.svg").exists());
        assert!(!dir.join("MFY_DAILY_3-90.svg").exists());
        // A second run changes nothing.
        migrate_cover_files(&db, &dir);
        assert_eq!(load(&db, "MFY_DAILY_1").unwrap().cover, one.cover);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_round_trip_keeps_spotify_order() {
        let db = super::super::test_db();
        for (id, kind) in [
            (DISCOVER_WEEKLY, Kind::DiscoverWeekly),
            ("MFY_DAILY_2", Kind::Daily),
            ("MFY_DAILY_1", Kind::Daily),
            (REPEAT_REWIND, Kind::RepeatRewind),
        ] {
            save(
                &db,
                &Mfy {
                    id: id.into(),
                    kind,
                    title: id.into(),
                    description: String::new(),
                    cover: None,
                    tracks: vec![s("a")],
                    generated_at: 1,
                    signals: 1,
                    cover_version: COVER_VERSION,
                },
            );
        }
        let ids: Vec<String> = load_all(&db).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["MFY_DAILY_1", "MFY_DAILY_2", DISCOVER_WEEKLY, REPEAT_REWIND]);
        let page = page(&db, DISCOVER_WEEKLY).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(!page.owned);
        assert_eq!(shelf(&db).unwrap().items[0].kind, "playlist");
    }

    #[test]
    fn known_songs_cover_listens_likes_and_playlists() {
        let db = super::super::test_db();
        let a = serde_json::to_string(&s("heard")).unwrap();
        db.record_listen("heard", &a, 1, 100);
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO liked_songs(video_id, title, artists, song_json, added_at)
                 VALUES('liked', 't', 'a', ?1, 5)",
                [serde_json::to_string(&s("liked")).unwrap()],
            )
            .unwrap();
        });
        let ids: HashSet<String> = known_songs(&db).into_iter().map(|s| s.video_id).collect();
        assert!(ids.contains("heard") && ids.contains("liked"));
        let seeds = dw_seeds(&db, 5);
        assert_eq!(seeds[0].video_id, "heard");
        assert_eq!(seeds[1].video_id, "liked");
    }
}
