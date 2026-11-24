//! SQLite side of downloads: what the user asked for (collections), which tracks that means
//! (members, refcounted), and each track's file and state. Every function takes a `&Connection`
//! so the queue rules are unit-tested on an in-memory database.
//!
//! A track is stored once however many downloaded collections hold it, and its file goes only
//! when the last one lets go (an *orphan*).

use std::collections::HashSet;

use innertube::SongItem;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub fn migrate(conn: &Connection) {
    let _ = conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS dl_tracks (
            video_id    TEXT PRIMARY KEY,
            song_json   TEXT NOT NULL,
            state       TEXT NOT NULL,           -- queued | downloading | done | failed
            file        TEXT,                    -- relative to the downloads folder
            art         TEXT,                    -- relative artwork file
            bytes       INTEGER NOT NULL DEFAULT 0,
            itag        INTEGER,
            loudness_db REAL,
            attempts    INTEGER NOT NULL DEFAULT 0,
            next_try_at INTEGER NOT NULL DEFAULT 0,
            error       TEXT,
            queued_at   INTEGER NOT NULL,
            done_at     INTEGER
        );
        CREATE TABLE IF NOT EXISTS dl_collections (
            id        TEXT PRIMARY KEY,
            kind      TEXT NOT NULL,             -- playlist | album | liked | episode | song
            title     TEXT NOT NULL,
            subtitle  TEXT,
            thumbnail TEXT,
            added_at  INTEGER NOT NULL,
            synced_at INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS dl_members (
            collection_id TEXT NOT NULL,
            video_id      TEXT NOT NULL,
            position      INTEGER NOT NULL,
            PRIMARY KEY (collection_id, video_id)
        );
        CREATE INDEX IF NOT EXISTS dl_members_video ON dl_members(video_id);
        "#,
    );
}

/// The thing a download button toggles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
}

/// How far along one collection is, for its button (ring / green arrow) and the library chip.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CollectionStatus {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub total: i64,
    pub done: i64,
    pub failed: i64,
    pub bytes: i64,
    pub added_at: i64,
}

/// A track nothing holds any more: its row is gone, its files are the caller's to delete.
#[derive(Debug, Clone, PartialEq)]
pub struct Orphan {
    pub video_id: String,
    pub file: Option<String>,
    pub art: Option<String>,
    pub was_done: bool,
}

/// One queued track for a worker.
#[derive(Debug, Clone)]
pub struct Job {
    pub video_id: String,
    pub song: SongItem,
    /// Failures so far (logged with the start).
    pub attempts: i64,
}

/// What finished downloading.
#[derive(Debug, Clone, Default)]
pub struct Finished {
    pub file: String,
    pub art: Option<String>,
    pub bytes: i64,
    pub itag: Option<i64>,
    pub loudness_db: Option<f64>,
}

/// Wait before retry number `attempts` (1-based count of failures so far), or `None` = give up.
/// 10 s, 30 s, 2 min, 10 min, 30 min, then failed (retried at the next launch or when the network
/// comes back). Metrolist and media3 settle on a handful of tries; spacing them out rides out a
/// cipher hiccup or a short YouTube refusal without hammering.
pub fn backoff(attempts: i64) -> Option<i64> {
    const STEPS: [i64; 5] = [10, 30, 120, 600, 1800];
    if attempts < 1 {
        return Some(0);
    }
    STEPS.get(attempts as usize - 1).copied()
}

