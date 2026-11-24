//! SQLite tables for podcasts. All functions take a `&Connection` so tests run on an in-memory
//! database; the app passes `Db::podcasts_conn()`.
//!
//! `podcast_follows` has exactly the shape stream B (local library) declares, so whichever branch
//! lands first creates it and the other's `CREATE TABLE IF NOT EXISTS` is a no-op. Everything this
//! stream needs beyond that shape lives in its own `podcast_*` tables.

use rusqlite::{params, Connection, OptionalExtension};

use super::{now_secs, rules, Episode, Progress, Show, ShowSource, Sort};

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS podcast_follows (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            author      TEXT,
            thumb       TEXT,
            feed_url    TEXT,
            yt_browse_id TEXT,
            followed_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS podcast_show_meta (
            show_id      TEXT PRIMARY KEY,
            description  TEXT,
            itunes_id    INTEGER,
            etag         TEXT,
            last_modified TEXT,
            last_refresh INTEGER
        );
        CREATE TABLE IF NOT EXISTS podcast_episodes (
            id           TEXT PRIMARY KEY,
            show_id      TEXT,
            show_title   TEXT NOT NULL DEFAULT '',
            title        TEXT NOT NULL,
            published_at INTEGER,
            duration_s   INTEGER,
            description  TEXT,
            thumbnail    TEXT,
            video_id     TEXT,
            enclosure_url TEXT,
            guid         TEXT,
            is_video     INTEGER NOT NULL DEFAULT 0,
            first_seen   INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS podcast_episodes_show ON podcast_episodes(show_id, published_at);
        CREATE TABLE IF NOT EXISTS podcast_progress (
            episode_id  TEXT PRIMARY KEY,
            position_s  REAL NOT NULL DEFAULT 0,
            duration_s  REAL NOT NULL DEFAULT 0,
            played      INTEGER NOT NULL DEFAULT 0,
            played_at   INTEGER,
            updated_at  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS podcast_saved (
            episode_id TEXT PRIMARY KEY,
            saved_at   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS podcast_sponsorblock (
            video_id   TEXT PRIMARY KEY,
            segments   TEXT NOT NULL,
            fetched_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS podcast_http_cache (
            key        TEXT PRIMARY KEY,
            body       TEXT NOT NULL,
            fetched_at INTEGER NOT NULL
        );
        "#,
    )?;
    // stream: chapters. An error here means the column is already there.
    let _ = conn.execute("ALTER TABLE podcast_episodes ADD COLUMN chapters_url TEXT", []);
    // stream: podcast-upnext. Same.
    let _ = conn.execute("ALTER TABLE podcast_show_meta ADD COLUMN serial INTEGER", []);
    Ok(())
}

/// stream: podcast-upnext. Remember whether a feed calls itself serial ([`Show::serial`]).
pub fn set_serial(conn: &Connection, show_id: &str, serial: bool) {
    let _ = conn.execute(
        "INSERT INTO podcast_show_meta(show_id, serial) VALUES(?1, ?2)
         ON CONFLICT(show_id) DO UPDATE SET serial = excluded.serial",
        params![show_id, serial as i64],
    );
}

/// stream: podcast-upnext. Whether the show's feed said it is serial (unknown reads as episodic).
pub fn is_serial(conn: &Connection, show_id: &str) -> bool {
    conn.query_row("SELECT serial FROM podcast_show_meta WHERE show_id = ?1", [show_id], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .ok()
    .flatten()
    .is_some_and(|v| v != 0)
}

// --- follows -------------------------------------------------------------------------------------

pub fn follow(conn: &Connection, show: &Show) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO podcast_follows(id, title, author, thumb, feed_url, yt_browse_id, followed_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, author = excluded.author,
           thumb = COALESCE(excluded.thumb, thumb), feed_url = COALESCE(excluded.feed_url, feed_url),
           yt_browse_id = COALESCE(excluded.yt_browse_id, yt_browse_id)",
        params![
            show.id,
            show.title,
            show.author,
            show.thumbnail,
            show.feed_url,
            show.yt_browse_id,
            now_secs()
        ],
    )?;
    upsert_show_meta(conn, show)
}

pub fn unfollow(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM podcast_follows WHERE id = ?1", [id])?;
    Ok(())
}

pub fn is_followed(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM podcast_follows WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

/// Followed shows, most recently followed first.
pub fn follows(conn: &Connection) -> rusqlite::Result<Vec<Show>> {
    let mut st = conn.prepare(
        "SELECT f.id, f.title, f.author, f.thumb, f.feed_url, f.yt_browse_id, m.description, m.itunes_id
         FROM podcast_follows f LEFT JOIN podcast_show_meta m ON m.show_id = f.id
         ORDER BY f.followed_at DESC, f.title",
    )?;
    let rows = st.query_map([], |r| {
        let id: String = r.get(0)?;
        Ok(Show {
            source: if id.starts_with("MPSP") { ShowSource::Youtube } else { ShowSource::Rss },
            id,
            title: r.get(1)?,
            author: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            thumbnail: r.get(3)?,
            feed_url: r.get(4)?,
            yt_browse_id: r.get(5)?,
            description: r.get(6)?,
            itunes_id: r.get(7)?,
            followed: true,
            episode_count: None,
            dir_author: None,
            chart_rank: None,
            serial: false,
        })
    })?;
    rows.collect()
}

/// Refresh a followed show's title / author / art from its source (no-op when not followed).
pub fn update_follow_meta(conn: &Connection, id: &str, show: &Show) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE podcast_follows SET title = CASE WHEN ?2 = '' THEN title ELSE ?2 END,
           author = CASE WHEN ?3 = '' THEN author ELSE ?3 END, thumb = COALESCE(?4, thumb)
         WHERE id = ?1",
        params![id, show.title, show.author, show.thumbnail],
    )?;
    let mut meta = show.clone();
    meta.id = id.to_owned();
    upsert_show_meta(conn, &meta)
}

pub fn upsert_show_meta(conn: &Connection, show: &Show) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO podcast_show_meta(show_id, description, itunes_id) VALUES(?1, ?2, ?3)
         ON CONFLICT(show_id) DO UPDATE SET description = COALESCE(excluded.description, description),
           itunes_id = COALESCE(excluded.itunes_id, itunes_id)",
        params![show.id, show.description, show.itunes_id],
    )?;
    Ok(())
}

