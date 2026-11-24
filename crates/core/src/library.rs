//! Encore's local library (Stream B). Everything here works with no account at all: liked songs,
//! saved albums and playlists, followed artists, the user's own playlists, folders, pins, the
//! "last played" stamps the Recents sort reads, recently played, hidden songs and the private
//! session flag. One SQLite file (the app's own `Db`) is the single source of truth; the YouTube
//! account, when there is one, is at most a mirror.
//!
//! Layout:
//! - `migrate` creates the tables and runs the one-shot schema upgrades. Called from `Db::open`.
//! - Core functions take a `&Connection` so the tests run against an in-memory database.
//! - `lib_*` Tauri commands at the bottom wrap them for the UI (`ui/src/lib/library.svelte.ts`).
//!
//! Undo: every mutation is idempotent and has an inverse (see `docs/streams/library.md`). The
//! destructive ones hand back what they removed, with positions, so the UI can put it back.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use innertube::{BrowseItem, PlaylistPage, SongItem};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::host::HostExt;

use crate::db::{now_secs, Db};
use crate::state::{AppState, LOCAL_PLAYLIST_PREFIX};

/// Liked Songs' id everywhere in the UI. The same id YouTube gives Liked Music, so every surface
/// that already routes `/playlist/VLLM` keeps working; `get_playlist` answers it from SQLite.
pub const LIKED_ID: &str = "VLLM";
pub const MAX_PINS: usize = 4;

const SCHEMA_KEY: &str = "library_schema";
const SCHEMA_VERSION: i64 = 1;
const LEGACY_MIGRATED_KEY: &str = "library_legacy_migrated";
const LEGACY_BACKUP_KEY: &str = "library_legacy_backup";
const PRIVATE_KEY: &str = "private_session_since";
/// Spotify ends a private session after six hours (and on restart, which we don't copy: a session
/// that survives a relaunch is the safer surprise).
const PRIVATE_TTL_SECS: i64 = 6 * 3600;
/// Kinds `library_items` holds. Podcasts have their own table (`podcast_follows`, Stream E).
const SAVED_KINDS: [&str; 3] = ["album", "playlist", "artist"];

pub type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ─── schema ──────────────────────────────────────────────────────────────────────────────────────

const SCHEMA: &str = r#"
-- Liked Songs. A full snapshot of the track, so the list opens offline and exports completely.
CREATE TABLE IF NOT EXISTS liked_songs (
    video_id      TEXT PRIMARY KEY,
    title         TEXT NOT NULL,
    artists       TEXT NOT NULL,
    artists_json  TEXT NOT NULL DEFAULT '[]',
    album         TEXT,
    album_id      TEXT,
    duration_secs INTEGER,
    thumbnail     TEXT,
    explicit      INTEGER NOT NULL DEFAULT 0,
    song_json     TEXT NOT NULL,
    added_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS liked_songs_added ON liked_songs(added_at);
-- Saved albums, saved (YouTube) playlists and followed artists. `card_json` is the BrowseItem
-- card as it was on screen, so the library renders offline. `synced`: also on the YouTube account.
CREATE TABLE IF NOT EXISTS library_items (
    id        TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    title     TEXT NOT NULL,
    subtitle  TEXT,
    thumbnail TEXT,
    card_json TEXT NOT NULL,
    synced    INTEGER NOT NULL DEFAULT 0,
    added_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS library_items_kind ON library_items(kind, added_at);
-- Followed podcasts. Owned by Stream E (podcasts); created here so the library has one schema.
CREATE TABLE IF NOT EXISTS podcast_follows (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    author       TEXT,
    thumb        TEXT,
    feed_url     TEXT,
    yt_browse_id TEXT,
    followed_at  INTEGER NOT NULL
);
-- Folders nest (parent_id) and are ordered among their siblings (position).
CREATE TABLE IF NOT EXISTS lib_folders (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    parent_id  TEXT,
    position   INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
-- Which folder a library item (a playlist id) sits in, and where. Not listed = at the root.
CREATE TABLE IF NOT EXISTS lib_folder_items (
    item_id   TEXT PRIMARY KEY,
    folder_id TEXT NOT NULL,
    position  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS lib_pins (
    item_id   TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    position  INTEGER NOT NULL,
    pinned_at INTEGER NOT NULL
);
-- When each library item (playlist, album, artist, Liked Songs, folder) was last played from:
-- the sidebar's Recents sort and the "recently played" contexts.
CREATE TABLE IF NOT EXISTS lib_played (
    item_id   TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    card_json TEXT,
    played_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS lib_played_at ON lib_played(played_at);
-- When each library item was last added to or edited (a song added to a playlist, a rename, a
-- like for Liked Songs). Spotify's Recents sort moves an item on these as well as on plays.
CREATE TABLE IF NOT EXISTS lib_touched (
    item_id    TEXT PRIMARY KEY,
    touched_at INTEGER NOT NULL
);
-- Hidden songs (Spotify's "Hide song"). `context_id` '' = everywhere, else just that playlist.
-- Hidden artists stay in blocked.rs's list, which the innertube parser already filters on.
CREATE TABLE IF NOT EXISTS hidden_songs (
    video_id   TEXT NOT NULL,
    context_id TEXT NOT NULL DEFAULT '',
    song_json  TEXT,
    hidden_at  INTEGER NOT NULL,
    PRIMARY KEY (video_id, context_id)
);
"#;

/// Create the library tables and run the one-shot upgrades. Called from `Db::open` on every
/// launch. Each upgrade is one transaction, so a crash halfway retries next launch.
pub fn migrate(conn: &Connection) {
    if let Err(e) = try_migrate(conn) {
        tracing::warn!(error = %e, "library migration failed, will retry next launch");
    }
    purge_deleted(conn);
}

fn get_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()
        .ok()?
}

fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )
    .map(|_| ())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else { return false };
    let names = stmt.query_map([], |r| r.get::<_, String>(1));
    names.map(|rows| rows.flatten().any(|n| n == column)).unwrap_or(false)
}

fn try_migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    let version: i64 = get_setting(conn, SCHEMA_KEY).and_then(|v| v.parse().ok()).unwrap_or(0);
    if version < 1 {
        let tx = conn.unchecked_transaction()?;
        // Soft delete, so "Delete playlist" can be undone for the rest of the session.
        if !has_column(&tx, "local_playlists", "deleted_at") {
            tx.execute("ALTER TABLE local_playlists ADD COLUMN deleted_at INTEGER", [])?;
        }
        // Limusic's UNIQUE(playlist_id, video_id) made "Add anyway" impossible (LIMUSIC-MAP §4
        // bug 9). Rebuild the table without it; duplicates are now a warning, not a wall. Row ids
        // carry over (they are the `set_video_id` handles the UI holds), and so does the
        // AUTOINCREMENT high-water mark, so a removed row's id is never handed out again.
        let seq: i64 = tx
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'local_playlist_tracks'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        tx.execute_batch(
            "CREATE TABLE local_playlist_tracks_v2 (
                 id          INTEGER PRIMARY KEY AUTOINCREMENT,
                 playlist_id INTEGER NOT NULL,
                 video_id    TEXT NOT NULL,
                 song_json   TEXT NOT NULL,
                 added_at    INTEGER NOT NULL,
                 pos         INTEGER
             );
             INSERT INTO local_playlist_tracks_v2(id, playlist_id, video_id, song_json, added_at, pos)
                 SELECT id, playlist_id, video_id, song_json, added_at, COALESCE(pos, id)
                 FROM local_playlist_tracks;
             DROP TABLE local_playlist_tracks;
             ALTER TABLE local_playlist_tracks_v2 RENAME TO local_playlist_tracks;
             CREATE INDEX IF NOT EXISTS local_playlist_tracks_video
                 ON local_playlist_tracks(video_id);
             CREATE INDEX IF NOT EXISTS local_playlist_tracks_list
                 ON local_playlist_tracks(playlist_id, pos);",
        )?;
        if seq > 0 {
            let n = tx.execute(
                "UPDATE sqlite_sequence SET seq = MAX(seq, ?1) WHERE name = 'local_playlist_tracks'",
                [seq],
            )?;
            if n == 0 {
                tx.execute(
                    "INSERT INTO sqlite_sequence(name, seq) VALUES('local_playlist_tracks', ?1)",
                    [seq],
                )?;
            }
        }
        set_setting(&tx, SCHEMA_KEY, &SCHEMA_VERSION.to_string())?;
        tx.commit()?;
    }
    Ok(())
}

/// stream: magic-d4 — how long a deleted playlist stays recoverable (Settings › Recover
/// playlists). Spotify keeps them 90 days on its account page; 30 is plenty on one machine.
pub const DELETED_KEEP_SECS: i64 = 30 * 86_400;

/// Playlists deleted more than [`DELETED_KEEP_SECS`] ago are gone for good: their rows, their
/// cover file, and anything still pointing at them (pins, folder slots, Recents). Runs at launch.
fn purge_deleted(conn: &Connection) {
    purge_deleted_before(conn, now_secs() - DELETED_KEEP_SECS);
}

fn purge_deleted_before(conn: &Connection, cutoff: i64) {
    if !has_column(conn, "local_playlists", "deleted_at") {
        return;
    }
    let ids: Vec<i64> = all(
        conn,
        "SELECT id FROM local_playlists WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
        [cutoff],
        |r| r.get(0),
    );
    for id in ids {
        let pid = local_id(id);
        if let Some(cover) = get_setting(conn, &cover_key(&pid)) {
            let _ = std::fs::remove_file(cover);
        }
        let _ = conn.execute("DELETE FROM settings WHERE key = ?1", [cover_key(&pid)]);
        let _ = conn.execute("DELETE FROM local_playlist_tracks WHERE playlist_id = ?1", [id]);
        let _ = conn.execute("DELETE FROM local_playlists WHERE id = ?1", [id]);
        forget_item(conn, &pid);
    }
}

/// Drop every pointer at a library item that no longer exists.
fn forget_item(conn: &Connection, item_id: &str) {
    for sql in [
        "DELETE FROM lib_pins WHERE item_id = ?1",
        "DELETE FROM lib_folder_items WHERE item_id = ?1",
        "DELETE FROM lib_played WHERE item_id = ?1",
        "DELETE FROM lib_touched WHERE item_id = ?1",
    ] {
        let _ = conn.execute(sql, [item_id]);
    }
    renumber_pins(conn);
}

fn all<T>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
    map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Vec<T> {
    let Ok(mut stmt) = conn.prepare(sql) else { return Vec::new() };
    let out = stmt.query_map(args, map).map(|rows| rows.flatten().collect()).unwrap_or_default();
    out
}

fn try_all<T>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
    map: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(args, map)?;
    rows.collect()
}

// ─── helpers ─────────────────────────────────────────────────────────────────────────────────────

pub fn local_id(n: i64) -> String {
    format!("{LOCAL_PLAYLIST_PREFIX}{n}")
}

fn local_key(id: &str) -> R<i64> {
    id.strip_prefix(LOCAL_PLAYLIST_PREFIX)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| "not_found".to_string())
}

/// Same key `commands::cover_key` writes a picked playlist cover under.
fn cover_key(playlist_id: &str) -> String {
    format!("playlist_cover:{}", playlist_id.strip_prefix("VL").unwrap_or(playlist_id))
}

/// "3:47" / "1:02:03" → seconds.
pub fn duration_secs(d: Option<&str>) -> Option<i64> {
    let d = d?.trim();
    if d.is_empty() {
        return None;
    }
    let mut total = 0i64;
    for part in d.split(':') {
        total = total * 60 + part.trim().parse::<i64>().ok()?;
    }
    Some(total)
}

/// What a track keeps once it is in the library: the song, none of the queue or playlist context
/// it was added from, and no snapshot of account state that would go stale.
fn clean(song: SongItem) -> SongItem {
    crate::api::playlist_row(SongItem { added_at: None, set_video_id: None, ..song })
}

/// The kind of a library id, for rows (pins, recents) migrated with no card to say.
fn kind_of(id: &str) -> &'static str {
    if id == LIKED_ID {
        "liked"
    } else if id.starts_with(LOCAL_PLAYLIST_PREFIX) || id.starts_with("VL") {
        "playlist"
    } else if id.starts_with("MPRE") {
        "album"
    } else if id.starts_with("UC") {
        "artist"
    } else if id.starts_with("FOLDER:") {
        "folder"
    } else if id.starts_with("MPSP") {
        "podcast"
    } else {
        "playlist"
    }
}

// ─── liked songs ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LikedEntry {
    /// The track snapshot (`SongItem`, snake_case like everywhere else). `added_at` is set too.
    pub song: SongItem,
    pub added_at: i64,
}

fn insert_like(conn: &Connection, song: &SongItem, added_at: i64) -> rusqlite::Result<bool> {
    let exists: bool = conn
        .query_row("SELECT 1 FROM liked_songs WHERE video_id = ?1", [&song.video_id], |_| Ok(()))
        .optional()?
        .is_some();
    let json = serde_json::to_string(song).unwrap_or_else(|_| "{}".into());
    let runs = serde_json::to_string(&song.artist_runs).unwrap_or_else(|_| "[]".into());
    // A re-like refreshes the snapshot but keeps the date it was first liked: idempotent.
    conn.execute(
        "INSERT INTO liked_songs(video_id, title, artists, artists_json, album, album_id,
             duration_secs, thumbnail, explicit, song_json, added_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(video_id) DO UPDATE SET title = excluded.title, artists = excluded.artists,
             artists_json = excluded.artists_json, album = excluded.album,
             album_id = excluded.album_id, duration_secs = excluded.duration_secs,
             thumbnail = excluded.thumbnail, explicit = excluded.explicit,
             song_json = excluded.song_json",
        params![
            song.video_id,
            song.title,
            song.artists,
            runs,
            song.album,
            song.album_id,
            duration_secs(song.duration.as_deref()),
            song.thumbnail,
            song.explicit,
            json,
            added_at
        ],
    )?;
    Ok(!exists)
}

