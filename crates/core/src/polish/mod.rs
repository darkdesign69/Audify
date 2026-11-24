//! Backend polish (stream: polish): the gaps left between the feature streams on the way to
//! NOW.md parity.
//!
//! - [`context`]: the playback context (playlist, album, artist, Liked Songs, local playlist,
//!   podcast show, radio/mix) recorded on every listen, feeding Recents and Home's shortcuts.
//! - [`explicit`]: with the explicit filter on, explicit rows are skipped however they were queued.
//! - [`podmatch`]: an RSS-only podcast episode plays its ad-free YouTube twin when one exists.
//! - [`health`]: a quick "does extraction still work" probe for the "playback is broken" banner.
//! - [`explore`]: charts, new releases and sectioned genre/mood pages (both bridges).

use std::sync::{Arc, OnceLock};

use crate::db::Db;

pub mod context;
pub mod explicit;
pub mod explore;
pub mod health;
pub mod podmatch;

static DB: OnceLock<Arc<Db>> = OnceLock::new();

/// Called once at startup (lib.rs setup), after `smart::init` and `podcasts::init`.
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
            -- What a queue was started from, keyed by the queue's own (source_id, source_name),
            -- so a listen can name its context even after a restart restored the queue.
            CREATE TABLE IF NOT EXISTS polish_contexts (
                key  TEXT PRIMARY KEY,
                json TEXT NOT NULL,
                at   INTEGER NOT NULL
            );
            -- One row per listen (past the listen threshold) that had a context.
            -- `context_key` is `<kind>:<id>`; `json` the context card at the time.
            CREATE TABLE IF NOT EXISTS polish_listen_contexts (
                id          INTEGER PRIMARY KEY,
                video_id    TEXT NOT NULL,
                context_key TEXT NOT NULL,
                json        TEXT NOT NULL,
                at          INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS polish_listen_contexts_key ON polish_listen_contexts(context_key);
            CREATE INDEX IF NOT EXISTS polish_listen_contexts_video ON polish_listen_contexts(video_id);
            -- RSS episode id (PODURL:…) -> its YouTube twin. `video_id` NULL = looked, none found.
            CREATE TABLE IF NOT EXISTS polish_pod_twins (
                id         TEXT PRIMARY KEY,
                video_id   TEXT,
                checked_at INTEGER NOT NULL
            );
            "#,
        );
        if let Err(e) = r {
            tracing::warn!(error = %e, "polish migrate failed");
        }
    });
}

pub fn now_secs() -> i64 {
    crate::db::now_secs()
}

#[cfg(test)]
pub fn test_db() -> Db {
    let db = Db::open(std::path::Path::new(":memory:")).unwrap();
    crate::smart::migrate(&db);
    db.with_conn(crate::library::migrate);
    migrate(&db);
    db
}