/// Conditional-GET validators and last refresh time of a show.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FeedState {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_refresh: Option<i64>,
}

pub fn feed_state(conn: &Connection, show_id: &str) -> FeedState {
    conn.query_row(
        "SELECT etag, last_modified, last_refresh FROM podcast_show_meta WHERE show_id = ?1",
        [show_id],
        |r| Ok(FeedState { etag: r.get(0)?, last_modified: r.get(1)?, last_refresh: r.get(2)? }),
    )
    .unwrap_or_default()
}

pub fn set_feed_state(conn: &Connection, show_id: &str, s: &FeedState) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO podcast_show_meta(show_id, etag, last_modified, last_refresh) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(show_id) DO UPDATE SET etag = excluded.etag,
           last_modified = excluded.last_modified, last_refresh = excluded.last_refresh",
        params![show_id, s.etag, s.last_modified, s.last_refresh],
    )?;
    Ok(())
}

// --- episodes ------------------------------------------------------------------------------------

/// Insert or refresh episodes. Returns the ids that were not known before (new episodes).
///
/// A known row keeps its first `published_at`: YouTube dates are relative ("5d ago") and would
/// otherwise drift by the refresh interval on every pass.
pub fn upsert_episodes(conn: &Connection, eps: &[Episode]) -> rusqlite::Result<Vec<String>> {
    let now = now_secs();
    let mut fresh = Vec::new();
    let mut ins = conn.prepare_cached(
        "INSERT OR IGNORE INTO podcast_episodes(id, show_id, show_title, title, published_at,
           duration_s, description, thumbnail, video_id, enclosure_url, guid, is_video, first_seen,
           chapters_url)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?;
    let mut upd = conn.prepare_cached(
        "UPDATE podcast_episodes SET show_id = COALESCE(?2, show_id),
           show_title = CASE WHEN ?3 = '' THEN show_title ELSE ?3 END, title = ?4,
           published_at = COALESCE(published_at, ?5), duration_s = COALESCE(?6, duration_s),
           description = COALESCE(?7, description), thumbnail = COALESCE(?8, thumbnail),
           video_id = COALESCE(?9, video_id), enclosure_url = COALESCE(?10, enclosure_url),
           guid = COALESCE(?11, guid), is_video = MAX(is_video, ?12),
           chapters_url = COALESCE(?13, chapters_url)
         WHERE id = ?1",
    )?;
    for e in eps {
        let p = params![
            e.id,
            e.show_id,
            e.show_title,
            e.title,
            e.published_at,
            e.duration_secs,
            e.description,
            e.thumbnail,
            e.video_id,
            e.enclosure_url,
            e.guid,
            e.is_video as i64,
            e.chapters_url,
        ];
        let n = ins.execute(params![
            e.id,
            e.show_id,
            e.show_title,
            e.title,
            e.published_at,
            e.duration_secs,
            e.description,
            e.thumbnail,
            e.video_id,
            e.enclosure_url,
            e.guid,
            e.is_video as i64,
            now,
            e.chapters_url
        ])?;
        if n > 0 {
            fresh.push(e.id.clone());
        } else {
            upd.execute(p)?;
        }
    }
    Ok(fresh)
}