/// Like songs. Answers per song whether it is newly liked. A bulk like stamps them one second
/// apart, the first newest, so "newest first" keeps the order they were given in.
pub fn like(conn: &Connection, songs: Vec<SongItem>, now: i64) -> rusqlite::Result<Vec<bool>> {
    let tx = conn.unchecked_transaction()?;
    let mut out = Vec::with_capacity(songs.len());
    for (i, song) in songs.into_iter().enumerate() {
        if song.video_id.is_empty() {
            out.push(false);
            continue;
        }
        out.push(insert_like(&tx, &clean(song), now - i as i64)?);
    }
    tx.commit()?;
    Ok(out)
}

/// Put entries back with their original dates (the inverse of [`unlike`]).
pub fn restore_likes(conn: &Connection, entries: Vec<LikedEntry>) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for e in entries {
        // Overwrite the date too: this is an undo, the original date is the truth.
        tx.execute("DELETE FROM liked_songs WHERE video_id = ?1", [&e.song.video_id])?;
        insert_like(&tx, &clean(e.song), e.added_at)?;
    }
    tx.commit()
}

/// Unlike. Answers what was removed, ready for [`restore_likes`].
pub fn unlike(conn: &Connection, video_ids: &[String]) -> rusqlite::Result<Vec<LikedEntry>> {
    let tx = conn.unchecked_transaction()?;
    let mut removed = Vec::new();
    for id in video_ids {
        let row: Option<(String, i64)> = tx
            .query_row(
                "SELECT song_json, added_at FROM liked_songs WHERE video_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((json, added_at)) = row {
            tx.execute("DELETE FROM liked_songs WHERE video_id = ?1", [id])?;
            if let Ok(song) = serde_json::from_str::<SongItem>(&json) {
                removed.push(LikedEntry {
                    song: SongItem { added_at: Some(added_at), ..song },
                    added_at,
                });
            }
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Liked Songs, newest first.
pub fn liked_songs(conn: &Connection) -> Vec<LikedEntry> {
    all(
        conn,
        "SELECT song_json, added_at FROM liked_songs ORDER BY added_at DESC, rowid DESC",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .into_iter()
    .filter_map(|(json, added_at)| {
        let song: SongItem = serde_json::from_str(&json).ok()?;
        Some(LikedEntry { song: SongItem { added_at: Some(added_at), ..song }, added_at })
    })
    .collect()
}

pub fn liked_ids(conn: &Connection) -> Vec<String> {
    all(conn, "SELECT video_id FROM liked_songs ORDER BY added_at DESC, rowid DESC", [], |r| {
        r.get(0)
    })
}

pub fn liked_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM liked_songs", [], |r| r.get(0)).unwrap_or(0)
}

/// Liked Songs as a playlist page, for `get_playlist("VLLM")`.
pub fn liked_page(conn: &Connection) -> PlaylistPage {
    let items: Vec<SongItem> = liked_songs(conn).into_iter().map(|e| e.song).collect();
    PlaylistPage {
        title: Some("Liked Songs".into()),
        subtitle: Some(format!("{} songs", items.len())),
        thumbnail: None,
        description: None,
        privacy: None,
        cover: None,
        items,
        continuation: None,
        owned: false,
        collaborative: false,
        sort_menu: None,
        suggestions: None,
    }
}

/// The Liked Songs library card (sidebar, Library page).
pub fn liked_card(conn: &Connection) -> BrowseItem {
    BrowseItem {
        kind: "playlist",
        id: LIKED_ID.into(),
        title: "Liked Songs".into(),
        subtitle: Some(format!("{} songs", liked_count(conn))),
        thumbnail: None, // the UI draws the gradient heart cover
        duration: None,
        album_id: None,
        artist_runs: Vec::new(),
        play_count: None,
        is_video: false,
        is_upload: false,
        explicit: false,
    }
}

// ─── saved albums / playlists / followed artists ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedItem {
    pub id: String,
    /// `album` | `playlist` | `artist`.
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    /// The `BrowseItem` card as it was saved.
    pub card: Value,
    pub synced: bool,
    pub added_at: i64,
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty())
}

fn card_item(card: &Value, synced: bool, added_at: i64) -> R<SavedItem> {
    let id = str_field(card, "id").ok_or("card has no id")?;
    let kind = str_field(card, "kind").unwrap_or_else(|| kind_of(&id).into());
    if !SAVED_KINDS.contains(&kind.as_str()) {
        return Err(format!("can't save a {kind} to the library"));
    }
    // The `synced` flag is ours, not part of the card.
    let mut card = card.clone();
    if let Some(o) = card.as_object_mut() {
        o.remove("synced");
    }
    Ok(SavedItem {
        title: str_field(&card, "title").unwrap_or_default(),
        subtitle: str_field(&card, "subtitle"),
        thumbnail: str_field(&card, "thumbnail"),
        id,
        kind,
        card,
        synced,
        added_at,
    })
}

fn put_saved(conn: &Connection, s: &SavedItem) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO library_items(id, kind, title, subtitle, thumbnail, card_json, synced, added_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, title = excluded.title,
             subtitle = excluded.subtitle, thumbnail = excluded.thumbnail,
             card_json = excluded.card_json, synced = excluded.synced, added_at = excluded.added_at",
        params![s.id, s.kind, s.title, s.subtitle, s.thumbnail, s.card.to_string(), s.synced, s.added_at],
    )
    .map(|_| ())
}

/// Save a card (album, YouTube playlist, artist). Idempotent: saving it again refreshes the card
/// and keeps the date. Answers whether it is newly saved.
pub fn save(conn: &Connection, card: &Value, synced: bool, now: i64) -> R<bool> {
    let mut item = card_item(card, synced, now)?;
    if let Some(prev) = saved_one(conn, &item.id) {
        item.added_at = prev.added_at;
        item.synced = prev.synced || synced;
        put_saved(conn, &item).map_err(err)?;
        return Ok(false);
    }
    put_saved(conn, &item).map_err(err)?;
    Ok(true)
}

/// Inverse of [`unsave`]: puts the row back exactly as it was.
pub fn restore_saved(conn: &Connection, item: &SavedItem) -> R<()> {
    let fresh = card_item(&item.card, item.synced, item.added_at)?;
    put_saved(conn, &SavedItem { id: item.id.clone(), kind: item.kind.clone(), ..fresh })
        .map_err(err)
}

pub fn unsave(conn: &Connection, id: &str) -> rusqlite::Result<Option<SavedItem>> {
    let prev = saved_one(conn, id);
    conn.execute("DELETE FROM library_items WHERE id = ?1", [id])?;
    Ok(prev)
}

pub fn mark_synced(conn: &Connection, ids: &[String], synced: bool) -> rusqlite::Result<()> {
    for id in ids {
        conn.execute("UPDATE library_items SET synced = ?2 WHERE id = ?1", params![id, synced])?;
    }
    Ok(())
}

fn saved_row(r: &rusqlite::Row) -> rusqlite::Result<SavedItem> {
    let card: String = r.get(5)?;
    Ok(SavedItem {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        subtitle: r.get(3)?,
        thumbnail: r.get(4)?,
        card: serde_json::from_str(&card).unwrap_or(Value::Null),
        synced: r.get(6)?,
        added_at: r.get(7)?,
    })
}

const SAVED_COLS: &str = "id, kind, title, subtitle, thumbnail, card_json, synced, added_at";

fn saved_one(conn: &Connection, id: &str) -> Option<SavedItem> {
    conn.query_row(
        &format!("SELECT {SAVED_COLS} FROM library_items WHERE id = ?1"),
        [id],
        saved_row,
    )
    .optional()
    .ok()?
}

/// Saved items, newest first, optionally one kind.
pub fn saved(conn: &Connection, kind: Option<&str>) -> Vec<SavedItem> {
    match kind {
        Some(k) => all(
            conn,
            &format!(
                "SELECT {SAVED_COLS} FROM library_items WHERE kind = ?1 ORDER BY added_at DESC, rowid DESC"
            ),
            [k],
            saved_row,
        ),
        None => all(
            conn,
            &format!("SELECT {SAVED_COLS} FROM library_items ORDER BY added_at DESC, rowid DESC"),
            [],
            saved_row,
        ),
    }
}

// ─── local playlists ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibPlaylist {
    /// `LOCALPLAYLIST:<n>`, the id every playlist command and route already takes.
    pub id: String,
    pub name: String,
    pub description: String,
    /// A cover the user picked (a file path), if any.
    pub cover: Option<String>,
    /// The first track's artwork, which stands in when there is no cover.
    pub thumbnail: Option<String>,
    pub count: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub folder_id: Option<String>,
    pub pinned: bool,
    pub last_played_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    /// The row's id: the `set_video_id` the playlist page and every removal already carry.
    pub row_id: i64,
    /// 0-based index in the playlist's order (at the time it was read or removed).
    pub position: i64,
    pub added_at: i64,
    /// The track, with `set_video_id` = row id and `added_at` filled in.
    pub song: SongItem,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AddResult {
    pub added: Vec<PlaylistEntry>,
    /// Video ids that were skipped as duplicates (only when `allow_duplicates` is false).
    pub duplicates: Vec<String>,
}

pub fn playlists(conn: &Connection) -> Vec<LibPlaylist> {
    query_playlists(conn, None)
}

pub fn playlist(conn: &Connection, id: &str) -> Option<LibPlaylist> {
    let key = local_key(id).ok()?;
    query_playlists(conn, Some(key)).pop()
}

fn query_playlists(conn: &Connection, id: Option<i64>) -> Vec<LibPlaylist> {
    let sql = format!(
        "SELECT p.id, p.title, p.description, p.created_at, p.updated_at,
                (SELECT COUNT(*) FROM local_playlist_tracks t WHERE t.playlist_id = p.id),
                (SELECT song_json FROM local_playlist_tracks t WHERE t.playlist_id = p.id
                 ORDER BY t.pos, t.id LIMIT 1),
                (SELECT value FROM settings s WHERE s.key = 'playlist_cover:' || '{prefix}' || p.id),
                (SELECT folder_id FROM lib_folder_items f WHERE f.item_id = '{prefix}' || p.id),
                EXISTS(SELECT 1 FROM lib_pins x WHERE x.item_id = '{prefix}' || p.id),
                (SELECT played_at FROM lib_played l WHERE l.item_id = '{prefix}' || p.id)
         FROM local_playlists p WHERE p.deleted_at IS NULL {only}
         ORDER BY p.updated_at DESC, p.id DESC",
        prefix = LOCAL_PLAYLIST_PREFIX,
        only = if id.is_some() { "AND p.id = ?1" } else { "" }
    );
    let row = |r: &rusqlite::Row| {
        let first: Option<String> = r.get(6)?;
        Ok(LibPlaylist {
            id: local_id(r.get(0)?),
            name: r.get(1)?,
            description: r.get(2)?,
            created_at: r.get(3)?,
            updated_at: r.get(4)?,
            count: r.get(5)?,
            thumbnail: first
                .and_then(|j| serde_json::from_str::<SongItem>(&j).ok())
                .and_then(|s| s.thumbnail),
            cover: r.get(7)?,
            folder_id: r.get(8)?,
            pinned: r.get(9)?,
            last_played_at: r.get(10)?,
        })
    };
    match id {
        Some(id) => all(conn, &sql, [id], row),
        None => all(conn, &sql, [], row),
    }
}

/// Spotify's default name for a playlist made with none: "My Playlist #N".
fn default_name(conn: &Connection) -> String {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM local_playlists WHERE deleted_at IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    format!("My Playlist #{}", n + 1)
}

pub fn create_playlist(
    conn: &Connection,
    name: Option<&str>,
    description: Option<&str>,
    folder_id: Option<&str>,
    now: i64,
) -> R<LibPlaylist> {
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    let name = name.unwrap_or_else(|| default_name(conn));
    conn.execute(
        "INSERT INTO local_playlists(title, description, created_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
        params![name, description.unwrap_or(""), now],
    )
    .map_err(err)?;
    let id = local_id(conn.last_insert_rowid());
    if let Some(folder) = folder_id {
        set_item_folder(conn, &id, Some(folder), None)?;
    }
    playlist(conn, &id).ok_or_else(|| "not_found".into())
}

pub fn update_playlist(
    conn: &Connection,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    now: i64,
) -> R<LibPlaylist> {
    let key = local_key(id)?;
    let name = name.map(str::trim);
    if name == Some("") {
        return Err("Give the playlist a name.".into());
    }
    let n = conn
        .execute(
            "UPDATE local_playlists SET title = COALESCE(?1, title),
                 description = COALESCE(?2, description), updated_at = ?3
             WHERE id = ?4 AND deleted_at IS NULL",
            params![name, description, now, key],
        )
        .map_err(err)?;
    if n == 0 {
        return Err("not_found".into());
    }
    playlist(conn, id).ok_or_else(|| "not_found".into())
}