pub fn upsert_collection(conn: &Connection, c: &Collection, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO dl_collections(id, kind, title, subtitle, thumbnail, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, subtitle = excluded.subtitle,
             thumbnail = COALESCE(excluded.thumbnail, dl_collections.thumbnail)",
        params![c.id, c.kind, c.title, c.subtitle, c.thumbnail, now],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn has_collection(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM dl_collections WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn collections(conn: &Connection) -> Vec<Collection> {
    let Ok(mut st) =
        conn.prepare("SELECT id, kind, title, subtitle, thumbnail FROM dl_collections ORDER BY added_at")
    else {
        return vec![];
    };
    st.query_map([], |r| {
        Ok(Collection {
            id: r.get(0)?,
            kind: r.get(1)?,
            title: r.get(2)?,
            subtitle: r.get(3)?,
            thumbnail: r.get(4)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

pub fn has_track(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM dl_tracks WHERE video_id = ?1", [id], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn members(conn: &Connection, id: &str) -> Vec<String> {
    let Ok(mut st) =
        conn.prepare("SELECT video_id FROM dl_members WHERE collection_id = ?1 ORDER BY position")
    else {
        return vec![];
    };
    st.query_map([id], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// Make `songs` the collection's tracks. New tracks are queued (a track already stored for
/// another collection is reused as is); tracks that left are dropped, and returned as orphans when
/// no other collection holds them. Returns `(newly queued ids, orphans)`.
pub fn set_members(
    conn: &Connection,
    id: &str,
    songs: &[SongItem],
    now: i64,
) -> rusqlite::Result<(Vec<String>, Vec<Orphan>)> {
    let tx = conn.unchecked_transaction()?;
    let mut seen = HashSet::new();
    let mut queued = Vec::new();
    tx.execute("DELETE FROM dl_members WHERE collection_id = ?1", [id])?;
    for (pos, s) in songs.iter().enumerate() {
        if s.video_id.is_empty() || !seen.insert(s.video_id.clone()) {
            continue;
        }
        tx.execute(
            "INSERT INTO dl_members(collection_id, video_id, position) VALUES (?1, ?2, ?3)",
            params![id, s.video_id, pos as i64],
        )?;
        let json = serde_json::to_string(s).unwrap_or_default();
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO dl_tracks(video_id, song_json, state, queued_at)
             VALUES (?1, ?2, 'queued', ?3)",
            params![s.video_id, json, now],
        )?;
        // Not inserted = already stored (for this or another collection): keep its file.
        if inserted > 0 {
            queued.push(s.video_id.clone());
        }
    }
    let orphans = take_orphans(&tx)?;
    tx.commit()?;
    Ok((queued, orphans))
}

/// Forget a collection; returns the tracks nothing else holds.
pub fn remove_collection(conn: &Connection, id: &str) -> rusqlite::Result<Vec<Orphan>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM dl_members WHERE collection_id = ?1", [id])?;
    tx.execute("DELETE FROM dl_collections WHERE id = ?1", [id])?;
    let o = take_orphans(&tx)?;
    tx.commit()?;
    Ok(o)
}

/// Forget everything; returns every track (all are orphans now).
pub fn remove_all(conn: &Connection) -> rusqlite::Result<Vec<Orphan>> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM dl_members", [])?;
    tx.execute("DELETE FROM dl_collections", [])?;
    let o = take_orphans(&tx)?;
    tx.commit()?;
    Ok(o)
}

fn take_orphans(conn: &Connection) -> rusqlite::Result<Vec<Orphan>> {
    let mut st = conn.prepare(
        "SELECT video_id, file, art, state FROM dl_tracks t
         WHERE NOT EXISTS (SELECT 1 FROM dl_members m WHERE m.video_id = t.video_id)",
    )?;
    let out: Vec<Orphan> = st
        .query_map([], |r| {
            Ok(Orphan {
                video_id: r.get(0)?,
                file: r.get(1)?,
                art: r.get(2)?,
                was_done: r.get::<_, String>(3)? == "done",
            })
        })?
        .flatten()
        .collect();
    conn.execute(
        "DELETE FROM dl_tracks WHERE NOT EXISTS
             (SELECT 1 FROM dl_members m WHERE m.video_id = dl_tracks.video_id)",
        [],
    )?;
    Ok(out)
}

/// The next tracks to start: queued, due, not already running. Oldest collection first, in each
/// collection's own order, so a playlist fills top-down like Spotify's.
pub fn next_jobs(
    conn: &Connection,
    now: i64,
    limit: usize,
    running: &HashSet<String>,
) -> Vec<Job> {
    if limit == 0 {
        return vec![];
    }
    let Ok(mut st) = conn.prepare(
        "SELECT t.video_id, t.song_json, t.attempts FROM dl_tracks t
         WHERE t.state = 'queued' AND t.next_try_at <= ?1
         ORDER BY t.queued_at,
             (SELECT MIN(c.added_at * 100000 + m.position) FROM dl_members m
              JOIN dl_collections c ON c.id = m.collection_id WHERE m.video_id = t.video_id),
             t.video_id",
    ) else {
        return vec![];
    };
    let rows = st.query_map([now], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    });
    let Ok(rows) = rows else { return vec![] };
    rows.flatten()
        .filter(|(id, _, _)| !running.contains(id))
        .filter_map(|(video_id, json, attempts)| {
            let song = serde_json::from_str(&json).ok()?;
            Some(Job { video_id, song, attempts })
        })
        .take(limit)
        .collect()
}

/// The earliest moment a waiting (backed-off) track becomes due, for the scheduler's sleep.
pub fn next_due(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT MIN(next_try_at) FROM dl_tracks WHERE state = 'queued'",
        [],
        |r| r.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

pub fn set_state(conn: &Connection, id: &str, state: &str) {
    let _ = conn.execute("UPDATE dl_tracks SET state = ?2 WHERE video_id = ?1", params![id, state]);
}

pub fn mark_done(conn: &Connection, id: &str, f: &Finished, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE dl_tracks SET state = 'done', file = ?2, art = ?3, bytes = ?4, itag = ?5,
             loudness_db = ?6, error = NULL, done_at = ?7
         WHERE video_id = ?1",
        params![id, f.file, f.art, f.bytes, f.itag, f.loudness_db, now],
    )?;
    Ok(n > 0)
}

/// Count a failure. Returns true when the track is given up on (`failed`).
pub fn mark_retry(conn: &Connection, id: &str, error: &str, now: i64) -> bool {
    let attempts: i64 = conn
        .query_row("SELECT attempts FROM dl_tracks WHERE video_id = ?1", [id], |r| r.get(0))
        .unwrap_or(0)
        + 1;
    let (state, next) = match backoff(attempts) {
        Some(wait) => ("queued", now + wait),
        None => ("failed", 0),
    };
    let _ = conn.execute(
        "UPDATE dl_tracks SET state = ?2, attempts = ?3, next_try_at = ?4, error = ?5
         WHERE video_id = ?1",
        params![id, state, attempts, next, error],
    );
    state == "failed"
}

/// Back in line without counting a failure (network gone, metered, app quitting).
pub fn requeue(conn: &Connection, id: &str) {
    let _ = conn.execute(
        "UPDATE dl_tracks SET state = 'queued' WHERE video_id = ?1 AND state = 'downloading'",
        [id],
    );
}

/// At launch: what was mid-download resumes from its `.part`; failed tracks get a fresh go.
pub fn reset_on_launch(conn: &Connection) {
    let _ = conn.execute("UPDATE dl_tracks SET state = 'queued' WHERE state = 'downloading'", []);
    retry_failed(conn);
}

/// "Retry" (or the network coming back): failed tracks start over with a clean count.
pub fn retry_failed(conn: &Connection) -> usize {
    conn.execute(
        "UPDATE dl_tracks SET state = 'queued', attempts = 0, next_try_at = 0, error = NULL
         WHERE state = 'failed'",
        [],
    )
    .unwrap_or(0)
}

/// Every finished track: `(video_id, file, loudness_db, art)`.
pub fn done_index(conn: &Connection) -> Vec<(String, String, Option<f64>, Option<String>)> {
    let Ok(mut st) = conn.prepare(
        "SELECT video_id, file, loudness_db, art FROM dl_tracks WHERE state = 'done' AND file IS NOT NULL",
    ) else {
        return vec![];
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// A finished track whose file vanished goes back in the queue.
pub fn mark_missing(conn: &Connection, id: &str) {
    let _ = conn.execute(
        "UPDATE dl_tracks SET state = 'queued', file = NULL, attempts = 0, next_try_at = 0
         WHERE video_id = ?1",
        [id],
    );
}

pub fn statuses(conn: &Connection) -> Vec<CollectionStatus> {
    let Ok(mut st) = conn.prepare(
        "SELECT c.id, c.kind, c.title, c.subtitle, c.thumbnail, c.added_at,
             COUNT(m.video_id),
             COALESCE(SUM(t.state = 'done'), 0),
             COALESCE(SUM(t.state = 'failed'), 0),
             COALESCE(SUM(CASE WHEN t.state = 'done' THEN t.bytes ELSE 0 END), 0)
         FROM dl_collections c
         LEFT JOIN dl_members m ON m.collection_id = c.id
         LEFT JOIN dl_tracks t ON t.video_id = m.video_id
         GROUP BY c.id ORDER BY c.added_at",
    ) else {
        return vec![];
    };
    st.query_map([], |r| {
        Ok(CollectionStatus {
            id: r.get(0)?,
            kind: r.get(1)?,
            title: r.get(2)?,
            subtitle: r.get(3)?,
            thumbnail: r.get(4)?,
            added_at: r.get(5)?,
            total: r.get(6)?,
            done: r.get(7)?,
            failed: r.get(8)?,
            bytes: r.get(9)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// `(done tracks, their bytes, queued, failed)`.
pub fn totals(conn: &Connection) -> (i64, i64, i64, i64) {
    conn.query_row(
        "SELECT COALESCE(SUM(state = 'done'), 0),
                COALESCE(SUM(CASE WHEN state = 'done' THEN bytes ELSE 0 END), 0),
                COALESCE(SUM(state IN ('queued', 'downloading')), 0),
                COALESCE(SUM(state = 'failed'), 0)
         FROM dl_tracks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap_or((0, 0, 0, 0))
}

pub fn song(conn: &Connection, id: &str) -> Option<SongItem> {
    conn.query_row("SELECT song_json FROM dl_tracks WHERE video_id = ?1", [id], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|j| serde_json::from_str(&j).ok())
}

/// Every downloaded song, newest first (the "Downloaded" view of a collection that is gone, and
/// the offline queue).
pub fn done_songs(conn: &Connection) -> Vec<SongItem> {
    let Ok(mut st) = conn
        .prepare("SELECT song_json FROM dl_tracks WHERE state = 'done' ORDER BY done_at DESC")
    else {
        return vec![];
    };
    st.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|j| serde_json::from_str(&j).ok()).collect())
        .unwrap_or_default()
}

/// The downloaded tracks of one collection, in its order: the page an album or playlist shows
/// when it can't be fetched (offline; the iOS app's entity pages).
pub fn collection_songs(conn: &Connection, id: &str) -> Vec<SongItem> {
    let Ok(mut st) = conn.prepare(
        "SELECT t.song_json FROM dl_members m JOIN dl_tracks t ON t.video_id = m.video_id
         WHERE m.collection_id = ?1 AND t.state = 'done' ORDER BY m.position",
    ) else {
        return vec![];
    };
    st.query_map([id], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().filter_map(|j| serde_json::from_str(&j).ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c);
        c
    }
    fn s(id: &str) -> SongItem {
        SongItem { video_id: id.into(), title: id.into(), ..Default::default() }
    }
    fn coll(id: &str, kind: &str) -> Collection {
        Collection { id: id.into(), kind: kind.into(), title: id.into(), subtitle: None, thumbnail: None }
    }
    fn fin(file: &str) -> Finished {
        Finished { file: file.into(), bytes: 100, ..Default::default() }
    }

    #[test]
    fn backoff_steps_then_gives_up() {
        assert_eq!(backoff(0), Some(0));
        assert_eq!(backoff(1), Some(10));
        assert_eq!(backoff(5), Some(1800));
        assert_eq!(backoff(6), None);
    }

    #[test]
    fn adding_queues_each_track_once_and_shares_across_collections() {
        let c = db();
        upsert_collection(&c, &coll("A", "playlist"), 1).unwrap();
        let (q, o) = set_members(&c, "A", &[s("x"), s("y"), s("x")], 1).unwrap();
        assert_eq!(q, vec!["x", "y"]);
        assert!(o.is_empty());
        upsert_collection(&c, &coll("B", "album"), 2).unwrap();
        let (q, _) = set_members(&c, "B", &[s("y"), s("z")], 2).unwrap();
        assert_eq!(q, vec!["z"], "y is already stored for A");
        assert_eq!(totals(&c).2, 3);
    }

    #[test]
    fn sync_diff_queues_new_and_orphans_removed_unless_shared() {
        let c = db();
        upsert_collection(&c, &coll("A", "playlist"), 1).unwrap();
        upsert_collection(&c, &coll("B", "playlist"), 1).unwrap();
        set_members(&c, "A", &[s("x"), s("y")], 1).unwrap();
        set_members(&c, "B", &[s("y")], 1).unwrap();
        mark_done(&c, "x", &fin("tracks/x.webm"), 5).unwrap();
        let (q, o) = set_members(&c, "A", &[s("w")], 9).unwrap();
        assert_eq!(q, vec!["w"]);
        assert_eq!(o.len(), 1, "y stays: B holds it");
        assert_eq!(o[0].video_id, "x");
        assert_eq!(o[0].file.as_deref(), Some("tracks/x.webm"));
        assert!(o[0].was_done);
        let o = remove_collection(&c, "B").unwrap();
        assert_eq!(o.iter().map(|o| o.video_id.as_str()).collect::<Vec<_>>(), vec!["y"]);
        assert!(!has_collection(&c, "B"));
        assert_eq!(remove_all(&c).unwrap().len(), 1);
        assert_eq!(totals(&c), (0, 0, 0, 0));
    }

    #[test]
    fn jobs_come_in_collection_order_skip_running_and_wait_for_backoff() {
        let c = db();
        upsert_collection(&c, &coll("A", "playlist"), 1).unwrap();
        set_members(&c, "A", &[s("a1"), s("a2"), s("a3")], 1).unwrap();
        let running: HashSet<String> = ["a1".to_string()].into();
        let ids: Vec<_> = next_jobs(&c, 1, 2, &running).into_iter().map(|j| j.video_id).collect();
        assert_eq!(ids, vec!["a2", "a3"]);
        assert!(!mark_retry(&c, "a2", "boom", 100));
        let ids: Vec<_> = next_jobs(&c, 105, 5, &HashSet::new()).into_iter().map(|j| j.video_id).collect();
        assert_eq!(ids, vec!["a1", "a3"], "a2 waits out its 10 s");
        assert_eq!(next_due(&c), Some(0));
        let ids: Vec<_> = next_jobs(&c, 111, 5, &HashSet::new()).into_iter().map(|j| j.video_id).collect();
        assert!(ids.contains(&"a2".to_string()));
        assert_eq!(next_jobs(&c, 111, 0, &HashSet::new()).len(), 0);
    }

    #[test]
    fn retries_exhaust_to_failed_and_reset_on_launch() {
        let c = db();
        upsert_collection(&c, &coll("A", "playlist"), 1).unwrap();
        set_members(&c, "A", &[s("a")], 1).unwrap();
        let mut gave_up = false;
        for i in 0..6 {
            gave_up = mark_retry(&c, "a", "nope", i);
        }
        assert!(gave_up);
        assert_eq!(totals(&c).3, 1);
        assert_eq!(statuses(&c)[0].failed, 1);
        set_state(&c, "a", "downloading");
        reset_on_launch(&c);
        assert_eq!(totals(&c), (0, 0, 1, 0));
        // a crashed download resumes
        set_state(&c, "a", "downloading");
        reset_on_launch(&c);
        assert_eq!(next_jobs(&c, 0, 3, &HashSet::new()).len(), 1);
    }

    #[test]
    fn status_counts_done_and_bytes_and_index_lists_files() {
        let c = db();
        upsert_collection(&c, &coll("L", "liked"), 1).unwrap();
        set_members(&c, "L", &[s("a"), s("b")], 1).unwrap();
        mark_done(&c, "a", &Finished { loudness_db: Some(-3.5), ..fin("tracks/a.m4a") }, 2).unwrap();
        let st = &statuses(&c)[0];
        assert_eq!((st.total, st.done, st.bytes), (2, 1, 100));
        let idx = done_index(&c);
        assert_eq!(idx, vec![("a".into(), "tracks/a.m4a".into(), Some(-3.5), None)]);
        assert_eq!(done_songs(&c)[0].video_id, "a");
        mark_missing(&c, "a");
        assert!(done_index(&c).is_empty());
        requeue(&c, "b");
        assert_eq!(song(&c, "b").unwrap().title, "b");
    }

    #[test]
    fn collection_songs_are_the_done_members_in_order() {
        let c = db();
        upsert_collection(&c, &coll("A", "album"), 1).unwrap();
        set_members(&c, "A", &[s("x"), s("y"), s("z")], 1).unwrap();
        let fin = Finished { file: "f".into(), art: None, bytes: 1, itag: None, loudness_db: None };
        mark_done(&c, "z", &fin, 2).unwrap();
        mark_done(&c, "x", &fin, 3).unwrap();
        let ids: Vec<_> = collection_songs(&c, "A").into_iter().map(|s| s.video_id).collect();
        assert_eq!(ids, ["x", "z"], "only finished tracks, in the collection's order");
        assert!(collection_songs(&c, "B").is_empty());
    }
}