const EP_COLS: &str = "e.id, e.show_id, e.show_title, e.title, e.published_at, e.duration_s,
    e.description, e.thumbnail, e.video_id, e.enclosure_url, e.guid, e.is_video,
    p.position_s, p.duration_s, p.played, p.updated_at, s.saved_at";
const EP_JOIN: &str = "FROM podcast_episodes e
    LEFT JOIN podcast_progress p ON p.episode_id = e.id
    LEFT JOIN podcast_saved s ON s.episode_id = e.id";

fn episode_row(r: &rusqlite::Row) -> rusqlite::Result<Episode> {
    let pos: Option<f64> = r.get(12)?;
    let pdur: Option<f64> = r.get(13)?;
    let played: Option<i64> = r.get(14)?;
    let dur_s: Option<i64> = r.get(5)?;
    let pos = pos.unwrap_or(0.0);
    let played = played.unwrap_or(0) != 0;
    let dur = pdur.filter(|d| *d > 0.0).or(dur_s.map(|d| d as f64)).unwrap_or(0.0);
    Ok(Episode {
        id: r.get(0)?,
        show_id: r.get(1)?,
        show_title: r.get(2)?,
        title: r.get(3)?,
        published_at: r.get(4)?,
        duration_secs: dur_s,
        description: r.get(6)?,
        thumbnail: r.get(7)?,
        video_id: r.get(8)?,
        enclosure_url: r.get(9)?,
        guid: r.get(10)?,
        is_video: r.get::<_, i64>(11)? != 0,
        progress: Progress {
            position_secs: pos,
            duration_secs: dur,
            fraction: rules::fraction(pos, dur, played),
            played,
            updated_at: r.get::<_, Option<i64>>(15)?.unwrap_or(0),
        },
        saved: r.get::<_, Option<i64>>(16)?.is_some(),
        chapters_url: None,
    })
}

pub fn get_episode(conn: &Connection, id: &str) -> Option<Episode> {
    conn.query_row(&format!("SELECT {EP_COLS} {EP_JOIN} WHERE e.id = ?1"), [id], episode_row)
        .optional()
        .ok()
        .flatten()
}