/// Soft delete. Restorable with [`restore_playlist`] for [`DELETED_KEEP_SECS`] (the undo toast,
/// then Settings › Recover playlists).
pub fn delete_playlist(conn: &Connection, id: &str, now: i64) -> R<()> {
    let key = local_key(id)?;
    let n = conn
        .execute(
            "UPDATE local_playlists SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![now, key],
        )
        .map_err(err)?;
    if n == 0 {
        return Err("not_found".into());
    }
    Ok(())
}

pub fn restore_playlist(conn: &Connection, id: &str) -> R<LibPlaylist> {
    let key = local_key(id)?;
    let n = conn
        .execute("UPDATE local_playlists SET deleted_at = NULL WHERE id = ?1", [key])
        .map_err(err)?;
    if n == 0 {
        return Err("not_found".into());
    }
    playlist(conn, id).ok_or_else(|| "not_found".into())
}

/// A deleted playlist that can still be recovered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeletedPlaylist {
    pub id: String,
    pub name: String,
    pub count: i64,
    /// The first track's artwork.
    pub thumbnail: Option<String>,
    pub deleted_at: i64,
    /// When it stops being recoverable.
    pub purge_at: i64,
}

/// Recoverable playlists, most recently deleted first.
pub fn deleted_playlists(conn: &Connection) -> Vec<DeletedPlaylist> {
    let rows: Vec<(i64, String, i64, Option<String>, i64)> = all(
        conn,
        "SELECT p.id, p.title,
                (SELECT COUNT(*) FROM local_playlist_tracks t WHERE t.playlist_id = p.id),
                (SELECT song_json FROM local_playlist_tracks t WHERE t.playlist_id = p.id
                 ORDER BY t.pos, t.id LIMIT 1),
                p.deleted_at
         FROM local_playlists p WHERE p.deleted_at IS NOT NULL ORDER BY p.deleted_at DESC, p.id DESC",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    );
    rows.into_iter()
        .map(|(id, name, count, first, deleted_at)| DeletedPlaylist {
            id: local_id(id),
            name,
            count,
            thumbnail: first
                .and_then(|j| serde_json::from_str::<SongItem>(&j).ok())
                .and_then(|s| s.thumbnail),
            deleted_at,
            purge_at: deleted_at + DELETED_KEEP_SECS,
        })
        .collect()
}

fn live_key(conn: &Connection, id: &str) -> R<i64> {
    let key = local_key(id)?;
    let live: bool = conn
        .query_row(
            "SELECT 1 FROM local_playlists WHERE id = ?1 AND deleted_at IS NULL",
            [key],
            |_| Ok(()),
        )
        .optional()
        .map_err(err)?
        .is_some();
    if live {
        Ok(key)
    } else {
        Err("not_found".into())
    }
}

fn order(conn: &Connection, key: i64) -> rusqlite::Result<Vec<i64>> {
    try_all(
        conn,
        "SELECT id FROM local_playlist_tracks WHERE playlist_id = ?1 ORDER BY pos, id",
        [key],
        |r| r.get(0),
    )
}

fn write_order(conn: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("UPDATE local_playlist_tracks SET pos = ?1 WHERE id = ?2")?;
    for (pos, id) in ids.iter().enumerate() {
        stmt.execute(params![pos as i64, id])?;
    }
    Ok(())
}

fn touch(conn: &Connection, key: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE local_playlists SET updated_at = ?1 WHERE id = ?2", params![now, key])
        .map(|_| ())
}

/// The playlist's tracks in order.
pub fn entries(conn: &Connection, id: &str) -> R<Vec<PlaylistEntry>> {
    let key = live_key(conn, id)?;
    let rows: Vec<(i64, String, i64)> = try_all(
        conn,
        "SELECT id, song_json, added_at FROM local_playlist_tracks WHERE playlist_id = ?1
         ORDER BY pos, id",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .map_err(err)?;
    Ok(rows
        .into_iter()
        .enumerate()
        .filter_map(|(i, (row, json, added_at))| {
            let song: SongItem = serde_json::from_str(&json).ok()?;
            Some(PlaylistEntry {
                row_id: row,
                position: i as i64,
                added_at,
                song: SongItem {
                    set_video_id: Some(row.to_string()),
                    added_at: Some(added_at),
                    ..song
                },
            })
        })
        .collect())
}

/// Which of `video_ids` the playlist already holds (the "Already added" warning).
pub fn duplicates(conn: &Connection, id: &str, video_ids: &[String]) -> R<Vec<String>> {
    let key = live_key(conn, id)?;
    let have: HashSet<String> = try_all(
        conn,
        "SELECT video_id FROM local_playlist_tracks WHERE playlist_id = ?1",
        [key],
        |r| r.get(0),
    )
    .map_err(err)?
    .into_iter()
    .collect();
    let mut seen = HashSet::new();
    Ok(video_ids.iter().filter(|v| have.contains(*v) && seen.insert(*v)).cloned().collect())
}

/// The live own playlists that hold `video_id`, as `LOCALPLAYLIST:<n>` ids: the checkmarks of
/// the iOS "Add to playlist" picker and the like button's "Saved in" sheet.
pub fn playlists_containing(conn: &Connection, video_id: &str) -> Vec<String> {
    all(
        conn,
        "SELECT DISTINCT t.playlist_id FROM local_playlist_tracks t
         JOIN local_playlists p ON p.id = t.playlist_id
         WHERE t.video_id = ?1 AND p.deleted_at IS NULL ORDER BY t.playlist_id",
        [video_id],
        |r| r.get::<_, i64>(0),
    )
    .into_iter()
    .map(local_id)
    .collect()
}

/// Add tracks at `position` (0-based; `None` = the end). With `allow_duplicates` false, tracks
/// the playlist already holds, and repeats within the batch, are skipped and reported.
pub fn add(
    conn: &Connection,
    id: &str,
    songs: Vec<SongItem>,
    allow_duplicates: bool,
    position: Option<usize>,
    now: i64,
) -> R<AddResult> {
    let key = live_key(conn, id)?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    let mut have: HashSet<String> = try_all(
        &tx,
        "SELECT video_id FROM local_playlist_tracks WHERE playlist_id = ?1",
        [key],
        |r| r.get(0),
    )
    .map_err(err)?
    .into_iter()
    .collect();
    let mut duplicates = Vec::new();
    let mut new_rows = Vec::new();
    for song in songs {
        if song.video_id.is_empty() {
            continue;
        }
        if !allow_duplicates && !have.insert(song.video_id.clone()) {
            duplicates.push(song.video_id);
            continue;
        }
        let song = clean(song);
        let json = serde_json::to_string(&song).map_err(err)?;
        tx.execute(
            "INSERT INTO local_playlist_tracks(playlist_id, video_id, song_json, added_at, pos)
             VALUES(?1, ?2, ?3, ?4,
                 (SELECT COALESCE(MAX(pos), -1) + 1 FROM local_playlist_tracks WHERE playlist_id = ?1))",
            params![key, song.video_id, json, now],
        )
        .map_err(err)?;
        new_rows.push(tx.last_insert_rowid());
    }
    if !new_rows.is_empty() {
        if let Some(at) = position {
            let mut ids: Vec<i64> = order(&tx, key)
                .map_err(err)?
                .into_iter()
                .filter(|r| !new_rows.contains(r))
                .collect();
            let at = at.min(ids.len());
            ids.splice(at..at, new_rows.iter().copied());
            write_order(&tx, &ids).map_err(err)?;
        }
        touch(&tx, key, now).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    let added = entries(conn, id)?.into_iter().filter(|e| new_rows.contains(&e.row_id)).collect();
    Ok(AddResult { added, duplicates })
}

/// Remove rows. Answers the removed entries with their positions, for [`insert_entries`].
pub fn remove(conn: &Connection, id: &str, rows: &[i64], now: i64) -> R<Vec<PlaylistEntry>> {
    let key = live_key(conn, id)?;
    let wanted: HashSet<i64> = rows.iter().copied().collect();
    let removed: Vec<PlaylistEntry> =
        entries(conn, id)?.into_iter().filter(|e| wanted.contains(&e.row_id)).collect();
    let tx = conn.unchecked_transaction().map_err(err)?;
    for e in &removed {
        tx.execute(
            "DELETE FROM local_playlist_tracks WHERE playlist_id = ?1 AND id = ?2",
            params![key, e.row_id],
        )
        .map_err(err)?;
    }
    if !removed.is_empty() {
        let ids = order(&tx, key).map_err(err)?;
        write_order(&tx, &ids).map_err(err)?;
        touch(&tx, key, now).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(removed)
}

/// The inverse of [`remove`]: the entries go back at their positions, with their row ids and
/// dates. A row id that is somehow taken gets a fresh one instead.
pub fn insert_entries(
    conn: &Connection,
    id: &str,
    mut back: Vec<PlaylistEntry>,
    now: i64,
) -> R<()> {
    let key = live_key(conn, id)?;
    back.sort_by_key(|e| e.position);
    let tx = conn.unchecked_transaction().map_err(err)?;
    let mut ids = order(&tx, key).map_err(err)?;
    for e in back {
        let song = clean(e.song);
        let json = serde_json::to_string(&song).map_err(err)?;
        let taken: bool = tx
            .query_row("SELECT 1 FROM local_playlist_tracks WHERE id = ?1", [e.row_id], |_| Ok(()))
            .optional()
            .map_err(err)?
            .is_some();
        if taken {
            tx.execute(
                "INSERT INTO local_playlist_tracks(playlist_id, video_id, song_json, added_at, pos)
                 VALUES(?1, ?2, ?3, ?4, 0)",
                params![key, song.video_id, json, e.added_at],
            )
        } else {
            tx.execute(
                "INSERT INTO local_playlist_tracks(id, playlist_id, video_id, song_json, added_at, pos)
                 VALUES(?1, ?2, ?3, ?4, ?5, 0)",
                params![e.row_id, key, song.video_id, json, e.added_at],
            )
        }
        .map_err(err)?;
        let row = tx.last_insert_rowid();
        let at = (e.position.max(0) as usize).min(ids.len());
        ids.insert(at, row);
    }
    write_order(&tx, &ids).map_err(err)?;
    touch(&tx, key, now).map_err(err)?;
    tx.commit().map_err(err)
}

/// Move `rows` (kept in their current relative order) so they sit before `before`, or at the end.
/// Answers the previous order, which [`set_order`] puts back.
pub fn move_rows(
    conn: &Connection,
    id: &str,
    rows: &[i64],
    before: Option<i64>,
    now: i64,
) -> R<Vec<i64>> {
    let key = live_key(conn, id)?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    let prev = order(&tx, key).map_err(err)?;
    let moving: HashSet<i64> = rows.iter().copied().collect();
    if moving.iter().any(|r| !prev.contains(r)) {
        return Err("not_found".into());
    }
    if before.is_some_and(|b| moving.contains(&b)) {
        return Ok(prev); // dropped onto itself
    }
    let picked: Vec<i64> = prev.iter().copied().filter(|r| moving.contains(r)).collect();
    let mut rest: Vec<i64> = prev.iter().copied().filter(|r| !moving.contains(r)).collect();
    let at = match before {
        Some(b) => rest.iter().position(|&r| r == b).ok_or("not_found")?,
        None => rest.len(),
    };
    rest.splice(at..at, picked);
    write_order(&tx, &rest).map_err(err)?;
    touch(&tx, key, now).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(prev)
}

/// Apply a whole order (a custom sort saved as the order, or the undo of a move). Rows the list
/// doesn't mention keep their relative order after the ones it does.
pub fn set_order(conn: &Connection, id: &str, wanted: &[i64], now: i64) -> R<Vec<i64>> {
    let key = live_key(conn, id)?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    let prev = order(&tx, key).map_err(err)?;
    let have: HashSet<i64> = prev.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut next: Vec<i64> =
        wanted.iter().copied().filter(|r| have.contains(r) && seen.insert(*r)).collect();
    next.extend(prev.iter().copied().filter(|r| !seen.contains(r)));
    write_order(&tx, &next).map_err(err)?;
    touch(&tx, key, now).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(prev)
}

// ─── folders ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    /// Library item ids (playlists) directly in this folder, in order.
    pub items: Vec<String>,
}

/// Where a folder or an item sat: the inverse of a move is a move back here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub folder_id: Option<String>,
    pub index: i64,
}

/// What [`delete_folder`] took apart, for [`restore_folder`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FolderSnapshot {
    pub folder: Folder,
    /// Child folder ids with their positions inside the deleted folder.
    pub child_folders: Vec<(String, i64)>,
    /// Items with their positions inside the deleted folder.
    pub items: Vec<(String, i64)>,
}

pub fn folders(conn: &Connection) -> Vec<Folder> {
    let mut items: HashMap<String, Vec<String>> = HashMap::new();
    for (item, folder) in all(
        conn,
        "SELECT item_id, folder_id FROM lib_folder_items ORDER BY folder_id, position",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    ) {
        items.entry(folder).or_default().push(item);
    }
    all(
        conn,
        "SELECT id, name, parent_id, position, created_at, updated_at FROM lib_folders
         ORDER BY COALESCE(parent_id, ''), position",
        [],
        |r| {
            Ok(Folder {
                id: r.get(0)?,
                name: r.get(1)?,
                parent_id: r.get(2)?,
                position: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
                items: Vec::new(),
            })
        },
    )
    .into_iter()
    .map(|mut f| {
        f.items = items.remove(&f.id).unwrap_or_default();
        f
    })
    .collect()
}

fn folder(conn: &Connection, id: &str) -> Option<Folder> {
    folders(conn).into_iter().find(|f| f.id == id)
}

fn child_folders(conn: &Connection, parent: Option<&str>) -> rusqlite::Result<Vec<String>> {
    match parent {
        Some(p) => try_all(
            conn,
            "SELECT id FROM lib_folders WHERE parent_id = ?1 ORDER BY position, created_at",
            [p],
            |r| r.get(0),
        ),
        None => try_all(
            conn,
            "SELECT id FROM lib_folders WHERE parent_id IS NULL ORDER BY position, created_at",
            [],
            |r| r.get(0),
        ),
    }
}

fn write_folder_order(conn: &Connection, ids: &[String]) -> rusqlite::Result<()> {
    for (i, id) in ids.iter().enumerate() {
        conn.execute("UPDATE lib_folders SET position = ?1 WHERE id = ?2", params![i as i64, id])?;
    }
    Ok(())
}

fn folder_items(conn: &Connection, folder: &str) -> rusqlite::Result<Vec<String>> {
    try_all(
        conn,
        "SELECT item_id FROM lib_folder_items WHERE folder_id = ?1 ORDER BY position",
        [folder],
        |r| r.get(0),
    )
}

fn write_item_order(conn: &Connection, folder: &str, ids: &[String]) -> rusqlite::Result<()> {
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "INSERT INTO lib_folder_items(item_id, folder_id, position) VALUES(?1, ?2, ?3)
             ON CONFLICT(item_id) DO UPDATE SET folder_id = excluded.folder_id,
                 position = excluded.position",
            params![id, folder, i as i64],
        )?;
    }
    Ok(())
}

