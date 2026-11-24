//! Stream D, the smart layer (research 07, 22): shuffle, taste, recommendations, personal Home,
//! playlist suggestions, explicit filter and the blocklist/private-session checks every generator
//! goes through.
//!
//! Everything here is local and anonymous. YouTube Music supplies candidates (radio, related,
//! artist pages, charts); this layer only filters and re-ranks them.
//!
//! ## Interfaces other streams own (reconcile at merge)
//!
//! Stream B (local library) owns likes, the blocklist and the private-session setting. To avoid
//! blocking on it, [`library`] reads them through a small interface with a temporary local
//! implementation over tables of the agreed simple shape, created here with `IF NOT EXISTS`:
//!
//! - `blocked_tracks(video_id TEXT PRIMARY KEY, title TEXT, artist TEXT, blocked_at INTEGER)`
//! - `blocked_artists(artist_id TEXT PRIMARY KEY, name TEXT, blocked_at INTEGER)` (`artist_id` is
//!   the channel id, or `name:<normalised name>` when there is none)
//! - `liked_tracks(video_id TEXT PRIMARY KEY, song_json TEXT, liked_at INTEGER)`
//! - setting `private_session` = `"true"`
//!
//! If B's tables use other names or columns, only the queries in `library.rs` change.

pub mod explicit;
pub mod home;
pub mod library;
pub mod mfy;
pub mod podhome; // stream: home-tiles-pods — Home's Podcasts chip
pub mod recs;
pub mod shuffle;
pub mod smartshuffle;
pub mod tags; // lane magic-d3: Liked Songs filter chips
pub mod taste;
pub mod whatsnew;

use std::sync::{Arc, OnceLock};

use crate::db::Db;

static DB: OnceLock<Arc<Db>> = OnceLock::new();

/// Called once at startup (lib.rs setup) so code without an `AppState` at hand (the queue's free
/// shuffle function) can read settings and recent plays. Also creates this layer's tables.
pub fn init(db: Arc<Db>) {
    migrate(&db);
    let _ = DB.set(db);
}

/// The database [`init`] was given; `None` in unit tests that never call it.
pub fn db() -> Option<&'static Arc<Db>> {
    DB.get()
}

pub fn migrate(db: &Db) {
    db.with_conn(|c| {
        let r = c.execute_batch(
            r#"
            -- One row per finished play (any length), so skips under the listen threshold are
            -- recorded too. `kind`: skip | partial | complete. `from_rec`: the track came from
            -- autoplay/radio. `private`: played in a private session; never feeds taste.
            CREATE TABLE IF NOT EXISTS smart_events (
                id         INTEGER PRIMARY KEY,
                video_id   TEXT NOT NULL,
                artist_key TEXT,
                artist     TEXT,
                kind       TEXT NOT NULL,
                secs       REAL NOT NULL,
                duration   REAL,
                from_rec   INTEGER NOT NULL DEFAULT 0,
                private    INTEGER NOT NULL DEFAULT 0,
                at         INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS smart_events_at ON smart_events(at);
            -- `listens` rows heard in a private session: kept for history, left out of taste.
            CREATE TABLE IF NOT EXISTS smart_private_listens (listen_id INTEGER PRIMARY KEY);
            -- Explicit flags learned from YouTube badges or Deezer, by videoId.
            CREATE TABLE IF NOT EXISTS smart_explicit (
                video_id TEXT PRIMARY KEY,
                explicit INTEGER,
                source   TEXT NOT NULL,
                at       INTEGER NOT NULL
            );
            -- Network answers this layer caches (artist pages, radios, home shelves).
            CREATE TABLE IF NOT EXISTS smart_cache (
                key  TEXT PRIMARY KEY,
                json TEXT NOT NULL,
                at   INTEGER NOT NULL
            );
            -- Temporary shapes of Stream B's tables (see the module docs).
            CREATE TABLE IF NOT EXISTS blocked_tracks (
                video_id   TEXT PRIMARY KEY,
                title      TEXT,
                artist     TEXT,
                blocked_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS blocked_artists (
                artist_id  TEXT PRIMARY KEY,
                name       TEXT,
                blocked_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS liked_tracks (
                video_id TEXT PRIMARY KEY,
                song_json TEXT,
                liked_at INTEGER
            );
            "#,
        );
        if let Err(e) = r {
            tracing::warn!(error = %e, "smart: creating tables failed");
        }
    });
    // ── stream: smart2 ──
    mfy::migrate(db);
    whatsnew::migrate(db);
    tags::migrate(db); // lane magic-d3
}

pub fn now_secs() -> i64 {
    crate::db::now_secs()
}

/// Settings this layer reads. All default to the behaviour Spotify ships with.
pub mod settings {
    use crate::db::Db;

    /// `"smart"` (default: artist-spread + fewer repeats) or `"standard"` (plain random).
    pub const SHUFFLE_MODE: &str = "shuffle_mode";
    /// `"true"`: explicit tracks are dimmed in the UI and skipped in playback/radio.
    pub const EXPLICIT_FILTER: &str = "explicit_filter";
    /// `"true"`: plays don't feed taste. Owned by Stream B; read here only.
    pub const PRIVATE_SESSION: &str = "private_session";

    pub fn smart_shuffle(db: &Db) -> bool {
        db.get_setting(SHUFFLE_MODE).as_deref() != Some("standard")
    }

    pub fn explicit_filter(db: &Db) -> bool {
        db.get_setting(EXPLICIT_FILTER).as_deref() == Some("true")
    }

    pub fn private_session(db: &Db) -> bool {
        crate::library::is_private(db)
    }
}

/// Read a cached JSON answer no older than `ttl` seconds.
pub fn cache_get<T: serde::de::DeserializeOwned>(db: &Db, key: &str, ttl: i64) -> Option<T> {
    let row: Option<(String, i64)> = db.with_conn(|c| {
        c.query_row("SELECT json, at FROM smart_cache WHERE key = ?1", [key], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok()
    });
    let (json, at) = row?;
    if now_secs() - at > ttl {
        return None;
    }
    serde_json::from_str(&json).ok()
}

pub fn cache_put<T: serde::Serialize>(db: &Db, key: &str, value: &T) {
    let Ok(json) = serde_json::to_string(value) else { return };
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_cache(key, json, at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET json = excluded.json, at = excluded.at",
            rusqlite::params![key, json, now_secs()],
        );
    });
}

#[cfg(test)]
pub fn test_db() -> Db {
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    migrate(&db);
    db
}