/// stream: chapters. The feed's chapters file for an RSS episode, if it links one.
pub fn chapters_url(conn: &Connection, id: &str) -> Option<String> {
    conn.query_row("SELECT chapters_url FROM podcast_episodes WHERE id = ?1", [id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
}

/// One page of a show's stored episodes (the RSS show page reads from here).
pub fn show_episodes(
    conn: &Connection,
    show_id: &str,
    sort: Sort,
    offset: usize,
    limit: usize,
) -> rusqlite::Result<Vec<Episode>> {
    let order = match sort {
        Sort::Newest => "e.published_at DESC, e.rowid ASC",
        Sort::Oldest => "e.published_at ASC, e.rowid DESC",
    };
    let mut st = conn.prepare(&format!(
        "SELECT {EP_COLS} {EP_JOIN} WHERE e.show_id = ?1 ORDER BY {order} LIMIT ?2 OFFSET ?3"
    ))?;
    let rows = st.query_map(params![show_id, limit as i64, offset as i64], episode_row)?;
    rows.collect()
}

pub fn count_show_episodes(conn: &Connection, show_id: &str) -> usize {
    conn.query_row("SELECT COUNT(*) FROM podcast_episodes WHERE show_id = ?1", [show_id], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0) as usize
}

/// Fill `progress` and `saved` on episodes that came from the network.
pub fn decorate(conn: &Connection, eps: &mut [Episode]) {
    let Ok(mut st) = conn.prepare_cached(
        "SELECT p.position_s, p.duration_s, p.played, p.updated_at,
           (SELECT 1 FROM podcast_saved s WHERE s.episode_id = ?1)
         FROM (SELECT ?1 AS id) x LEFT JOIN podcast_progress p ON p.episode_id = x.id",
    ) else {
        return;
    };
    for e in eps.iter_mut() {
        let row = st.query_row([&e.id], |r| {
            Ok((
                r.get::<_, Option<f64>>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        });
        if let Ok((pos, pdur, played, upd, saved)) = row {
            let pos = pos.unwrap_or(0.0);
            let played = played.unwrap_or(0) != 0;
            let dur = pdur
                .filter(|d| *d > 0.0)
                .or(e.duration_secs.map(|d| d as f64))
                .unwrap_or(0.0);
            e.progress = Progress {
                position_secs: pos,
                duration_secs: dur,
                fraction: rules::fraction(pos, dur, played),
                played,
                updated_at: upd.unwrap_or(0),
            };
            e.saved = saved.is_some();
        }
    }
}

/// New episodes of followed shows published since `since` (unix secs), newest first. Played
/// episodes are left out, like Spotify's "New episodes".
pub fn new_episodes(conn: &Connection, since: i64, limit: usize) -> rusqlite::Result<Vec<Episode>> {
    let mut st = conn.prepare(&format!(
        "SELECT {EP_COLS} {EP_JOIN}
         WHERE e.show_id IN (SELECT id FROM podcast_follows)
           AND COALESCE(e.published_at, e.first_seen) >= ?1
           AND COALESCE(p.played, 0) = 0
         ORDER BY COALESCE(e.published_at, e.first_seen) DESC LIMIT ?2"
    ))?;
    let rows = st.query_map(params![since, limit as i64], episode_row)?;
    rows.collect()
}

/// Started, not finished, most recently listened first.
pub fn continue_listening(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<Episode>> {
    let mut st = conn.prepare(&format!(
        "SELECT {EP_COLS} {EP_JOIN}
         WHERE p.played = 0 AND p.position_s >= ?1
         ORDER BY p.updated_at DESC LIMIT ?2"
    ))?;
    let rows = st.query_map(params![rules::MIN_STARTED_SECS, limit as i64], episode_row)?;
    rows.collect()
}

/// "Your Episodes": saved, most recently saved first.
pub fn saved_episodes(conn: &Connection) -> rusqlite::Result<Vec<Episode>> {
    let mut st = conn.prepare(&format!(
        "SELECT {EP_COLS} {EP_JOIN} WHERE s.saved_at IS NOT NULL ORDER BY s.saved_at DESC"
    ))?;
    let rows = st.query_map([], episode_row)?;
    rows.collect()
}

pub fn set_saved(conn: &Connection, ep: &Episode, saved: bool) -> rusqlite::Result<()> {
    if saved {
        upsert_episodes(conn, std::slice::from_ref(ep))?;
        conn.execute(
            "INSERT OR IGNORE INTO podcast_saved(episode_id, saved_at) VALUES(?1, ?2)",
            params![ep.id, now_secs()],
        )?;
    } else {
        conn.execute("DELETE FROM podcast_saved WHERE episode_id = ?1", [&ep.id])?;
    }
    Ok(())
}

/// Spotify keeps downloaded episodes in "Your Episodes": downloading one saves it (once; an
/// episode already saved keeps its place). Removing the download never unsaves. The stored row
/// (from a show or episode page) wins over `fallback`, which is built from the download request.
/// Returns whether it was newly saved.
pub fn save_downloaded(conn: &Connection, fallback: &Episode) -> rusqlite::Result<bool> {
    let saved: bool = conn
        .query_row("SELECT 1 FROM podcast_saved WHERE episode_id = ?1", [&fallback.id], |_| Ok(()))
        .optional()?
        .is_some();
    if saved {
        return Ok(false);
    }
    let ep = get_episode(conn, &fallback.id).unwrap_or_else(|| fallback.clone());
    set_saved(conn, &ep, true)?;
    Ok(true)
}

// --- progress ------------------------------------------------------------------------------------

pub fn progress(conn: &Connection, id: &str) -> Option<Progress> {
    conn.query_row(
        "SELECT position_s, duration_s, played, updated_at FROM podcast_progress WHERE episode_id = ?1",
        [id],
        |r| {
            let (pos, dur, played): (f64, f64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
            Ok(Progress {
                position_secs: pos,
                duration_secs: dur,
                fraction: rules::fraction(pos, dur, played != 0),
                played: played != 0,
                updated_at: r.get(3)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// Save a playing position. Marks played when the rules say so; never un-marks (a replay of a
/// played episode stays played until it is explicitly marked unplayed). Returns the stored state.
pub fn save_position(conn: &Connection, id: &str, pos: f64, dur: f64) -> rusqlite::Result<Progress> {
    let now = now_secs();
    let played_now = rules::is_played(pos, dur);
    conn.execute(
        "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, played_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, CASE WHEN ?4 = 1 THEN ?5 END, ?5)
         ON CONFLICT(episode_id) DO UPDATE SET position_s = excluded.position_s,
           duration_s = CASE WHEN excluded.duration_s > 0 THEN excluded.duration_s ELSE duration_s END,
           played = MAX(played, excluded.played),
           played_at = CASE WHEN excluded.played = 1 THEN COALESCE(played_at, ?5) ELSE played_at END,
           updated_at = ?5",
        params![id, pos, dur, played_now as i64, now],
    )?;
    Ok(progress(conn, id).unwrap_or_default())
}

pub fn set_played(conn: &Connection, id: &str, played: bool) -> rusqlite::Result<()> {
    let now = now_secs();
    if played {
        conn.execute(
            "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, played_at, updated_at)
             VALUES(?1, 0, 0, 1, ?2, ?2)
             ON CONFLICT(episode_id) DO UPDATE SET played = 1, played_at = ?2, updated_at = ?2",
            params![id, now],
        )?;
    } else {
        // Unplayed means from the top, like AntennaPod's "mark unplayed".
        conn.execute(
            "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, updated_at)
             VALUES(?1, 0, 0, 0, ?2)
             ON CONFLICT(episode_id) DO UPDATE SET played = 0, played_at = NULL, position_s = 0, updated_at = ?2",
            params![id, now],
        )?;
    }
    Ok(())
}

/// Mark every stored episode of a show played. Returns how many.
pub fn mark_show_played(conn: &Connection, show_id: &str) -> rusqlite::Result<usize> {
    let now = now_secs();
    conn.execute(
        "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, played_at, updated_at)
         SELECT id, 0, COALESCE(duration_s, 0), 1, ?2, ?2 FROM podcast_episodes WHERE show_id = ?1
         ON CONFLICT(episode_id) DO UPDATE SET played = 1, played_at = ?2, updated_at = ?2",
        params![show_id, now],
    )
}

// --- show art (stream: tilefix) ------------------------------------------------------------------

/// How long a signed YouTube show-art URL is trusted after it was handed out. YouTube's square
/// podcast art (`i.ytimg.com/pl_c/<playlist>/studio_square_thumbnail.jpg?sqp=…&rs=…`) is signed,
/// and the signature dies within hours: the same URL that loaded on the show page 404s at the next
/// launch (seen live: dead under 7 h; there's no unsigned form, the bare path 404s too).
pub const SHOW_ART_FRESH_SECS: i64 = 2 * 3600;

/// A YouTube show-art URL whose signature expires (see [`SHOW_ART_FRESH_SECS`]).
pub fn is_expiring_art(url: &str) -> bool {
    url.contains("i.ytimg.com/pl_c/")
}

/// When YouTube signed an expiring art URL: the `sqp` parameter is a base64url protobuf whose
/// first field (tag `0x08`) is the issue time in unix seconds.
pub fn art_signed_at(url: &str) -> Option<i64> {
    use base64::Engine;
    let sqp = url.split(['?', '&']).find_map(|p| p.strip_prefix("sqp="))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sqp.trim_end_matches('='))
        .ok()?;
    let mut it = bytes.into_iter();
    if it.next()? != 0x08 {
        return None;
    }
    let mut v: i64 = 0;
    for (i, b) in it.enumerate().take(9) {
        v |= i64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn show_art_key(show_id: &str) -> String {
    format!("showart:{show_id}")
}

/// Keep the art a fresh show fetch just returned, so cards recorded with an older (dead) URL can
/// show it while it lasts.
pub fn remember_show_art(conn: &Connection, show_id: &str, thumb: Option<&str>) {
    if let Some(t) = thumb.filter(|t| !t.is_empty()) {
        cache_put(conn, &show_art_key(show_id), t);
    }
}

/// Artwork for a show card (a recently played context, Home's shortcut tiles) that will actually
/// load. A stored URL that doesn't expire is kept. An expiring YouTube one is kept only while young;
/// otherwise, in order: the art from the last show fetch while still fresh, the followed show's
/// art when it is durable (RSS / iTunes), the show's newest stored episode art (`i.ytimg.com/vi/…`
/// thumbnails don't expire), else `None` so the UI draws its placeholder tile.
pub fn show_art(conn: &Connection, show_id: &str, stored: Option<&str>) -> Option<String> {
    let fresh = |u: &str| {
        !is_expiring_art(u)
            || art_signed_at(u).is_some_and(|t| now_secs() - t < SHOW_ART_FRESH_SECS)
    };
    if let Some(u) = stored.filter(|u| !u.is_empty() && fresh(u)) {
        return Some(u.to_owned());
    }
    if let Some(u) = cache_get(conn, &show_art_key(show_id), SHOW_ART_FRESH_SECS) {
        return Some(u);
    }
    let followed: Option<String> = conn
        .query_row("SELECT thumb FROM podcast_follows WHERE id = ?1", [show_id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .flatten();
    if let Some(u) = followed.filter(|u| !u.is_empty() && !is_expiring_art(u)) {
        return Some(u);
    }
    let mut st = conn
        .prepare(
            "SELECT thumbnail FROM podcast_episodes WHERE show_id = ?1 AND thumbnail <> ''
             ORDER BY published_at DESC, rowid ASC LIMIT 20",
        )
        .ok()?;
    let art = st
        .query_map([show_id], |r| r.get::<_, String>(0))
        .ok()?
        .flatten()
        .find(|u| !is_expiring_art(u));
    art
}

// --- caches --------------------------------------------------------------------------------------

pub fn cache_get(conn: &Connection, key: &str, max_age: i64) -> Option<String> {
    conn.query_row(
        "SELECT body FROM podcast_http_cache WHERE key = ?1 AND fetched_at >= ?2",
        params![key, now_secs() - max_age],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn cache_put(conn: &Connection, key: &str, body: &str) {
    let _ = conn.execute(
        "INSERT INTO podcast_http_cache(key, body, fetched_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET body = excluded.body, fetched_at = excluded.fetched_at",
        params![key, body, now_secs()],
    );
    // Keep the table small: a week is past every TTL this module uses.
    let _ = conn.execute(
        "DELETE FROM podcast_http_cache WHERE fetched_at < ?1",
        [now_secs() - 7 * 86_400],
    );
}

pub fn sponsorblock_get(conn: &Connection, video_id: &str, max_age: i64) -> Option<String> {
    conn.query_row(
        "SELECT segments FROM podcast_sponsorblock WHERE video_id = ?1 AND fetched_at >= ?2",
        params![video_id, now_secs() - max_age],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn sponsorblock_put(conn: &Connection, video_id: &str, segments: &str) {
    let _ = conn.execute(
        "INSERT INTO podcast_sponsorblock(video_id, segments, fetched_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(video_id) DO UPDATE SET segments = excluded.segments, fetched_at = excluded.fetched_at",
        params![video_id, segments, now_secs()],
    );
}

#[cfg(test)]
pub fn test_conn() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    init(&c).unwrap();
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(id: &str, show: &str, at: i64) -> Episode {
        Episode {
            id: id.into(),
            show_id: Some(show.into()),
            show_title: "Show".into(),
            title: format!("Ep {id}"),
            published_at: Some(at),
            duration_secs: Some(3600),
            ..Default::default()
        }
    }

    #[test]
    fn downloading_saves_once_and_prefers_the_stored_row() {
        let c = test_conn();
        // Stored from a show page: the fallback's thin fields don't replace it.
        upsert_episodes(&c, &[ep("a", "S", 100)]).unwrap();
        let thin = Episode { id: "a".into(), title: "Ep a".into(), ..Default::default() };
        assert!(save_downloaded(&c, &thin).unwrap());
        let a = &saved_episodes(&c).unwrap()[0];
        assert_eq!(a.show_id.as_deref(), Some("S"));
        assert_eq!(a.show_title, "Show");
        // Already saved: nothing changes.
        assert!(!save_downloaded(&c, &thin).unwrap());
        // Never seen before (a search result): the fallback is saved.
        let b = Episode { id: "b".into(), title: "B".into(), show_title: "Other".into(), ..Default::default() };
        assert!(save_downloaded(&c, &b).unwrap());
        assert_eq!(saved_episodes(&c).unwrap().len(), 2);
    }

    #[test]
    fn chapters_url_is_kept_across_refreshes() {
        let c = test_conn();
        init(&c).unwrap(); // a second init (the ALTER) is harmless
        let mut e = ep("PODURL:https://x/1.mp3", "https://f", 100);
        e.chapters_url = Some("https://x/1.json".into());
        upsert_episodes(&c, &[e.clone()]).unwrap();
        e.chapters_url = None;
        upsert_episodes(&c, &[e]).unwrap();
        assert_eq!(chapters_url(&c, "PODURL:https://x/1.mp3").as_deref(), Some("https://x/1.json"));
        assert_eq!(chapters_url(&c, "nope"), None);
    }

    #[test]
    fn follow_unfollow_and_list() {
        let c = test_conn();
        let mut s = Show::rss("https://f/feed.xml");
        s.title = "F".into();
        follow(&c, &s).unwrap();
        assert!(is_followed(&c, "https://f/feed.xml"));
        let all = follows(&c).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].source, ShowSource::Rss);
        assert!(all[0].followed);
        unfollow(&c, "https://f/feed.xml").unwrap();
        assert!(follows(&c).unwrap().is_empty());
    }

    #[test]
    fn upsert_reports_only_new_and_keeps_first_date() {
        let c = test_conn();
        let fresh = upsert_episodes(&c, &[ep("a", "S", 100), ep("b", "S", 200)]).unwrap();
        assert_eq!(fresh, vec!["a", "b"]);
        let mut again = ep("a", "S", 999);
        again.title = "Renamed".into();
        let fresh = upsert_episodes(&c, &[again, ep("c", "S", 300)]).unwrap();
        assert_eq!(fresh, vec!["c"]);
        let a = get_episode(&c, "a").unwrap();
        assert_eq!(a.published_at, Some(100));
        assert_eq!(a.title, "Renamed");
        let page = show_episodes(&c, "S", Sort::Newest, 0, 2).unwrap();
        assert_eq!(page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["c", "b"]);
        let page = show_episodes(&c, "S", Sort::Oldest, 1, 5).unwrap();
        assert_eq!(page.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["b", "c"]);
        assert_eq!(count_show_episodes(&c, "S"), 3);
    }

    #[test]
    fn resume_progress_played_and_continue_listening() {
        let c = test_conn();
        upsert_episodes(&c, &[ep("a", "S", 100), ep("b", "S", 200)]).unwrap();
        let p = save_position(&c, "a", 600.0, 3600.0).unwrap();
        assert!(!p.played);
        assert!((p.fraction - 600.0 / 3600.0).abs() < 1e-9);
        let cl = continue_listening(&c, 10).unwrap();
        assert_eq!(cl.len(), 1);
        assert_eq!(cl[0].progress.position_secs, 600.0);
        // Into the last stretch: played, leaves Continue listening.
        let p = save_position(&c, "a", 3590.0, 3600.0).unwrap();
        assert!(p.played);
        assert_eq!(p.fraction, 1.0);
        assert!(continue_listening(&c, 10).unwrap().is_empty());
        // Replay from the top doesn't flip it back to unplayed on the first ticks.
        let p = save_position(&c, "a", 3.0, 3600.0).unwrap();
        assert!(p.played);
        // Manual unplayed resets the position.
        set_played(&c, "a", false).unwrap();
        let p = progress(&c, "a").unwrap();
        assert!(!p.played);
        assert_eq!(p.position_secs, 0.0);
        set_played(&c, "b", true).unwrap();
        assert!(progress(&c, "b").unwrap().played);
        assert_eq!(mark_show_played(&c, "S").unwrap(), 2);
        assert!(progress(&c, "a").unwrap().played);
    }

    #[test]
    fn decorate_fills_progress_and_saved() {
        let c = test_conn();
        let e = ep("a", "S", 100);
        set_saved(&c, &e, true).unwrap();
        save_position(&c, "a", 1800.0, 3600.0).unwrap();
        let mut v = vec![ep("a", "S", 100), ep("zz", "S", 1)];
        decorate(&c, &mut v);
        assert!(v[0].saved);
        assert_eq!(v[0].progress.fraction, 0.5);
        assert!(!v[1].saved);
        // Never played: no position, but the bar still knows the length.
        assert_eq!(v[1].progress.position_secs, 0.0);
        assert_eq!(v[1].progress.duration_secs, 3600.0);
        assert_eq!(v[1].progress.fraction, 0.0);
        assert_eq!(saved_episodes(&c).unwrap().len(), 1);
        set_saved(&c, &e, false).unwrap();
        assert!(saved_episodes(&c).unwrap().is_empty());
    }

    #[test]
    fn new_episodes_only_from_follows_and_unplayed() {
        let c = test_conn();
        let mut s = Show::rss("S");
        s.title = "S".into();
        follow(&c, &s).unwrap();
        upsert_episodes(&c, &[ep("a", "S", 1000), ep("b", "S", 2000), ep("x", "Other", 3000)])
            .unwrap();
        set_played(&c, "b", true).unwrap();
        let n = new_episodes(&c, 500, 10).unwrap();
        assert_eq!(n.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert!(new_episodes(&c, 1500, 10).unwrap().is_empty());
    }

    #[test]
    fn caches_expire() {
        let c = test_conn();
        cache_put(&c, "k", "v");
        assert_eq!(cache_get(&c, "k", 60).as_deref(), Some("v"));
        assert_eq!(cache_get(&c, "nope", 60), None);
        sponsorblock_put(&c, "vid", "[]");
        assert_eq!(sponsorblock_get(&c, "vid", 60).as_deref(), Some("[]"));
        let mut st = feed_state(&c, "S");
        assert_eq!(st, FeedState::default());
        st.etag = Some("\"e\"".into());
        set_feed_state(&c, "S", &st).unwrap();
        assert_eq!(feed_state(&c, "S").etag.as_deref(), Some("\"e\""));
    }

    // stream: tilefix — a recently played YouTube show kept its signed `pl_c` art, which 404s
    // hours later: Home's shortcut tile showed a broken image on every launch.
    #[test]
    fn show_art_never_hands_out_a_dead_signed_url() {
        use base64::Engine;
        let signed = |at: i64| {
            let mut b = vec![0x08u8];
            let mut v = at as u64;
            loop {
                let byte = (v & 0x7f) as u8;
                v >>= 7;
                if v == 0 {
                    b.push(byte);
                    break;
                }
                b.push(byte | 0x80);
            }
            b.extend_from_slice(&[0xfa, 0x86, 0xaa]);
            let sqp = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
            format!("https://i.ytimg.com/pl_c/PLX/studio_square_thumbnail.jpg?sqp={sqp}&rs=AMzJ")
        };
        // The real URL from the bug report decodes to its issue time.
        assert_eq!(
            art_signed_at("https://i.ytimg.com/pl_c/PLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW/studio_square_thumbnail.jpg?sqp=CLq15dcG-oaymwEKCKAEEKAEIABIWqLzl_8DBgiR84OgBg&rs=AMzJL3kpyYOtkgEvOZJkYW5Yo5CQjDhz4A"),
            Some(1_794_726_586)
        );
        assert_eq!(art_signed_at("https://i.ytimg.com/vi/abc/hq720.jpg"), None);
        let c = test_conn();
        let dead = signed(now_secs() - 7 * 3600);
        let young = signed(now_secs() - 60);
        // Durable art and young signed art are kept as they are.
        assert_eq!(show_art(&c, "MPSPX", Some("https://a/art.jpg")).as_deref(), Some("https://a/art.jpg"));
        assert_eq!(show_art(&c, "MPSPX", Some(&young)), Some(young.clone()));
        // Dead, nothing better known: no URL (the UI draws its placeholder).
        assert_eq!(show_art(&c, "MPSPX", Some(&dead)), None);
        // The show's newest episode art (vi thumbnails don't expire).
        let mut old = ep("e1", "MPSPX", 100);
        old.thumbnail = Some("https://i.ytimg.com/vi/old/hq720.jpg".into());
        let mut new = ep("e2", "MPSPX", 200);
        new.thumbnail = Some("https://i.ytimg.com/vi/new/hq720.jpg".into());
        upsert_episodes(&c, &[old, new]).unwrap();
        assert_eq!(show_art(&c, "MPSPX", Some(&dead)).as_deref(), Some("https://i.ytimg.com/vi/new/hq720.jpg"));
        // A followed show's durable art beats episode art; its own dead signed art doesn't.
        let mut show = Show::youtube("MPSPX");
        show.title = "X".into();
        show.thumbnail = Some(dead.clone());
        follow(&c, &show).unwrap();
        assert_eq!(show_art(&c, "MPSPX", Some(&dead)).as_deref(), Some("https://i.ytimg.com/vi/new/hq720.jpg"));
        show.thumbnail = Some("https://feed/art.jpg".into());
        update_follow_meta(&c, "MPSPX", &show).unwrap();
        assert_eq!(show_art(&c, "MPSPX", Some(&dead)).as_deref(), Some("https://feed/art.jpg"));
        // Fresh art from the last show fetch wins while it lasts.
        remember_show_art(&c, "MPSPX", Some(&young));
        assert_eq!(show_art(&c, "MPSPX", Some(&dead)), Some(young));
    }
}