fn folder_exists(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT 1 FROM lib_folders WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

pub fn create_folder(
    conn: &Connection,
    name: Option<&str>,
    parent: Option<&str>,
    now: i64,
) -> R<Folder> {
    if let Some(p) = parent {
        if !folder_exists(conn, p) {
            return Err("not_found".into());
        }
    }
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("New Folder");
    let id = format!("FOLDER:{:016x}", rand::random::<u64>());
    let position = child_folders(conn, parent).map_err(err)?.len() as i64;
    conn.execute(
        "INSERT INTO lib_folders(id, name, parent_id, position, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, name, parent, position, now],
    )
    .map_err(err)?;
    folder(conn, &id).ok_or_else(|| "not_found".into())
}

/// Rename. Answers the old name (the inverse).
pub fn rename_folder(conn: &Connection, id: &str, name: &str, now: i64) -> R<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the folder a name.".into());
    }
    let old = folder(conn, id).ok_or("not_found")?.name;
    conn.execute(
        "UPDATE lib_folders SET name = ?1, updated_at = ?2 WHERE id = ?3",
        params![name, now, id],
    )
    .map_err(err)?;
    Ok(old)
}

/// Move a folder under `parent` (None = the root) at `index` (None = last). Refuses to move a
/// folder into itself or its own descendants. Answers where it was.
pub fn move_folder(
    conn: &Connection,
    id: &str,
    parent: Option<&str>,
    index: Option<usize>,
    now: i64,
) -> R<Place> {
    let me = folder(conn, id).ok_or("not_found")?;
    if let Some(p) = parent {
        if !folder_exists(conn, p) {
            return Err("not_found".into());
        }
        // Walk up from the new parent: meeting ourselves means a cycle.
        let mut cur = Some(p.to_owned());
        while let Some(c) = cur {
            if c == id {
                return Err("folder_cycle".into());
            }
            cur = conn
                .query_row("SELECT parent_id FROM lib_folders WHERE id = ?1", [&c], |r| r.get(0))
                .optional()
                .map_err(err)?
                .flatten();
        }
    }
    let tx = conn.unchecked_transaction().map_err(err)?;
    let old_siblings = child_folders(&tx, me.parent_id.as_deref()).map_err(err)?;
    let prev = Place {
        folder_id: me.parent_id.clone(),
        index: old_siblings.iter().position(|f| f == id).unwrap_or(0) as i64,
    };
    let rest: Vec<String> = old_siblings.into_iter().filter(|f| f != id).collect();
    write_folder_order(&tx, &rest).map_err(err)?;
    tx.execute(
        "UPDATE lib_folders SET parent_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![parent, now, id],
    )
    .map_err(err)?;
    let mut siblings: Vec<String> =
        child_folders(&tx, parent).map_err(err)?.into_iter().filter(|f| f != id).collect();
    let at = index.unwrap_or(siblings.len()).min(siblings.len());
    siblings.insert(at, id.to_owned());
    write_folder_order(&tx, &siblings).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(prev)
}

/// Delete a folder. Its contents (sub-folders and playlists) move up into its parent, at the end:
/// nothing the user made is lost with it. Answers a snapshot for [`restore_folder`].
pub fn delete_folder(conn: &Connection, id: &str) -> R<FolderSnapshot> {
    let me = folder(conn, id).ok_or("not_found")?;
    let tx = conn.unchecked_transaction().map_err(err)?;
    let kids = child_folders(&tx, Some(id)).map_err(err)?;
    let items = folder_items(&tx, id).map_err(err)?;
    let snapshot = FolderSnapshot {
        folder: me.clone(),
        child_folders: kids.iter().enumerate().map(|(i, k)| (k.clone(), i as i64)).collect(),
        items: items.iter().enumerate().map(|(i, k)| (k.clone(), i as i64)).collect(),
    };
    tx.execute("DELETE FROM lib_folders WHERE id = ?1", [id]).map_err(err)?;
    tx.execute("DELETE FROM lib_pins WHERE item_id = ?1", [id]).map_err(err)?;
    // Sub-folders join the parent's children, after the existing ones.
    let parent = me.parent_id.as_deref();
    let mut siblings: Vec<String> =
        child_folders(&tx, parent).map_err(err)?.into_iter().filter(|f| f != id).collect();
    for k in &kids {
        tx.execute("UPDATE lib_folders SET parent_id = ?1 WHERE id = ?2", params![parent, k])
            .map_err(err)?;
        siblings.push(k.clone());
    }
    write_folder_order(&tx, &siblings).map_err(err)?;
    match parent {
        Some(p) => {
            let mut list = folder_items(&tx, p).map_err(err)?;
            list.extend(items.iter().cloned());
            write_item_order(&tx, p, &list).map_err(err)?;
        }
        None => {
            tx.execute("DELETE FROM lib_folder_items WHERE folder_id = ?1", [id]).map_err(err)?;
        }
    }
    tx.commit().map_err(err)?;
    renumber_pins(conn);
    Ok(snapshot)
}

pub fn restore_folder(conn: &Connection, s: &FolderSnapshot) -> R<Folder> {
    let f = &s.folder;
    let tx = conn.unchecked_transaction().map_err(err)?;
    tx.execute(
        "INSERT OR REPLACE INTO lib_folders(id, name, parent_id, position, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![f.id, f.name, f.parent_id, f.position, f.created_at, f.updated_at],
    )
    .map_err(err)?;
    // Back in its old slot among its siblings.
    let mut siblings: Vec<String> = child_folders(&tx, f.parent_id.as_deref())
        .map_err(err)?
        .into_iter()
        .filter(|x| *x != f.id && !s.child_folders.iter().any(|(k, _)| k == x))
        .collect();
    let at = (f.position.max(0) as usize).min(siblings.len());
    siblings.insert(at, f.id.clone());
    write_folder_order(&tx, &siblings).map_err(err)?;
    for (k, pos) in &s.child_folders {
        tx.execute(
            "UPDATE lib_folders SET parent_id = ?1, position = ?2 WHERE id = ?3",
            params![f.id, pos, k],
        )
        .map_err(err)?;
    }
    if let Some(p) = f.parent_id.as_deref() {
        let rest: Vec<String> = folder_items(&tx, p)
            .map_err(err)?
            .into_iter()
            .filter(|x| !s.items.iter().any(|(k, _)| k == x))
            .collect();
        tx.execute("DELETE FROM lib_folder_items WHERE folder_id = ?1", [p]).map_err(err)?;
        write_item_order(&tx, p, &rest).map_err(err)?;
    }
    for (k, pos) in &s.items {
        tx.execute(
            "INSERT OR REPLACE INTO lib_folder_items(item_id, folder_id, position) VALUES(?1, ?2, ?3)",
            params![k, f.id, pos],
        )
        .map_err(err)?;
    }
    tx.commit().map_err(err)?;
    folder(conn, &f.id).ok_or_else(|| "not_found".into())
}

/// Put a library item (a playlist) in `folder` (None = back at the root) at `index` (None = last).
/// Answers where it was before (`None` = it was at the root).
pub fn set_item_folder(
    conn: &Connection,
    item_id: &str,
    folder: Option<&str>,
    index: Option<usize>,
) -> R<Option<Place>> {
    if let Some(f) = folder {
        if !folder_exists(conn, f) {
            return Err("not_found".into());
        }
    }
    let tx = conn.unchecked_transaction().map_err(err)?;
    let prev: Option<String> = tx
        .query_row("SELECT folder_id FROM lib_folder_items WHERE item_id = ?1", [item_id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?;
    let mut before = None;
    if let Some(p) = &prev {
        let list = folder_items(&tx, p).map_err(err)?;
        before = Some(Place {
            folder_id: Some(p.clone()),
            index: list.iter().position(|x| x == item_id).unwrap_or(0) as i64,
        });
        tx.execute("DELETE FROM lib_folder_items WHERE item_id = ?1", [item_id]).map_err(err)?;
        let rest: Vec<String> = list.into_iter().filter(|x| x != item_id).collect();
        write_item_order(&tx, p, &rest).map_err(err)?;
    }
    if let Some(f) = folder {
        let mut list = folder_items(&tx, f).map_err(err)?;
        let at = index.unwrap_or(list.len()).min(list.len());
        list.insert(at, item_id.to_owned());
        write_item_order(&tx, f, &list).map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(before)
}

// ─── pins ────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub item_id: String,
    /// `liked` | `playlist` | `album` | `artist` | `folder` | `podcast`.
    pub kind: String,
    pub position: i64,
}

pub fn pins(conn: &Connection) -> Vec<Pin> {
    all(
        conn,
        "SELECT item_id, kind, position FROM lib_pins ORDER BY position, pinned_at",
        [],
        |r| Ok(Pin { item_id: r.get(0)?, kind: r.get(1)?, position: r.get(2)? }),
    )
}

fn renumber_pins(conn: &Connection) {
    let ids: Vec<String> = pins(conn).into_iter().map(|p| p.item_id).collect();
    for (i, id) in ids.iter().enumerate() {
        let _ = conn
            .execute("UPDATE lib_pins SET position = ?1 WHERE item_id = ?2", params![i as i64, id]);
    }
}

/// Pin at `index` (None = last). Idempotent (an existing pin just moves). `pins_full` past four.
pub fn pin(
    conn: &Connection,
    item_id: &str,
    kind: Option<&str>,
    index: Option<usize>,
    now: i64,
) -> R<Vec<Pin>> {
    let mut ids: Vec<String> = pins(conn).into_iter().map(|p| p.item_id).collect();
    let already = ids.iter().any(|x| x == item_id);
    if !already && ids.len() >= MAX_PINS {
        return Err("pins_full".into());
    }
    ids.retain(|x| x != item_id);
    let at = index.unwrap_or(ids.len()).min(ids.len());
    ids.insert(at, item_id.to_owned());
    let tx = conn.unchecked_transaction().map_err(err)?;
    if !already {
        tx.execute(
            "INSERT INTO lib_pins(item_id, kind, position, pinned_at) VALUES(?1, ?2, 0, ?3)",
            params![item_id, kind.unwrap_or_else(|| kind_of(item_id)), now],
        )
        .map_err(err)?;
    }
    for (i, id) in ids.iter().enumerate() {
        tx.execute("UPDATE lib_pins SET position = ?1 WHERE item_id = ?2", params![i as i64, id])
            .map_err(err)?;
    }
    tx.commit().map_err(err)?;
    Ok(pins(conn))
}

/// Unpin. Answers the index it had (the inverse is `pin` at that index), or None if not pinned.
pub fn unpin(conn: &Connection, item_id: &str) -> R<Option<usize>> {
    let ids: Vec<String> = pins(conn).into_iter().map(|p| p.item_id).collect();
    let at = ids.iter().position(|x| x == item_id);
    conn.execute("DELETE FROM lib_pins WHERE item_id = ?1", [item_id]).map_err(err)?;
    renumber_pins(conn);
    Ok(at)
}

pub fn reorder_pins(conn: &Connection, wanted: &[String]) -> R<Vec<Pin>> {
    let have: Vec<String> = pins(conn).into_iter().map(|p| p.item_id).collect();
    let mut seen = HashSet::new();
    let mut next: Vec<&String> =
        wanted.iter().filter(|w| have.contains(w) && seen.insert(w.as_str())).collect();
    next.extend(have.iter().filter(|h| !seen.contains(h.as_str())));
    for (i, id) in next.iter().enumerate() {
        conn.execute("UPDATE lib_pins SET position = ?1 WHERE item_id = ?2", params![i as i64, id])
            .map_err(err)?;
    }
    Ok(pins(conn))
}

// ─── played / recents ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecentContext {
    pub item_id: String,
    pub kind: String,
    /// The card as it was when played (a `BrowseItem`), when the caller gave one.
    pub card: Option<Value>,
    pub played_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecentTrack {
    pub song: SongItem,
    pub played_at: i64,
    pub plays: i64,
}

/// Stamp "played from this item now". Idempotent upsert.
pub fn note_played(
    conn: &Connection,
    item_id: &str,
    kind: Option<&str>,
    card: Option<&Value>,
    now: i64,
) -> R<()> {
    let kind = kind.map(str::to_owned).or_else(|| card.and_then(|c| str_field(c, "kind")));
    let kind = kind.unwrap_or_else(|| kind_of(item_id).into());
    conn.execute(
        "INSERT INTO lib_played(item_id, kind, card_json, played_at) VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(item_id) DO UPDATE SET kind = excluded.kind,
             card_json = COALESCE(excluded.card_json, card_json),
             played_at = MAX(played_at, excluded.played_at)",
        params![item_id, kind, card.map(Value::to_string), now],
    )
    .map_err(err)?;
    Ok(())
}

/// Recently played contexts (playlists, albums, artists, Liked Songs), newest first.
pub fn recent_contexts(conn: &Connection, limit: usize) -> Vec<RecentContext> {
    all(
        conn,
        "SELECT item_id, kind, card_json, played_at FROM lib_played ORDER BY played_at DESC LIMIT ?1",
        [limit as i64],
        |r| {
            let card: Option<String> = r.get(2)?;
            let kind: String = r.get(1)?;
            let mut card: Option<Value> = card.and_then(|c| serde_json::from_str(&c).ok());
            // stream: tilefix — a show's recorded YouTube art is a signed URL that dies in hours.
            if kind == "podcast" {
                if let Some(Value::Object(m)) = card.as_mut() {
                    let id: String = r.get(0)?;
                    let stored = m.get("thumbnail").and_then(Value::as_str);
                    match crate::podcasts::store::show_art(conn, &id, stored) {
                        Some(u) => m.insert("thumbnail".into(), Value::String(u)),
                        None => m.remove("thumbnail"),
                    };
                }
            }
            Ok(RecentContext {
                item_id: r.get(0)?,
                kind,
                card,
                played_at: r.get(3)?,
            })
        },
    )
}

/// Stamp "added to / edited now" (the Recents sort's other half). Idempotent; never goes back.
pub fn note_touched(conn: &Connection, item_id: &str, now: i64) -> R<()> {
    conn.execute(
        "INSERT INTO lib_touched(item_id, touched_at) VALUES(?1, ?2)
         ON CONFLICT(item_id) DO UPDATE SET touched_at = MAX(touched_at, excluded.touched_at)",
        params![item_id, now],
    )
    .map_err(err)?;
    Ok(())
}

/// item id → when it was last added to or edited.
pub fn last_touched(conn: &Connection) -> HashMap<String, i64> {
    all(conn, "SELECT item_id, touched_at FROM lib_touched", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .into_iter()
        .collect()
}

pub fn last_played(conn: &Connection) -> HashMap<String, i64> {
    all(conn, "SELECT item_id, played_at FROM lib_played", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .into_iter()
        .collect()
}

/// Recently played songs from the local `listens` table, each song once, newest first.
pub fn recent_tracks(conn: &Connection, limit: usize) -> Vec<RecentTrack> {
    all(
        conn,
        // Podcast episodes are kept apart from songs (`episode_listens`), but Recents is
        // everything you played, as in Spotify.
        "SELECT s.song_json, MAX(l.played_at) AS last, COUNT(*) FROM listens l
         JOIN listen_songs s ON s.video_id = l.video_id
         GROUP BY l.video_id
         UNION ALL
         SELECT (SELECT j.song_json FROM episode_listens j WHERE j.video_id = e.video_id
                 ORDER BY j.played_at DESC LIMIT 1), MAX(e.played_at) AS last, COUNT(*)
         FROM episode_listens e GROUP BY e.video_id
         ORDER BY last DESC LIMIT ?1",
        [limit as i64],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
    )
    .into_iter()
    .filter_map(|(json, played_at, plays)| {
        let song = crate::api::playlist_row(serde_json::from_str(&json).ok()?);
        Some(RecentTrack { song, played_at, plays })
    })
    .collect()
}

// ─── hidden songs (blocklist; Stream D filters with it) ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HiddenSong {
    pub video_id: String,
    /// `None` = hidden everywhere, else only in that playlist/album.
    pub context_id: Option<String>,
    pub song: Option<SongItem>,
    pub hidden_at: i64,
}

pub fn hide_song(
    conn: &Connection,
    video_id: &str,
    song: Option<SongItem>,
    context: Option<&str>,
    now: i64,
) -> R<bool> {
    let json = song.map(clean).and_then(|s| serde_json::to_string(&s).ok());
    let n = conn
        .execute(
            "INSERT INTO hidden_songs(video_id, context_id, song_json, hidden_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(video_id, context_id) DO NOTHING",
            params![video_id, context.unwrap_or(""), json, now],
        )
        .map_err(err)?;
    Ok(n > 0)
}

/// Answers the removed row (the inverse is `hide_song` with it).
pub fn unhide_song(
    conn: &Connection,
    video_id: &str,
    context: Option<&str>,
) -> R<Option<HiddenSong>> {
    let prev = hidden_songs(conn, None)
        .into_iter()
        .find(|h| h.video_id == video_id && h.context_id.as_deref() == context);
    conn.execute(
        "DELETE FROM hidden_songs WHERE video_id = ?1 AND context_id = ?2",
        params![video_id, context.unwrap_or("")],
    )
    .map_err(err)?;
    Ok(prev)
}

/// Every hidden row, or (with `context`) the ones that apply there: global plus that context's.
pub fn hidden_songs(conn: &Connection, context: Option<&str>) -> Vec<HiddenSong> {
    let row = |r: &rusqlite::Row| {
        let ctx: String = r.get(1)?;
        let json: Option<String> = r.get(2)?;
        Ok(HiddenSong {
            video_id: r.get(0)?,
            context_id: (!ctx.is_empty()).then_some(ctx),
            song: json.and_then(|j| serde_json::from_str(&j).ok()),
            hidden_at: r.get(3)?,
        })
    };
    let sql = "SELECT video_id, context_id, song_json, hidden_at FROM hidden_songs";
    match context {
        Some(c) => all(
            conn,
            &format!("{sql} WHERE context_id IN ('', ?1) ORDER BY hidden_at DESC"),
            [c],
            row,
        ),
        None => all(conn, &format!("{sql} ORDER BY hidden_at DESC"), [], row),
    }
}

/// Video ids hidden everywhere.
pub fn hidden_ids(conn: &Connection) -> Vec<String> {
    all(conn, "SELECT video_id FROM hidden_songs WHERE context_id = ''", [], |r| r.get(0))
}

// ─── private session ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrivateSession {
    pub enabled: bool,
    pub since: Option<i64>,
    pub expires_at: Option<i64>,
}

pub fn private_session(conn: &Connection, now: i64) -> PrivateSession {
    let since = get_setting(conn, PRIVATE_KEY).and_then(|v| v.parse::<i64>().ok());
    match since {
        Some(s) if now - s < PRIVATE_TTL_SECS => {
            PrivateSession { enabled: true, since: Some(s), expires_at: Some(s + PRIVATE_TTL_SECS) }
        }
        _ => PrivateSession { enabled: false, since: None, expires_at: None },
    }
}

pub fn set_private_session(conn: &Connection, enabled: bool, now: i64) -> R<PrivateSession> {
    if enabled {
        set_setting(conn, PRIVATE_KEY, &now.to_string()).map_err(err)?;
    } else {
        conn.execute("DELETE FROM settings WHERE key = ?1", [PRIVATE_KEY]).map_err(err)?;
    }
    Ok(private_session(conn, now))
}

/// Is a private session on? `state.rs` checks this before counting a listen: in a private session
/// nothing is recorded (local history, Your Rotation, stats, the YouTube history ping).
pub fn is_private(db: &Db) -> bool {
    private_session(&db.conn(), now_secs()).enabled
}

// ─── one-time migration of the webview's localStorage library ────────────────────────────────────

/// The parts of Limusic's `limusic:personal` localStorage blob that are library data. The rest
/// (Shortcuts grid, home layout, recent searches) stays a UI preference in the webview.
#[derive(Debug, Default, Deserialize)]
pub struct LegacyBlob {
    #[serde(default)]
    saved: Vec<Value>,
    #[serde(default)]
    pins: Vec<String>,
    #[serde(default)]
    recent: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    /// It ran before: nothing was imported this time.
    pub already: bool,
    pub saved: usize,
    pub pins: usize,
    pub recent: usize,
}

pub fn migrate_legacy(conn: &Connection, raw: &Value, now: i64) -> R<MigrationReport> {
    if get_setting(conn, LEGACY_MIGRATED_KEY).is_some() {
        return Ok(MigrationReport { already: true, ..Default::default() });
    }
    let blob: LegacyBlob = serde_json::from_value(raw.clone()).unwrap_or_default();
    let mut report = MigrationReport::default();
    let tx = conn.unchecked_transaction().map_err(err)?;
    // Keep the raw blob: if anything here misreads it, nothing is lost.
    set_setting(&tx, LEGACY_BACKUP_KEY, &raw.to_string()).map_err(err)?;
    // Newest first in the blob: stamp one second apart so the order survives.
    for (i, card) in blob.saved.iter().enumerate() {
        let synced = card.get("synced").and_then(Value::as_bool).unwrap_or(false);
        let Ok(item) = card_item(card, synced, now - i as i64) else { continue };
        if saved_one(&tx, &item.id).is_none() {
            put_saved(&tx, &item).map_err(err)?;
            report.saved += 1;
        }
    }
    for (id, entry) in &blob.recent {
        // `at` is milliseconds in the webview.
        let at = entry.get("at").and_then(Value::as_i64).map(|ms| ms / 1000).unwrap_or(now);
        let mut card = entry.clone();
        if let Some(o) = card.as_object_mut() {
            o.remove("at");
        }
        note_played(&tx, id, None, Some(&card), at)?;
        report.recent += 1;
    }
    tx.commit().map_err(err)?;
    for id in blob.pins.iter().take(MAX_PINS) {
        if pin(conn, id, None, None, now).is_ok() {
            report.pins += 1;
        }
    }
    set_setting(conn, LEGACY_MIGRATED_KEY, &now.to_string()).map_err(err)?;
    Ok(report)
}

// ─── snapshot (one round trip for the store) ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySnapshot {
    pub liked_ids: Vec<String>,
    pub playlists: Vec<LibPlaylist>,
    pub folders: Vec<Folder>,
    pub pins: Vec<Pin>,
    pub saved: Vec<SavedItem>,
    pub last_played: HashMap<String, i64>,
    /// item id → when it was last added to or edited (`lib_touched`). Recents sorts by the later
    /// of this and `last_played`.
    pub last_touched: HashMap<String, i64>,
    pub hidden_song_ids: Vec<String>,
    pub private_session: PrivateSession,
    pub legacy_migrated: bool,
}

/// The first-run import offer's setting: "true" once it was answered (imported or "Not now").
pub const IMPORT_ONBOARDING_DONE: &str = "import_onboarding_done";

/// First run, both apps: offer the Spotify import only while the library is empty (no liked
/// songs, playlists or saved items) and the offer was never answered.
pub fn import_offer(conn: &Connection, answered: bool) -> bool {
    !answered && liked_ids(conn).is_empty() && playlists(conn).is_empty() && saved(conn, None).is_empty()
}

/// [`import_offer`] against the settings table.
pub fn import_offer_db(db: &crate::db::Db) -> bool {
    let answered = db.get_setting(IMPORT_ONBOARDING_DONE).as_deref() == Some("true");
    import_offer(&db.conn(), answered)
}

/// The offer was answered: never again.
pub fn import_offer_done(db: &crate::db::Db) {
    db.set_setting(IMPORT_ONBOARDING_DONE, "true");
}

pub fn snapshot(conn: &Connection, now: i64) -> LibrarySnapshot {
    LibrarySnapshot {
        liked_ids: liked_ids(conn),
        playlists: playlists(conn),
        folders: folders(conn),
        pins: pins(conn),
        saved: saved(conn, None),
        last_played: last_played(conn),
        last_touched: last_touched(conn),
        hidden_song_ids: hidden_ids(conn),
        private_session: private_session(conn, now),
        legacy_migrated: get_setting(conn, LEGACY_MIGRATED_KEY).is_some(),
    }
}

// ─── export ──────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub path: String,
    pub liked_songs: usize,
    pub playlists: usize,
    pub files: Vec<String>,
}

/// Unix seconds → `2026-11-15T09:30:00Z`.
pub fn iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Where a track lives: a YouTube Music link, or the file on disk for a local track.
fn location(video_id: &str) -> String {
    match video_id.strip_prefix("LOCAL:") {
        Some(path) => format!("file://{}", path.replace(' ', "%20")),
        None => format!("https://music.youtube.com/watch?v={video_id}"),
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Exportify's column names, so TuneMyMusic / Soundiiz read it, plus our own ids.
pub fn tracks_csv(rows: &[(SongItem, i64)]) -> String {
    let mut w = csv::Writer::from_writer(Vec::new());
    let _ = w.write_record([
        "Track URI",
        "Track Name",
        "Album Name",
        "Artist Name(s)",
        "Duration (ms)",
        "Explicit",
        "Added At",
        "YouTube Video ID",
        "Album ID",
    ]);
    for (s, added_at) in rows {
        let ms = duration_secs(s.duration.as_deref())
            .map(|d| (d * 1000).to_string())
            .unwrap_or_default();
        let _ = w.write_record([
            location(&s.video_id).as_str(),
            &s.title,
            s.album.as_deref().unwrap_or(""),
            &s.artists,
            &ms,
            if s.explicit { "true" } else { "false" },
            &iso8601(*added_at),
            &s.video_id,
            s.album_id.as_deref().unwrap_or(""),
        ]);
    }
    String::from_utf8(w.into_inner().unwrap_or_default()).unwrap_or_default()
}

pub fn tracks_m3u8(name: &str, songs: &[SongItem]) -> String {
    let mut out = format!("#EXTM3U\n#PLAYLIST:{name}\n");
    for s in songs {
        let secs = duration_secs(s.duration.as_deref()).unwrap_or(-1);
        out.push_str(&format!(
            "#EXTINF:{secs},{} - {}\n{}\n",
            s.artists,
            s.title,
            location(&s.video_id)
        ));
    }
    out
}

pub fn tracks_xspf(name: &str, songs: &[SongItem]) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<playlist version=\"1\" xmlns=\"http://xspf.org/ns/0/\">\n  <title>{}</title>\n  <trackList>\n",
        xml(name)
    );
    for s in songs {
        out.push_str("    <track>\n");
        out.push_str(&format!("      <location>{}</location>\n", xml(&location(&s.video_id))));
        out.push_str(&format!(
            "      <identifier>https://music.youtube.com/watch?v={}</identifier>\n",
            xml(&s.video_id)
        ));
        out.push_str(&format!("      <title>{}</title>\n", xml(&s.title)));
        out.push_str(&format!("      <creator>{}</creator>\n", xml(&s.artists)));
        if let Some(a) = &s.album {
            out.push_str(&format!("      <album>{}</album>\n", xml(a)));
        }
        if let Some(d) = duration_secs(s.duration.as_deref()) {
            out.push_str(&format!("      <duration>{}</duration>\n", d * 1000));
        }
        if let Some(t) = &s.thumbnail {
            out.push_str(&format!("      <image>{}</image>\n", xml(t)));
        }
        out.push_str("    </track>\n");
    }
    out.push_str("  </trackList>\n</playlist>\n");
    out
}

fn podcasts(conn: &Connection) -> Vec<Value> {
    all(
        conn,
        "SELECT id, title, author, thumb, feed_url, yt_browse_id, followed_at FROM podcast_follows
         ORDER BY followed_at DESC",
        [],
        |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?,
                "author": r.get::<_, Option<String>>(2)?, "thumb": r.get::<_, Option<String>>(3)?,
                "feedUrl": r.get::<_, Option<String>>(4)?, "ytBrowseId": r.get::<_, Option<String>>(5)?,
                "followedAt": r.get::<_, i64>(6)?,
            }))
        },
    )
}

/// Everything, as one versioned JSON document: the backup format.
pub fn library_json(conn: &Connection, blocked_artists: Value, now: i64) -> Value {
    let lists: Vec<Value> = playlists(conn)
        .into_iter()
        .map(|p| {
            let tracks: Vec<Value> = entries(conn, &p.id)
                .unwrap_or_default()
                .into_iter()
                .map(|e| json!({ "position": e.position, "addedAt": e.added_at, "song": SongItem { set_video_id: None, added_at: None, ..e.song } }))
                .collect();
            json!({ "playlist": p, "tracks": tracks })
        })
        .collect();
    let saved_of = |k: &str| serde_json::to_value(saved(conn, Some(k))).unwrap_or(Value::Null);
    json!({
        "format": "encore-library",
        "version": 1,
        "exportedAt": iso8601(now),
        "likedSongs": liked_songs(conn),
        "playlists": lists,
        "folders": folders(conn),
        "pins": pins(conn),
        "savedAlbums": saved_of("album"),
        "savedPlaylists": saved_of("playlist"),
        "followedArtists": saved_of("artist"),
        "followedPodcasts": podcasts(conn),
        "hiddenSongs": hidden_songs(conn, None),
        "blockedArtists": blocked_artists,
        "recentlyPlayed": recent_contexts(conn, 500),
    })
}

/// A file name that's safe on every OS, unique within the archive.
fn file_name(name: &str, taken: &mut HashSet<String>) -> String {
    let mut base: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_owned();
    if base.is_empty() {
        base = "Playlist".into();
    }
    base = base.chars().take(100).collect();
    let mut candidate = base.clone();
    let mut n = 2;
    while !taken.insert(candidate.to_lowercase()) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    candidate
}

/// Write the export zip: `library.json`, `Liked Songs.{csv,m3u8,xspf}` and one CSV + M3U8 + XSPF
/// per playlist under `playlists/`.
pub fn export_zip(
    conn: &Connection,
    blocked_artists: Value,
    path: &std::path::Path,
    now: i64,
) -> R<ExportReport> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(path).map_err(err)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut files = Vec::new();
    let mut put = |zip: &mut zip::ZipWriter<std::fs::File>, name: String, body: String| -> R<()> {
        zip.start_file(name.as_str(), opts).map_err(err)?;
        zip.write_all(body.as_bytes()).map_err(err)?;
        files.push(name);
        Ok(())
    };
    let doc = library_json(conn, blocked_artists, now);
    put(&mut zip, "library.json".into(), serde_json::to_string_pretty(&doc).map_err(err)?)?;
    let liked = liked_songs(conn);
    let liked_rows: Vec<(SongItem, i64)> =
        liked.iter().map(|e| (e.song.clone(), e.added_at)).collect();
    let liked_songs_only: Vec<SongItem> = liked.iter().map(|e| e.song.clone()).collect();
    put(&mut zip, "Liked Songs.csv".into(), tracks_csv(&liked_rows))?;
    put(&mut zip, "Liked Songs.m3u8".into(), tracks_m3u8("Liked Songs", &liked_songs_only))?;
    put(&mut zip, "Liked Songs.xspf".into(), tracks_xspf("Liked Songs", &liked_songs_only))?;
    let lists = playlists(conn);
    let mut taken = HashSet::new();
    for p in &lists {
        let rows: Vec<(SongItem, i64)> =
            entries(conn, &p.id)?.into_iter().map(|e| (e.song, e.added_at)).collect();
        let songs: Vec<SongItem> = rows.iter().map(|r| r.0.clone()).collect();
        let base = file_name(&p.name, &mut taken);
        put(&mut zip, format!("playlists/{base}.csv"), tracks_csv(&rows))?;
        put(&mut zip, format!("playlists/{base}.m3u8"), tracks_m3u8(&p.name, &songs))?;
        put(&mut zip, format!("playlists/{base}.xspf"), tracks_xspf(&p.name, &songs))?;
    }
    zip.finish().map_err(err)?;
    Ok(ExportReport {
        path: path.to_string_lossy().into_owned(),
        liked_songs: liked.len(),
        playlists: lists.len(),
        files,
    })
}

// ─── Spotify import hooks (import.rs) ────────────────────────────────────────────────────────────

/// Liked Songs from a Spotify import become likes, not a playlist. `songs` newest first.
pub fn import_likes(db: &Db, songs: Vec<SongItem>) -> R<usize> {
    let added = like(&db.conn(), songs, now_secs()).map_err(err)?;
    Ok(added.into_iter().filter(|a| *a).count())
}

/// A saved album / followed artist resolved from a Spotify export.
pub fn import_saved(db: &Db, card: &Value) -> R<bool> {
    save(&db.conn(), card, false, now_secs())
}

// ─── Commands (the desktop wraps each in a Tauri command; iOS calls them through UniFFI) ──────────────────────────────────────────────────────────────────────────────


/// Every window's store reloads the named part on this event (`library-changed`).
pub fn changed(state: &AppState, scope: &str) {
    let _ = state.host.emit("library-changed", json!({ "scope": scope }));
}

/// stream: magic-d4 — an item was added to or edited (or, for Liked Songs, a song liked): stamp
/// `lib_touched` and tell every window, so Recents moves it to the top at once, as in Spotify.
/// Emits `library-changed` `{ scope: "touched", itemId, at }`; the payload carries the stamp, so a
/// client can update its map without a round trip.
pub fn touched(state: &AppState, item_id: &str) {
    let at = now_secs();
    if note_touched(&state.db.conn(), item_id, at).is_ok() {
        tracing::info!(item_id, "library: recents touched");
        let _ = state
            .host
            .emit("library-changed", json!({ "scope": "touched", "itemId": item_id, "at": at }));
    }
}

pub async fn lib_snapshot(state: &AppState) -> R<LibrarySnapshot> {
    Ok(snapshot(&state.db.conn(), now_secs()))
}

// likes

pub async fn lib_liked_ids(state: &AppState) -> R<Vec<String>> {
    Ok(liked_ids(&state.db.conn()))
}

pub async fn lib_liked_songs(state: &AppState) -> R<Vec<LikedEntry>> {
    Ok(liked_songs(&state.db.conn()))
}

pub async fn lib_like(state: &AppState, songs: Vec<SongItem>) -> R<Vec<bool>> {
    let out = like(&state.db.conn(), songs, now_secs()).map_err(err)?;
    changed(state, "liked");
    if out.iter().any(|n| *n) {
        touched(state, LIKED_ID);
    }
    Ok(out)
}

pub async fn lib_unlike(state: &AppState, video_ids: Vec<String>) -> R<Vec<LikedEntry>> {
    let out = unlike(&state.db.conn(), &video_ids).map_err(err)?;
    changed(state, "liked");
    Ok(out)
}

pub async fn lib_restore_likes(state: &AppState, entries: Vec<LikedEntry>) -> R<()> {
    restore_likes(&state.db.conn(), entries).map_err(err)?;
    changed(state, "liked");
    touched(state, LIKED_ID);
    Ok(())
}

// saved items

pub async fn lib_saved(state: &AppState, kind: Option<String>) -> R<Vec<SavedItem>> {
    Ok(saved(&state.db.conn(), kind.as_deref()))
}

pub async fn lib_save(state: &AppState, card: Value, synced: Option<bool>) -> R<bool> {
    let out = save(&state.db.conn(), &card, synced.unwrap_or(false), now_secs())?;
    changed(state, "saved");
    Ok(out)
}

pub async fn lib_unsave(state: &AppState, id: String) -> R<Option<SavedItem>> {
    let out = unsave(&state.db.conn(), &id).map_err(err)?;
    changed(state, "saved");
    Ok(out)
}

pub async fn lib_restore_saved(state: &AppState, item: SavedItem) -> R<()> {
    restore_saved(&state.db.conn(), &item)?;
    changed(state, "saved");
    Ok(())
}

pub async fn lib_mark_synced(state: &AppState, ids: Vec<String>, synced: Option<bool>) -> R<()> {
    mark_synced(&state.db.conn(), &ids, synced.unwrap_or(true)).map_err(err)?;
    changed(state, "saved");
    Ok(())
}

// playlists

pub async fn lib_playlists(state: &AppState) -> R<Vec<LibPlaylist>> {
    Ok(playlists(&state.db.conn()))
}

pub async fn lib_playlist_create(
    state: &AppState,
    name: Option<String>,
    description: Option<String>,
    folder_id: Option<String>,
) -> R<LibPlaylist> {
    let out = create_playlist(
        &state.db.conn(),
        name.as_deref(),
        description.as_deref(),
        folder_id.as_deref(),
        now_secs(),
    )?;
    changed(state, "playlists");
    Ok(out)
}

pub async fn lib_playlist_update(
    state: &AppState,
    id: String,
    name: Option<String>,
    description: Option<String>,
) -> R<LibPlaylist> {
    let out = update_playlist(
        &state.db.conn(),
        &id,
        name.as_deref(),
        description.as_deref(),
        now_secs(),
    )?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

pub async fn lib_playlist_delete(state: &AppState, id: String) -> R<()> {
    delete_playlist(&state.db.conn(), &id, now_secs())?;
    changed(state, "playlists");
    Ok(())
}

pub async fn lib_playlist_restore(state: &AppState, id: String) -> R<LibPlaylist> {
    let out = restore_playlist(&state.db.conn(), &id)?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

/// stream: magic-d4 — Settings › Recover playlists.
pub async fn lib_deleted_playlists(state: &AppState) -> R<Vec<DeletedPlaylist>> {
    Ok(deleted_playlists(&state.db.conn()))
}

pub async fn lib_playlist_entries(state: &AppState, id: String) -> R<Vec<PlaylistEntry>> {
    entries(&state.db.conn(), &id)
}

pub async fn lib_playlist_duplicates(
    state: &AppState,
    id: String,
    video_ids: Vec<String>,
) -> R<Vec<String>> {
    duplicates(&state.db.conn(), &id, &video_ids)
}

pub async fn lib_playlists_containing(state: &AppState, video_id: String) -> R<Vec<String>> {
    Ok(playlists_containing(&state.db.conn(), &video_id))
}

/// Remove every row of `video_ids` from the playlist (the picker's uncheck). Returns the rows.
pub async fn lib_playlist_remove_videos(
    state: &AppState,
    id: String,
    video_ids: Vec<String>,
) -> R<Vec<PlaylistEntry>> {
    let conn = state.db.conn();
    let rows: Vec<i64> = entries(&conn, &id)?
        .into_iter()
        .filter(|e| video_ids.iter().any(|v| *v == e.song.video_id))
        .map(|e| e.row_id)
        .collect();
    let out = if rows.is_empty() { Vec::new() } else { remove(&conn, &id, &rows, now_secs())? };
    drop(conn);
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

pub async fn lib_playlist_add(
    state: &AppState,
    id: String,
    songs: Vec<SongItem>,
    allow_duplicates: Option<bool>,
    position: Option<usize>,
) -> R<AddResult> {
    let out =
        add(&state.db.conn(), &id, songs, allow_duplicates.unwrap_or(false), position, now_secs())?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

pub async fn lib_playlist_remove(
    state: &AppState,
    id: String,
    row_ids: Vec<i64>,
) -> R<Vec<PlaylistEntry>> {
    let out = remove(&state.db.conn(), &id, &row_ids, now_secs())?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

pub async fn lib_playlist_insert_entries(
    state: &AppState,
    id: String,
    entries: Vec<PlaylistEntry>,
) -> R<()> {
    insert_entries(&state.db.conn(), &id, entries, now_secs())?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(())
}

pub async fn lib_playlist_move(
    state: &AppState,
    id: String,
    row_ids: Vec<i64>,
    before: Option<i64>,
) -> R<Vec<i64>> {
    let out = move_rows(&state.db.conn(), &id, &row_ids, before, now_secs())?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

pub async fn lib_playlist_set_order(state: &AppState, id: String, row_ids: Vec<i64>) -> R<Vec<i64>> {
    let out = set_order(&state.db.conn(), &id, &row_ids, now_secs())?;
    changed(state, "playlists");
    touched(state, &id);
    Ok(out)
}

// folders

pub async fn lib_folders(state: &AppState) -> R<Vec<Folder>> {
    Ok(folders(&state.db.conn()))
}

pub async fn lib_folder_create(
    state: &AppState,
    name: Option<String>,
    parent_id: Option<String>,
) -> R<Folder> {
    let out = create_folder(&state.db.conn(), name.as_deref(), parent_id.as_deref(), now_secs())?;
    changed(state, "folders");
    Ok(out)
}

pub async fn lib_folder_rename(state: &AppState, id: String, name: String) -> R<String> {
    let out = rename_folder(&state.db.conn(), &id, &name, now_secs())?;
    changed(state, "folders");
    Ok(out)
}

pub async fn lib_folder_move(
    state: &AppState,
    id: String,
    parent_id: Option<String>,
    index: Option<usize>,
) -> R<Place> {
    let out = move_folder(&state.db.conn(), &id, parent_id.as_deref(), index, now_secs())?;
    changed(state, "folders");
    Ok(out)
}

pub async fn lib_folder_delete(state: &AppState, id: String) -> R<FolderSnapshot> {
    let out = delete_folder(&state.db.conn(), &id)?;
    changed(state, "folders");
    Ok(out)
}

pub async fn lib_folder_restore(state: &AppState, snapshot: FolderSnapshot) -> R<Folder> {
    let out = restore_folder(&state.db.conn(), &snapshot)?;
    changed(state, "folders");
    Ok(out)
}

pub async fn lib_folder_set_item(
    state: &AppState,
    item_id: String,
    folder_id: Option<String>,
    index: Option<usize>,
) -> R<Option<Place>> {
    let out = set_item_folder(&state.db.conn(), &item_id, folder_id.as_deref(), index)?;
    changed(state, "folders");
    Ok(out)
}

// pins

pub async fn lib_pins(state: &AppState) -> R<Vec<Pin>> {
    Ok(pins(&state.db.conn()))
}

pub async fn lib_pin(
    state: &AppState,
    item_id: String,
    kind: Option<String>,
    index: Option<usize>,
) -> R<Vec<Pin>> {
    let out = pin(&state.db.conn(), &item_id, kind.as_deref(), index, now_secs())?;
    changed(state, "pins");
    Ok(out)
}

pub async fn lib_unpin(state: &AppState, item_id: String) -> R<Option<usize>> {
    let out = unpin(&state.db.conn(), &item_id)?;
    changed(state, "pins");
    Ok(out)
}

pub async fn lib_pins_reorder(state: &AppState, item_ids: Vec<String>) -> R<Vec<Pin>> {
    let out = reorder_pins(&state.db.conn(), &item_ids)?;
    changed(state, "pins");
    Ok(out)
}

// played / recents

pub async fn lib_note_played(
    state: &AppState,
    item_id: String,
    kind: Option<String>,
    card: Option<Value>,
) -> R<()> {
    note_played(&state.db.conn(), &item_id, kind.as_deref(), card.as_ref(), now_secs())?;
    changed(state, "played");
    Ok(())
}

pub async fn lib_recent_contexts(state: &AppState, limit: Option<usize>) -> R<Vec<RecentContext>> {
    Ok(recent_contexts(&state.db.conn(), limit.unwrap_or(50)))
}

pub async fn lib_recent_tracks(state: &AppState, limit: Option<usize>) -> R<Vec<RecentTrack>> {
    Ok(recent_tracks(&state.db.conn(), limit.unwrap_or(50)))
}

pub async fn lib_last_played(state: &AppState) -> R<HashMap<String, i64>> {
    Ok(last_played(&state.db.conn()))
}

// hidden songs

pub async fn lib_hidden_songs(state: &AppState, context_id: Option<String>) -> R<Vec<HiddenSong>> {
    Ok(hidden_songs(&state.db.conn(), context_id.as_deref()))
}

pub async fn lib_hide_song(
    state: &AppState,
    video_id: String,
    song: Option<SongItem>,
    context_id: Option<String>,
) -> R<bool> {
    let out = hide_song(&state.db.conn(), &video_id, song, context_id.as_deref(), now_secs())?;
    changed(state, "hidden");
    Ok(out)
}

pub async fn lib_unhide_song(
    state: &AppState,
    video_id: String,
    context_id: Option<String>,
) -> R<Option<HiddenSong>> {
    let out = unhide_song(&state.db.conn(), &video_id, context_id.as_deref())?;
    changed(state, "hidden");
    Ok(out)
}

// private session

pub async fn lib_private_session(state: &AppState) -> R<PrivateSession> {
    Ok(private_session(&state.db.conn(), now_secs()))
}

pub async fn lib_set_private_session(state: &AppState, enabled: bool) -> R<PrivateSession> {
    let out = set_private_session(&state.db.conn(), enabled, now_secs())?;
    changed(state, "private");
    Ok(out)
}

// migration + export

pub async fn lib_migrate_legacy(state: &AppState, blob: Value) -> R<MigrationReport> {
    let out = migrate_legacy(&state.db.conn(), &blob, now_secs())?;
    if !out.already {
        changed(state, "all");
    }
    Ok(out)
}

/// Write the export zip to `path`, or to `~/Downloads/Spotless Library <date>.zip` when none given.
pub async fn lib_export(state: &AppState, path: Option<String>) -> R<ExportReport> {
    let now = now_secs();
    let path = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let dir = state.host.download_dir().ok_or("no Downloads folder")?;
            dir.join(format!("Spotless Library {}.zip", &iso8601(now)[..10]))
        }
    };
    let blocked = serde_json::to_value(crate::blocked::list(&state.db)).unwrap_or(Value::Null);
    let db = Arc::clone(&state.db);
    crate::rt::spawn_blocking(move || export_zip(&db.conn(), blocked, &path, now))
        .await
        .map_err(err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open(std::path::Path::new(":memory:")).unwrap()
    }

    fn song(id: &str, title: &str) -> SongItem {
        SongItem {
            video_id: id.into(),
            title: title.into(),
            artists: "Artist".into(),
            artist_runs: serde_json::from_value(json!([{"text": "Artist", "id": "UCa"}])).unwrap(),
            album: Some("Album".into()),
            album_id: Some("MPREx".into()),
            duration: Some("3:05".into()),
            thumbnail: Some("https://img/x.jpg".into()),
            explicit: true,
            ..Default::default()
        }
    }

    fn rows(c: &Connection, id: &str) -> Vec<String> {
        entries(c, id).unwrap().into_iter().map(|e| e.song.video_id).collect()
    }

    // stream: magic-d4 — Recents' "touched" half: adds, edits and likes.
    #[test]
    fn touched_stamps_only_move_forward_and_reach_the_snapshot() {
        let d = db();
        let c = d.conn();
        note_touched(&c, LIKED_ID, 500).unwrap();
        note_touched(&c, LIKED_ID, 300).unwrap();
        note_touched(&c, "LOCALPLAYLIST:1", 400).unwrap();
        let t = last_touched(&c);
        assert_eq!(t[LIKED_ID], 500, "an older stamp never moves it back");
        assert_eq!(t["LOCALPLAYLIST:1"], 400);
        assert_eq!(snapshot(&c, 600).last_touched, t);
        forget_item(&c, "LOCALPLAYLIST:1");
        assert!(!last_touched(&c).contains_key("LOCALPLAYLIST:1"));
    }

    #[test]
    fn import_offer_only_for_an_empty_unanswered_library() {
        let d = db();
        assert!(import_offer_db(&d), "first run, empty library");
        import_offer_done(&d);
        assert!(!import_offer_db(&d), "answered: never again");
        let d = db();
        like(&d.conn(), vec![song("a", "A")], 1).unwrap();
        assert!(!import_offer_db(&d), "a library with a liked song isn't offered the import");
    }

    #[test]
    fn like_unlike_restore() {
        let d = db();
        let c = d.conn();
        assert_eq!(like(&c, vec![song("a", "A")], 100).unwrap(), [true]);
        assert_eq!(like(&c, vec![song("b", "B"), song("a", "A2")], 200).unwrap(), [true, false]);
        // Newest first; a re-like keeps its date but refreshes the snapshot.
        let l = liked_songs(&c);
        assert_eq!(l.iter().map(|e| e.song.video_id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(l[1].added_at, 100);
        assert_eq!(l[1].song.title, "A2");
        assert!(l[0].song.explicit);
        let dur: i64 = c
            .query_row("SELECT duration_secs FROM liked_songs WHERE video_id='b'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dur, 185);
        let removed = unlike(&c, &["a".into(), "zz".into()]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(liked_ids(&c), ["b"]);
        restore_likes(&c, removed).unwrap();
        assert_eq!(liked_ids(&c), ["b", "a"]);
        assert_eq!(liked_songs(&c)[1].added_at, 100);
        // Bulk like keeps the given order newest first.
        like(&c, vec![song("x", "X"), song("y", "Y")], 1_000).unwrap();
        assert_eq!(&liked_ids(&c)[..2], ["x", "y"]);
        assert_eq!(liked_page(&c).items.len(), 4);
    }

    #[test]
    fn saved_items_round_trip() {
        let d = db();
        let c = d.conn();
        let card =
            json!({"kind": "album", "id": "MPRE1", "title": "Album", "subtitle": "X • 2020"});
        assert!(save(&c, &card, false, 10).unwrap());
        assert!(!save(&c, &card, true, 20).unwrap());
        let s = saved(&c, Some("album"));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].added_at, 10);
        assert!(s[0].synced);
        assert!(save(&c, &json!({"kind": "song", "id": "v"}), false, 1).is_err());
        let gone = unsave(&c, "MPRE1").unwrap().unwrap();
        assert!(saved(&c, None).is_empty());
        restore_saved(&c, &gone).unwrap();
        assert_eq!(saved(&c, None), vec![gone]);
    }

    #[test]
    fn playlist_crud_reorder_and_duplicates() {
        let d = db();
        let c = d.conn();
        let p = create_playlist(&c, None, None, None, 1).unwrap();
        assert_eq!(p.name, "My Playlist #1");
        let id = p.id.clone();
        let r = add(&c, &id, vec![song("a", "A"), song("b", "B"), song("a", "A")], false, None, 2)
            .unwrap();
        assert_eq!(r.added.len(), 2);
        assert_eq!(r.duplicates, ["a"]);
        assert_eq!(duplicates(&c, &id, &["a".into(), "c".into()]).unwrap(), ["a"]);
        assert_eq!(playlists_containing(&c, "a"), [id.clone()]);
        assert!(playlists_containing(&c, "c").is_empty());
        // "Add anyway" works now (Limusic's UNIQUE refused it).
        let r = add(&c, &id, vec![song("a", "A")], true, Some(0), 3).unwrap();
        assert_eq!(r.added[0].position, 0);
        assert_eq!(rows(&c, &id), ["a", "a", "b"]);
        let e = entries(&c, &id).unwrap();
        // Move the last row to the front, then undo with the previous order.
        let prev = move_rows(&c, &id, &[e[2].row_id], Some(e[0].row_id), 4).unwrap();
        assert_eq!(rows(&c, &id), ["b", "a", "a"]);
        set_order(&c, &id, &prev, 5).unwrap();
        assert_eq!(rows(&c, &id), ["a", "a", "b"]);
        // Remove the middle row and put it back where it was.
        let removed = remove(&c, &id, &[e[1].row_id], 6).unwrap();
        assert_eq!(removed[0].position, 1);
        assert_eq!(rows(&c, &id), ["a", "b"]);
        insert_entries(&c, &id, removed.clone(), 7).unwrap();
        let after = entries(&c, &id).unwrap();
        assert_eq!(after[1].row_id, removed[0].row_id);
        assert_eq!(after[1].added_at, removed[0].added_at);
        // Rename, then soft delete and restore.
        let p = update_playlist(&c, &id, Some("Road trip"), Some("vibes"), 8).unwrap();
        assert_eq!((p.name.as_str(), p.description.as_str(), p.count), ("Road trip", "vibes", 3));
        assert!(update_playlist(&c, &id, Some("  "), None, 8).is_err());
        delete_playlist(&c, &id, 9).unwrap();
        assert!(playlists(&c).is_empty());
        assert!(playlists_containing(&c, "a").is_empty(), "deleted playlists don't count");
        assert!(entries(&c, &id).is_err());
        restore_playlist(&c, &id).unwrap();
        assert_eq!(playlists(&c).len(), 1);
    }

    #[test]
    fn deleted_playlists_are_purged_after_thirty_days() {
        let d = db();
        let c = d.conn();
        let now = now_secs();
        let p = create_playlist(&c, Some("x"), None, None, 1).unwrap();
        pin(&c, &p.id, None, None, 1).unwrap();
        delete_playlist(&c, &p.id, 2).unwrap();
        purge_deleted(&c);
        assert!(restore_playlist(&c, &p.id).is_err());
        assert!(pins(&c).is_empty());
        // stream: magic-d4 — one deleted yesterday survives a relaunch and can be recovered.
        let q = create_playlist(&c, Some("Keep"), None, None, 1).unwrap();
        add(&c, &q.id, vec![song("a", "A")], false, None, 1).unwrap();
        delete_playlist(&c, &q.id, now - 86_400).unwrap();
        purge_deleted(&c);
        let gone = deleted_playlists(&c);
        assert_eq!(gone.len(), 1);
        assert_eq!((gone[0].id.as_str(), gone[0].name.as_str(), gone[0].count), (q.id.as_str(), "Keep", 1));
        assert_eq!(gone[0].purge_at, now - 86_400 + DELETED_KEEP_SECS);
        assert_eq!(gone[0].thumbnail.as_deref(), Some("https://img/x.jpg"));
        assert!(playlists(&c).iter().all(|l| l.id != q.id));
        assert_eq!(restore_playlist(&c, &q.id).unwrap().name, "Keep");
        assert!(deleted_playlists(&c).is_empty());
        // …and past the window it is purged.
        delete_playlist(&c, &q.id, now - DELETED_KEEP_SECS - 1).unwrap();
        purge_deleted(&c);
        assert!(deleted_playlists(&c).is_empty());
        assert!(restore_playlist(&c, &q.id).is_err());
    }

    #[test]
    fn folders_nest_move_and_restore() {
        let d = db();
        let c = d.conn();
        let a = create_folder(&c, Some("A"), None, 1).unwrap();
        let b = create_folder(&c, Some("B"), None, 1).unwrap();
        let a1 = create_folder(&c, Some("A1"), Some(&a.id), 1).unwrap();
        assert_eq!(a1.parent_id.as_deref(), Some(a.id.as_str()));
        // No cycles.
        assert_eq!(move_folder(&c, &a.id, Some(&a1.id), None, 2).unwrap_err(), "folder_cycle");
        assert_eq!(move_folder(&c, &a.id, Some(&a.id), None, 2).unwrap_err(), "folder_cycle");
        // B to the front of the root.
        let prev = move_folder(&c, &b.id, None, Some(0), 2).unwrap();
        assert_eq!(prev, Place { folder_id: None, index: 1 });
        let root: Vec<String> =
            folders(&c).into_iter().filter(|f| f.parent_id.is_none()).map(|f| f.name).collect();
        assert_eq!(root, ["B", "A"]);
        // Playlists into folders, ordered, and moved between them.
        let p1 = create_playlist(&c, Some("p1"), None, Some(&a1.id), 3).unwrap();
        let p2 = create_playlist(&c, Some("p2"), None, None, 3).unwrap();
        assert_eq!(set_item_folder(&c, &p2.id, Some(&a1.id), Some(0)).unwrap(), None);
        assert_eq!(folder(&c, &a1.id).unwrap().items, [p2.id.clone(), p1.id.clone()]);
        assert_eq!(playlist(&c, &p1.id).unwrap().folder_id.as_deref(), Some(a1.id.as_str()));
        let was = set_item_folder(&c, &p2.id, Some(&b.id), None).unwrap();
        assert_eq!(was, Some(Place { folder_id: Some(a1.id.clone()), index: 0 }));
        // Deleting A moves A1 up to the root; restoring puts it back.
        let snap = delete_folder(&c, &a.id).unwrap();
        assert_eq!(folder(&c, &a1.id).unwrap().parent_id, None);
        restore_folder(&c, &snap).unwrap();
        assert_eq!(folder(&c, &a1.id).unwrap().parent_id.as_deref(), Some(a.id.as_str()));
        // Deleting A1 (inside A) moves its playlist up into A.
        let snap = delete_folder(&c, &a1.id).unwrap();
        assert_eq!(folder(&c, &a.id).unwrap().items, [p1.id.clone()]);
        restore_folder(&c, &snap).unwrap();
        assert!(folder(&c, &a.id).unwrap().items.is_empty());
        assert_eq!(folder(&c, &a1.id).unwrap().items, [p1.id.clone()]);
    }

    #[test]
    fn pins_max_four_ordered() {
        let d = db();
        let c = d.conn();
        for id in ["VLLM", "LOCALPLAYLIST:1", "MPRE1", "UC1"] {
            pin(&c, id, None, None, 1).unwrap();
        }
        assert_eq!(pin(&c, "UC2", None, None, 1).unwrap_err(), "pins_full");
        // Re-pinning is a move, not a fifth pin.
        let p = pin(&c, "UC1", None, Some(0), 1).unwrap();
        assert_eq!(
            p.iter().map(|p| p.item_id.as_str()).collect::<Vec<_>>(),
            ["UC1", "VLLM", "LOCALPLAYLIST:1", "MPRE1"]
        );
        assert_eq!(p[1].kind, "liked");
        let at = unpin(&c, "VLLM").unwrap();
        assert_eq!(at, Some(1));
        pin(&c, "VLLM", None, at, 1).unwrap();
        assert_eq!(pins(&c)[1].item_id, "VLLM");
        let p = reorder_pins(&c, &["MPRE1".into()]).unwrap();
        assert_eq!(p[0].item_id, "MPRE1");
        assert_eq!(p.iter().map(|p| p.position).collect::<Vec<_>>(), [0, 1, 2, 3]);
    }

    #[test]
    fn recents_ordering() {
        let d = db();
        let c = d.conn();
        note_played(&c, "MPRE1", None, Some(&json!({"kind": "album", "id": "MPRE1"})), 10).unwrap();
        note_played(&c, "VLLM", None, None, 20).unwrap();
        note_played(&c, "MPRE1", None, None, 30).unwrap();
        note_played(&c, "UC1", Some("artist"), None, 25).unwrap();
        let r = recent_contexts(&c, 10);
        assert_eq!(
            r.iter().map(|r| r.item_id.as_str()).collect::<Vec<_>>(),
            ["MPRE1", "UC1", "VLLM"]
        );
        // The card survives an update that sends none.
        assert!(r[0].card.is_some());
        assert_eq!(r[2].kind, "liked");
        // A late stamp never moves an item back in time.
        note_played(&c, "MPRE1", None, None, 5).unwrap();
        assert_eq!(last_played(&c)["MPRE1"], 30);
        // Songs from listens, de-duplicated, newest first.
        drop(c); // record_listen takes the same lock
        for (id, at) in [("a", 1), ("b", 2), ("a", 3)] {
            d.record_listen(id, &serde_json::to_string(&song(id, id)).unwrap(), at, 30);
        }
        let t = recent_tracks(&d.conn(), 10);
        assert_eq!(
            t.iter().map(|t| (t.song.video_id.as_str(), t.plays)).collect::<Vec<_>>(),
            [("a", 2), ("b", 1)]
        );
    }

    #[test]
    fn hidden_songs_and_private_session() {
        let d = db();
        let c = d.conn();
        assert!(hide_song(&c, "a", Some(song("a", "A")), None, 1).unwrap());
        assert!(!hide_song(&c, "a", None, None, 2).unwrap());
        hide_song(&c, "b", None, Some("LOCALPLAYLIST:1"), 3).unwrap();
        assert_eq!(hidden_ids(&c), ["a"]);
        assert_eq!(hidden_songs(&c, Some("LOCALPLAYLIST:1")).len(), 2);
        assert_eq!(hidden_songs(&c, Some("other")).len(), 1);
        let gone = unhide_song(&c, "b", Some("LOCALPLAYLIST:1")).unwrap().unwrap();
        assert_eq!(gone.context_id.as_deref(), Some("LOCALPLAYLIST:1"));
        assert!(!private_session(&c, 100).enabled);
        assert!(set_private_session(&c, true, 100).unwrap().enabled);
        assert!(private_session(&c, 100 + PRIVATE_TTL_SECS - 1).enabled);
        assert!(!private_session(&c, 100 + PRIVATE_TTL_SECS).enabled);
        set_private_session(&c, false, 200).unwrap();
        drop(c);
        assert!(!is_private(&d));
    }

    #[test]
    fn legacy_local_storage_migrates_once() {
        let d = db();
        let c = d.conn();
        let blob = json!({
            "picks": [],
            "saved": [
                {"kind": "album", "id": "MPRE1", "title": "Newest", "synced": true},
                {"kind": "artist", "id": "UC1", "title": "Older"},
                {"kind": "bogus", "id": "x"}
            ],
            "pins": ["LOCALPLAYLIST:1", "MPRE1", "VLLM", "UC1", "UC9"],
            "recent": { "MPRE1": {"kind": "album", "id": "MPRE1", "title": "Newest", "at": 5_000} }
        });
        let r = migrate_legacy(&c, &blob, 1_000).unwrap();
        assert_eq!((r.saved, r.pins, r.recent, r.already), (2, 4, 1, false));
        let s = saved(&c, None);
        assert_eq!(s[0].id, "MPRE1");
        assert!(s[0].synced);
        assert!(s[0].card.get("synced").is_none());
        assert_eq!(last_played(&c)["MPRE1"], 5);
        assert_eq!(pins(&c)[0].item_id, "LOCALPLAYLIST:1");
        assert!(migrate_legacy(&c, &blob, 2_000).unwrap().already);
        assert_eq!(saved(&c, None).len(), 2);
        assert!(snapshot(&c, 1).legacy_migrated);
    }

    #[test]
    fn export_writes_every_format() {
        let d = db();
        let c = d.conn();
        like(&c, vec![song("a", "Song, \"quoted\"")], 0).unwrap();
        let p = create_playlist(&c, Some("Mix/1"), None, None, 1).unwrap();
        add(&c, &p.id, vec![song("b", "B & <C>")], false, None, 1).unwrap();
        let dir = std::env::temp_dir().join(format!("encore-export-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.zip");
        let r = export_zip(&c, json!([]), &path, 86_400).unwrap();
        assert_eq!((r.liked_songs, r.playlists), (1, 1));
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut read = |n: &str| {
            use std::io::Read;
            let mut s = String::new();
            z.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let doc: Value = serde_json::from_str(&read("library.json")).unwrap();
        assert_eq!(doc["format"], "encore-library");
        assert_eq!(doc["likedSongs"][0]["song"]["video_id"], "a");
        assert_eq!(doc["playlists"][0]["tracks"][0]["song"]["video_id"], "b");
        let csv = read("Liked Songs.csv");
        assert!(csv.starts_with("Track URI,Track Name"));
        assert!(csv.contains("\"Song, \"\"quoted\"\"\""));
        assert!(csv.contains("1970-01-01T00:00:00Z"));
        assert!(csv.contains(",185000,"));
        let m3u = read("playlists/Mix_1.m3u8");
        assert!(m3u.contains("#EXTINF:185,Artist - B & <C>\nhttps://music.youtube.com/watch?v=b"));
        let xspf = read("playlists/Mix_1.xspf");
        assert!(xspf.contains("<title>B &amp; &lt;C&gt;</title>"));
        read("playlists/Mix_1.csv");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_763_200_000), "2025-11-15T09:46:40Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn migration_keeps_existing_rows_and_drops_the_unique() {
        // A Limusic-era DB: tracks in a UNIQUE table, with row ids the UI holds.
        let dir = std::env::temp_dir().join(format!("encore-mig-{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("db.sqlite");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE local_playlists (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL,
                     description TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 CREATE TABLE local_playlist_tracks (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     playlist_id INTEGER NOT NULL, video_id TEXT NOT NULL, song_json TEXT NOT NULL,
                     added_at INTEGER NOT NULL, UNIQUE (playlist_id, video_id));
                 INSERT INTO local_playlists(title, created_at, updated_at) VALUES('Old', 1, 1);",
            )
            .unwrap();
            for (i, v) in ["a", "b", "c"].iter().enumerate() {
                c.execute(
                    "INSERT INTO local_playlist_tracks(playlist_id, video_id, song_json, added_at) VALUES(1, ?1, ?2, ?3)",
                    params![v, serde_json::to_string(&song(v, v)).unwrap(), i as i64],
                )
                .unwrap();
            }
            c.execute("DELETE FROM local_playlist_tracks WHERE video_id = 'c'", []).unwrap();
        }
        let d = Db::open(&path).unwrap();
        let c = d.conn();
        assert_eq!(rows(&c, "LOCALPLAYLIST:1"), ["a", "b"]);
        let r = add(&c, "LOCALPLAYLIST:1", vec![song("a", "a")], true, None, 9).unwrap();
        // Duplicates allowed, and the deleted row's id (3) is never handed out again.
        assert_eq!(r.added[0].row_id, 4);
        drop(c);
        drop(d);
        // A second open is a no-op.
        let d = Db::open(&path).unwrap();
        assert_eq!(rows(&d.conn(), "LOCALPLAYLIST:1"), ["a", "b", "a"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
