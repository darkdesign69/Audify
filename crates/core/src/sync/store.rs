//! The change log and the merge (docs/SYNC.md §2–3).
//!
//! **Capture.** Every synced table gets generated SQLite triggers. Whatever code path writes a row
//! (library.rs, db.rs, podcasts, smart, an import, an undo), the trigger records the new field
//! values in `sync_field`, stamped with a hybrid logical clock and this device's id. That is the
//! central mutation layer: no write path needs a hook, and a new one can't forget it.
//!
//! **State, not a journal.** `sync_field` keeps the *latest* version of each (entity, key, field),
//! with a per-row `seq` that grows on every change. A peer that last saw `seq = n` gets every field
//! with a higher seq: superseded versions are never sent, so a progress bar that moved 500 times
//! offline costs one field.
//!
//! **Merge.** Per-field last-writer-wins on `(hlc, device)`. Rows exist or not through the `_del`
//! pseudo-field, so likes, saves, follows and playlist membership are LWW element sets with
//! tombstones, and a delete beats a concurrent edit of another field (the reorder of a track
//! someone else removed doesn't bring it back). Remote fields that win are written to
//! `sync_field` and the row is rebuilt from that state with the capture switched off, so applying
//! never records new ops and never echoes. Applying the same op twice is a no-op.
//!
//! **Identity.** Rows are matched across devices by natural keys (video id, played-at second,
//! episode id, ...) or, where the local key is an autoincrement integer (`local_playlists`,
//! `local_playlist_tracks`), by a random `uid` column added here. References to those integers
//! (`LOCALPLAYLIST:<n>` in pins, folders, recents; a track's `playlist_id`; a private listen's
//! `listen_id`) are translated to the global id on the way out and back on the way in.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::LOCAL_PLAYLIST_PREFIX;

/// How a column's local value maps to what goes on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Plain,
    /// The row's random global id (`uid TEXT`), filled by a trigger on insert.
    Uid,
    /// A `local_playlists.id` integer → that playlist's uid.
    PlaylistRef,
    /// A library item id string; `LOCALPLAYLIST:<n>` ↔ `LOCALPLAYLIST@<uid>`, others unchanged.
    ItemRef,
    /// A `listens.id` integer → `[video_id, played_at]`.
    ListenRef,
}

#[derive(Clone, Copy, Debug)]
pub struct Col {
    pub name: &'static str,
    pub kind: Kind,
}

const fn c(name: &'static str) -> Col {
    Col { name, kind: Kind::Plain }
}
const fn r(name: &'static str, kind: Kind) -> Col {
    Col { name, kind }
}

/// One synced table. `entity` is the name on the wire (the table name).
pub struct Spec {
    pub table: &'static str,
    pub key: &'static [Col],
    pub fields: &'static [Col],
    /// Extra condition on the row (`R` is NEW or OLD), e.g. the settings allowlist.
    pub filter: Option<&'static str>,
    /// A seconds column the initial snapshot stamps the row with, so the first merge of two
    /// non-empty libraries keeps the more recently changed version.
    pub ts: Option<&'static str>,
    /// A JSON snapshot of the podcast episode the row points at, sent along with an insert so the
    /// other device can show a saved episode or a progress bar of a show it never loaded.
    pub episode: Option<&'static str>,
}

/// Settings that sync. Everything else in `settings` is device-local (docs/SYNC.md §4): volume,
/// output, window and tray behaviour, proxies, downloads, accounts and cookies, caches, queue,
/// migration markers, private session. `blocked_artists` syncs as a set (`BLOCKED`), not here.
pub const SYNCED_SETTINGS: &[&str] = &[
    "quality",
    "normalize_volume",
    "enable_history",
    "autoplay",
    "hide_videos",
    "prevent_duplicates",
    "lyrics_providers",
    "lyrics_prefer_words",
    "music_videos",
    "videos_first",
    "podcast_audio_only",
    "sticky_shuffle",
    "shuffle_whole_queue",
    "crossfade",
    "crossfade_secs",
    "locale",
    "show_unplayable",
    "lastfm_primary_artist",
    "lastfm_primary_strict",
    "shuffle_mode",
    "explicit_filter",
    "episode_speed",
    "gapless",
    "podcast_sponsorblock",
    "podcast_sponsorblock_categories",
    "podcast_country",
    "podcast_youtube_match",
    "charts_country",
    "whatsnew_seen_at",
];

/// The blocked-artist list lives as a JSON array in one settings row (blocked.rs). It syncs one
/// artist at a time, as entity `blocked_artist` keyed by the artist's channel id (or name).
pub const BLOCKED: &str = "blocked_artist";
const BLOCKED_SETTING: &str = "blocked_artists";

/// In apply order: a playlist before its tracks and the pins pointing at it, a listen before the
/// private flag on it.
pub const SPECS: &[Spec] = &[
    Spec { table: "settings", key: &[c("key")], fields: &[c("value")], filter: Some("settings"), ts: None, episode: None },
    Spec {
        table: "local_playlists",
        key: &[r("uid", Kind::Uid)],
        fields: &[c("title"), c("description"), c("created_at"), c("updated_at"), c("deleted_at")],
        filter: None,
        ts: Some("updated_at"),
        episode: None,
    },
    Spec {
        table: "local_playlist_tracks",
        key: &[r("uid", Kind::Uid)],
        fields: &[r("playlist_id", Kind::PlaylistRef), c("video_id"), c("song_json"), c("added_at"), c("pos")],
        filter: None,
        ts: Some("added_at"),
        episode: None,
    },
    Spec {
        table: "liked_songs",
        key: &[c("video_id")],
        fields: &[
            c("title"), c("artists"), c("artists_json"), c("album"), c("album_id"), c("duration_secs"),
            c("thumbnail"), c("explicit"), c("song_json"), c("added_at"),
        ],
        filter: None,
        ts: Some("added_at"),
        episode: None,
    },
    Spec {
        table: "library_items",
        key: &[c("id")],
        fields: &[c("kind"), c("title"), c("subtitle"), c("thumbnail"), c("card_json"), c("added_at")],
        filter: None,
        ts: Some("added_at"),
        episode: None,
    },
    Spec {
        table: "podcast_follows",
        key: &[c("id")],
        fields: &[c("title"), c("author"), c("thumb"), c("feed_url"), c("yt_browse_id"), c("followed_at")],
        filter: None,
        ts: Some("followed_at"),
        episode: None,
    },
    Spec {
        table: "lib_folders",
        key: &[c("id")],
        fields: &[c("name"), c("parent_id"), c("position"), c("created_at"), c("updated_at")],
        filter: None,
        ts: Some("updated_at"),
        episode: None,
    },
    Spec { table: "lib_folder_items", key: &[r("item_id", Kind::ItemRef)], fields: &[c("folder_id"), c("position")], filter: None, ts: None, episode: None },
    Spec { table: "lib_pins", key: &[r("item_id", Kind::ItemRef)], fields: &[c("kind"), c("position"), c("pinned_at")], filter: None, ts: Some("pinned_at"), episode: None },
    Spec { table: "lib_played", key: &[r("item_id", Kind::ItemRef)], fields: &[c("kind"), c("card_json"), c("played_at")], filter: None, ts: Some("played_at"), episode: None },
    Spec { table: "lib_touched", key: &[r("item_id", Kind::ItemRef)], fields: &[c("touched_at")], filter: None, ts: Some("touched_at"), episode: None },
    Spec {
        table: "hidden_songs",
        key: &[c("video_id"), r("context_id", Kind::ItemRef)],
        fields: &[c("song_json"), c("hidden_at")],
        filter: None,
        ts: Some("hidden_at"),
        episode: None,
    },
    Spec { table: "listen_songs", key: &[c("video_id")], fields: &[c("song_json")], filter: None, ts: None, episode: None },
    Spec { table: "listens", key: &[c("video_id"), c("played_at")], fields: &[c("secs")], filter: None, ts: Some("played_at"), episode: None },
    Spec {
        table: "episode_listens",
        key: &[c("video_id"), c("played_at")],
        fields: &[c("secs"), c("song_json"), c("show_id")],
        filter: None,
        ts: Some("played_at"),
        episode: None,
    },
    Spec {
        table: "smart_events",
        key: &[c("video_id"), c("at")],
        fields: &[c("artist_key"), c("artist"), c("kind"), c("secs"), c("duration"), c("from_rec"), c("private")],
        filter: None,
        ts: Some("at"),
        episode: None,
    },
    Spec { table: "smart_private_listens", key: &[r("listen_id", Kind::ListenRef)], fields: &[], filter: None, ts: None, episode: None },
    Spec {
        table: "polish_listen_contexts",
        key: &[c("video_id"), c("at"), c("context_key")],
        fields: &[c("json")],
        filter: None,
        ts: Some("at"),
        episode: None,
    },
    Spec { table: "taste_excluded", key: &[r("id", Kind::ItemRef)], fields: &[c("kind"), c("title"), c("at")], filter: None, ts: Some("at"), episode: None },
    Spec { table: "taste_excluded_tracks", key: &[r("context_id", Kind::ItemRef), c("video_id")], fields: &[], filter: None, ts: None, episode: None },
    Spec {
        table: "podcast_progress",
        key: &[c("episode_id")],
        fields: &[c("position_s"), c("duration_s"), c("played"), c("played_at"), c("updated_at")],
        filter: None,
        ts: Some("updated_at"),
        episode: Some("episode_id"),
    },
    Spec { table: "podcast_saved", key: &[c("episode_id")], fields: &[c("saved_at")], filter: None, ts: Some("saved_at"), episode: Some("episode_id") },
    Spec { table: "audiobook_books", key: &[c("id")], fields: &[c("json"), c("saved_at"), c("opened_at")], filter: None, ts: Some("opened_at"), episode: None },
];

const EPISODE_COLS: &[&str] = &[
    "id", "show_id", "show_title", "title", "published_at", "duration_s", "description", "thumbnail",
    "video_id", "enclosure_url", "guid", "is_video", "first_seen", "chapters_url",
];

/// One changed row on the wire: the fields that changed together, under one clock reading.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Op {
    /// Entity (table name, or `blocked_artist`).
    pub e: String,
    /// The key: a JSON array of the key columns' global values.
    pub k: String,
    /// Hybrid logical clock (milliseconds, bumped past the last value seen).
    pub h: i64,
    /// The device that made the change.
    pub d: String,
    /// field → value. `_del`: 1 tombstone, 0 present. `_ep`: the episode snapshot.
    pub f: BTreeMap<String, Value>,
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Applied {
    /// Fields that won and were stored.
    pub fields: usize,
    /// Entities whose rows changed on this device.
    pub entities: BTreeSet<String>,
    /// Setting keys that changed (the app re-applies live ones: crossfade, locale, ...).
    pub settings: Vec<String>,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The SQL that reads a column's wire value off row alias `row` (NEW, OLD or a table alias).
fn out_expr(spec: &Spec, col: &Col, row: &str) -> String {
    let v = format!("{row}.{}", col.name);
    match col.kind {
        Kind::Plain => v,
        Kind::Uid => format!("coalesce({v}, (SELECT uid FROM {} WHERE rowid = {row}.rowid))", spec.table),
        Kind::PlaylistRef => playlist_uid(&v),
        Kind::ItemRef => {
            let n = LOCAL_PLAYLIST_PREFIX.len() + 1;
            format!(
                "CASE WHEN substr({v}, 1, {len}) = {pre} THEN \
                 ({at} || {uid}) \
                 ELSE {v} END",
                len = LOCAL_PLAYLIST_PREFIX.len(),
                pre = quote(LOCAL_PLAYLIST_PREFIX),
                at = quote(&global_prefix()),
                uid = playlist_uid(&format!("CAST(substr({v}, {n}) AS INTEGER)")),
            )
        }
        Kind::ListenRef => format!("(SELECT json_array(video_id, played_at) FROM listens WHERE id = {v})"),
    }
}

/// A local playlist id's uid, also after the playlist was deleted (a pin or a track row removed
/// in the same breath still has to name it). Ids are AUTOINCREMENT, never reused.
fn playlist_uid(id: &str) -> String {
    format!(
        "coalesce((SELECT uid FROM local_playlists WHERE id = {id}), (SELECT uid FROM sync_gone_playlists WHERE id = {id}))"
    )
}

fn global_prefix() -> String {
    format!("{}@", LOCAL_PLAYLIST_PREFIX.trim_end_matches(':'))
}

fn filter_sql(spec: &Spec, row: &str) -> String {
    match spec.filter {
        Some("settings") => {
            let list: Vec<String> = SYNCED_SETTINGS.iter().map(|k| quote(k)).collect();
            format!(" AND {row}.key IN ({})", list.join(", "))
        }
        _ => String::new(),
    }
}

/// `SELECT <key json> AS k WHERE <every translated key part resolves>`.
fn key_select(spec: &Spec, row: &str) -> (String, String) {
    let parts: Vec<String> = spec.key.iter().map(|k| out_expr(spec, k, row)).collect();
    let key = format!("json_array({})", parts.join(", "));
    let ok: Vec<String> = parts.iter().map(|p| format!("({p}) IS NOT NULL")).collect();
    (key, ok.join(" AND "))
}

/// (field, SQL expression) for a row: `_del` and `_ep` first when `with_meta`, then the fields.
fn field_exprs(spec: &Spec, row: &str, with_meta: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if with_meta {
        out.push(("_del".to_string(), "0".to_string()));
        if let Some(col) = spec.episode {
            let pairs: Vec<String> = EPISODE_COLS.iter().map(|c| format!("'{c}', e.{c}")).collect();
            out.push((
                "_ep".to_string(),
                format!("(SELECT json_object({}) FROM podcast_episodes e WHERE e.id = {row}.{col})", pairs.join(", ")),
            ));
        }
    }
    for f in spec.fields {
        out.push((f.name.to_string(), out_expr(spec, f, row)));
    }
    out
}

/// `SELECT 'f' AS f, value AS v [WHERE changed] UNION ALL ...` for a trigger body.
fn fields_union(spec: &Spec, row: &str, changed_only: bool, with_meta: bool) -> Vec<String> {
    field_exprs(spec, row, with_meta)
        .into_iter()
        .map(|(name, expr)| {
            let mut s = format!("SELECT {} AS f, {expr} AS v", quote(&name));
            if changed_only {
                s.push_str(&format!(" WHERE OLD.{name} IS NOT NEW.{name}"));
            }
            s
        })
        .collect()
}

const LIVE: &str = "(SELECT capture FROM sync_ctl WHERE id = 1) = 1";
const TICK: &str = "UPDATE sync_ctl SET hlc = max(hlc + 1, CAST(unixepoch('subsec') * 1000 AS INTEGER)) WHERE id = 1;";

/// Write fields from inside a trigger: delete the old versions, insert the new ones (a fresh seq).
/// Not `INSERT OR REPLACE`: SQLite lets the conflict clause of the statement that fired the
/// trigger (an upsert, `INSERT OR IGNORE`) override the one inside it, which would abort the
/// app's own write on the UNIQUE index.
fn put(ekf: &str, rest: &str, from_where: &str) -> String {
    format!(
        "DELETE FROM sync_field WHERE (entity, key, field) IN (SELECT {ekf} {from_where});
         INSERT INTO sync_field(entity, key, field, value, hlc, dev) SELECT {ekf}, {rest} {from_where};"
    )
}

fn record(spec: &Spec, row: &str, fields: &[String]) -> String {
    let (key, ok) = key_select(spec, row);
    if fields.is_empty() {
        return String::new();
    }
    put(
        &format!("{}, kk.k, ff.f", quote(spec.table)),
        "ff.v, c.hlc, c.dev",
        &format!("FROM sync_ctl c, (SELECT {key} AS k WHERE {ok}) kk, ({}) ff WHERE c.id = 1", fields.join(" UNION ALL ")),
    )
}

fn tombstone(spec: &Spec, row: &str) -> String {
    let (key, ok) = key_select(spec, row);
    put(
        &format!("{}, kk.k, '_del'", quote(spec.table)),
        "1, c.hlc, c.dev",
        &format!("FROM sync_ctl c, (SELECT {key} AS k WHERE {ok}) kk WHERE c.id = 1"),
    )
}

fn has_uid(spec: &Spec) -> bool {
    spec.key.iter().any(|k| k.kind == Kind::Uid)
}

/// The capture triggers for one table.
fn triggers(spec: &Spec) -> Vec<String> {
    let t = spec.table;
    let mut out = Vec::new();
    let uid_fill = if has_uid(spec) {
        format!("UPDATE {t} SET uid = lower(hex(randomblob(16))) WHERE rowid = NEW.rowid AND uid IS NULL;")
    } else {
        String::new()
    };
    if has_uid(spec) {
        // Always on, synced or not, so every row has its global id before it is ever sent.
        out.push(format!(
            "CREATE TRIGGER sync_{t}_uid AFTER INSERT ON {t} WHEN NEW.uid IS NULL BEGIN {uid_fill} END;"
        ));
    }
    out.push(format!(
        "CREATE TRIGGER sync_{t}_ins AFTER INSERT ON {t} WHEN {LIVE}{f} BEGIN {uid_fill} {TICK} {rec} END;",
        f = filter_sql(spec, "NEW"),
        rec = record(spec, "NEW", &fields_union(spec, "NEW", false, true)),
    ));
    out.push(format!(
        "CREATE TRIGGER sync_{t}_del AFTER DELETE ON {t} WHEN {LIVE}{f} BEGIN {TICK} {rec} END;",
        f = filter_sql(spec, "OLD"),
        rec = tombstone(spec, "OLD"),
    ));
    let key_same: Vec<String> =
        spec.key.iter().map(|k| format!("OLD.{0} IS NEW.{0}", k.name)).collect();
    let key_same = key_same.join(" AND ");
    let changed = fields_union(spec, "NEW", true, false);
    if !changed.is_empty() {
        out.push(format!(
            "CREATE TRIGGER sync_{t}_upd AFTER UPDATE ON {t} WHEN {LIVE} AND {key_same}{f} BEGIN {TICK} {rec} END;",
            f = filter_sql(spec, "NEW"),
            rec = record(spec, "NEW", &changed),
        ));
    }
    // A key that changes is a delete of the old row and an insert of the new one. (Settings keys
    // never change: their rows are upserts by key.)
    if spec.filter.is_none() {
        out.push(format!(
            "CREATE TRIGGER sync_{t}_rekey AFTER UPDATE ON {t} WHEN {LIVE} AND NOT ({key_same}) BEGIN {TICK} {del} {ins} END;",
            del = tombstone(spec, "OLD"),
            ins = record(spec, "NEW", &fields_union(spec, "NEW", false, true)),
        ));
    }
    out
}

/// The blocked-artist list, diffed element by element in SQL.
fn blocked_triggers() -> Vec<String> {
    let key = "coalesce(json_extract(j.value, '$.id'), json_extract(j.value, '$.name'))";
    let added = |new: &str, old: &str| {
        put(
            &format!("'{BLOCKED}', json_array({key}), ff.f"),
            "CASE ff.f WHEN '_del' THEN 0 ELSE j.value END, c.hlc, c.dev",
            &format!(
                "FROM sync_ctl c, json_each(CASE WHEN json_valid({new}) THEN {new} ELSE '[]' END) j,
                      (SELECT '_del' AS f UNION ALL SELECT 'v') ff
                 WHERE c.id = 1 AND {key} IS NOT NULL AND {key} NOT IN
                   (SELECT {okey} FROM json_each(CASE WHEN json_valid({old}) THEN {old} ELSE '[]' END) o)",
                okey = "coalesce(json_extract(o.value, '$.id'), json_extract(o.value, '$.name'))",
            ),
        )
    };
    let removed = |new: &str, old: &str| {
        put(
            &format!("'{BLOCKED}', json_array({key}), '_del'"),
            "1, c.hlc, c.dev",
            &format!(
                "FROM sync_ctl c, json_each(CASE WHEN json_valid({old}) THEN {old} ELSE '[]' END) j
                 WHERE c.id = 1 AND {key} IS NOT NULL AND {key} NOT IN
                   (SELECT {nkey} FROM json_each(CASE WHEN json_valid({new}) THEN {new} ELSE '[]' END) n)",
                nkey = "coalesce(json_extract(n.value, '$.id'), json_extract(n.value, '$.name'))",
            ),
        )
    };
    let w = format!("WHEN {LIVE} AND NEW.key = '{BLOCKED_SETTING}'");
    let wo = format!("WHEN {LIVE} AND OLD.key = '{BLOCKED_SETTING}'");
    vec![
        format!("CREATE TRIGGER sync_blocked_ins AFTER INSERT ON settings {w} BEGIN {TICK} {} END;", added("NEW.value", "'[]'")),
        format!(
            "CREATE TRIGGER sync_blocked_upd AFTER UPDATE ON settings {w} AND OLD.key = NEW.key BEGIN {TICK} {} {} END;",
            added("NEW.value", "OLD.value"),
            removed("NEW.value", "OLD.value")
        ),
        format!("CREATE TRIGGER sync_blocked_del AFTER DELETE ON settings {wo} BEGIN {TICK} {} END;", removed("'[]'", "OLD.value")),
    ]
}

fn table_exists(conn: &Connection, t: &str) -> bool {
    conn.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [t], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut st) = conn.prepare(&format!("PRAGMA table_info({table})")) else { return false };
    let names = st.query_map([], |r| r.get::<_, String>(1));
    names.map(|rows| rows.flatten().any(|n| n == column)).unwrap_or(false)
}

/// Create the sync tables and (re)create every capture trigger. Run at every launch after all the
/// other modules created their tables, so a spec change takes effect on the next start.
pub fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_ctl (
             id      INTEGER PRIMARY KEY CHECK (id = 1),
             capture INTEGER NOT NULL DEFAULT 0,
             hlc     INTEGER NOT NULL DEFAULT 0,
             dev     TEXT NOT NULL
         );
         INSERT OR IGNORE INTO sync_ctl(id, capture, hlc, dev) VALUES(1, 0, 0, lower(hex(randomblob(16))));
         CREATE TABLE IF NOT EXISTS sync_field (
             seq    INTEGER PRIMARY KEY AUTOINCREMENT,
             entity TEXT NOT NULL,
             key    TEXT NOT NULL,
             field  TEXT NOT NULL,
             value,
             hlc    INTEGER NOT NULL,
             dev    TEXT NOT NULL,
             UNIQUE (entity, key, field)
         );
         CREATE INDEX IF NOT EXISTS sync_field_row ON sync_field(entity, key);
         CREATE TABLE IF NOT EXISTS sync_gone_playlists (
             id  INTEGER PRIMARY KEY,
             uid TEXT
         );
         CREATE TABLE IF NOT EXISTS sync_pending (
             entity TEXT NOT NULL,
             key    TEXT NOT NULL,
             PRIMARY KEY (entity, key)
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS sync_group (
             id           INTEGER PRIMARY KEY CHECK (id = 1),
             pair_id      TEXT NOT NULL,
             key          TEXT NOT NULL,
             created_at   INTEGER NOT NULL,
             relay_sent   INTEGER NOT NULL DEFAULT 0,
             relay_recv   INTEGER NOT NULL DEFAULT 0,
             relay_full   INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS sync_peers (
             dev       TEXT PRIMARY KEY,
             name      TEXT NOT NULL DEFAULT '',
             platform  TEXT NOT NULL DEFAULT '',
             addr      TEXT,
             port      INTEGER,
             sent      INTEGER NOT NULL DEFAULT 0,
             recv      INTEGER NOT NULL DEFAULT 0,
             last_seen INTEGER NOT NULL DEFAULT 0
         );",
    )?;
    // Tables some modules create lazily; the triggers need them now.
    crate::smart::taste::ensure_excluded_tables(conn);
    for t in ["local_playlists", "local_playlist_tracks"] {
        if table_exists(conn, t) && !has_column(conn, t, "uid") {
            conn.execute(&format!("ALTER TABLE {t} ADD COLUMN uid TEXT"), [])?;
        }
        if table_exists(conn, t) {
            conn.execute(&format!("UPDATE {t} SET uid = lower(hex(randomblob(16))) WHERE uid IS NULL"), [])?;
            conn.execute(&format!("CREATE UNIQUE INDEX IF NOT EXISTS {t}_uid ON {t}(uid)"), [])?;
        }
    }
    // Drop every trigger this module ever made, then make the current set.
    let old: Vec<String> = {
        let mut st = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'sync\\_%' ESCAPE '\\'")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.flatten().collect()
    };
    let tx = conn.unchecked_transaction()?;
    for name in old {
        tx.execute(&format!("DROP TRIGGER IF EXISTS \"{name}\""), [])?;
    }
    for spec in SPECS {
        if !table_exists(&tx, spec.table) {
            tracing::warn!(table = spec.table, "sync: table missing, not captured");
            continue;
        }
        for sql in triggers(spec) {
            tx.execute_batch(&sql)?;
        }
    }
    if table_exists(&tx, "local_playlists") {
        tx.execute_batch(
            "CREATE TRIGGER sync_local_playlists_gone AFTER DELETE ON local_playlists BEGIN
                 DELETE FROM sync_gone_playlists WHERE id = OLD.id;
                 INSERT INTO sync_gone_playlists(id, uid) VALUES(OLD.id, OLD.uid);
             END;",
        )?;
    }
    for sql in blocked_triggers() {
        tx.execute_batch(&sql)?;
    }
    tx.commit()
}

pub fn device_id(conn: &Connection) -> String {
    conn.query_row("SELECT dev FROM sync_ctl WHERE id = 1", [], |r| r.get(0)).unwrap_or_default()
}

pub fn capturing(conn: &Connection) -> bool {
    conn.query_row("SELECT capture FROM sync_ctl WHERE id = 1", [], |r| r.get::<_, i64>(0)).unwrap_or(0) == 1
}

/// The highest seq in the log (0 when empty).
pub fn head(conn: &Connection) -> i64 {
    conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM sync_field", [], |r| r.get(0)).unwrap_or(0)
}

/// Turn capture on, recording every existing row as present (the initial snapshot). Rows carry
/// their own timestamp column as the clock (`base` + seconds × 1000), or `base` without one, so
/// any edit made after pairing beats the snapshot and between two snapshots the newer row wins.
/// `base` breaks the remaining ties: the inviting device passes 1 and the joining one 0, so
/// settings that differ at first pairing take the inviter's value.
pub fn enable(conn: &Connection, base: i64) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM sync_field", [])?;
    tx.execute("DELETE FROM sync_pending", [])?;
    for spec in SPECS {
        if !table_exists(&tx, spec.table) {
            continue;
        }
        let (key, ok) = key_select(spec, "R");
        let hlc = match spec.ts {
            Some(ts) => format!("{base} + MAX(COALESCE(R.{ts}, 0), 0) * 1000"),
            None => base.to_string(),
        };
        let where_filter = filter_sql(spec, "R");
        // One statement per field: SQLite has no LATERAL, so a row can't feed a derived table.
        for (name, expr) in field_exprs(spec, "R", true) {
            tx.execute_batch(&format!(
                "INSERT OR REPLACE INTO sync_field(entity, key, field, value, hlc, dev)
                   SELECT {e}, {key}, {f}, {expr}, {hlc}, c.dev
                   FROM {t} R, sync_ctl c
                   WHERE c.id = 1 AND {ok} {where_filter};",
                e = quote(spec.table),
                f = quote(&name),
                t = spec.table,
            ))?;
        }
    }
    // The blocked list, one artist per key.
    tx.execute_batch(&format!(
        "INSERT OR REPLACE INTO sync_field(entity, key, field, value, hlc, dev)
           SELECT '{BLOCKED}', json_array(coalesce(json_extract(j.value, '$.id'), json_extract(j.value, '$.name'))),
                  ff.f, CASE ff.f WHEN '_del' THEN 0 ELSE j.value END, {base}, c.dev
           FROM settings s, sync_ctl c, json_each(CASE WHEN json_valid(s.value) THEN s.value ELSE '[]' END) j,
                (SELECT '_del' AS f UNION ALL SELECT 'v') ff
           WHERE c.id = 1 AND s.key = '{BLOCKED_SETTING}'
             AND coalesce(json_extract(j.value, '$.id'), json_extract(j.value, '$.name')) IS NOT NULL;"
    ))?;
    tx.execute(
        "UPDATE sync_ctl SET capture = 1,
             hlc = max(hlc, (SELECT COALESCE(MAX(hlc), 0) FROM sync_field)) WHERE id = 1",
        [],
    )?;
    tx.commit()
}

/// Turn capture off and forget the log (unpairing). The data itself stays.
pub fn disable(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "UPDATE sync_ctl SET capture = 0 WHERE id = 1;
         DELETE FROM sync_field; DELETE FROM sync_pending;",
    )
}

fn to_json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null | ValueRef::Blob(_) => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
    }
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Sql::Integer(i),
            None => Sql::Real(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

/// Fields with `after < seq <= upto`, oldest first, grouped into ops. `skip_dev` leaves out what
/// that device wrote (it has it); `only_dev` keeps only what that device wrote. At most `limit`
/// fields (a row cut at the limit is fine: fields merge one by one, and a half-arrived row waits in
/// `sync_pending`). Returns the ops, the cursor the receiver is now up to date with (`upto` when
/// everything fitted), and whether more remain.
pub fn ops_since(
    conn: &Connection,
    after: i64,
    upto: i64,
    skip_dev: Option<&str>,
    only_dev: Option<&str>,
    limit: usize,
) -> rusqlite::Result<(Vec<Op>, i64, bool)> {
    let mut st = conn.prepare_cached(
        "SELECT seq, entity, key, field, value, hlc, dev FROM sync_field
         WHERE seq > ?1 AND seq <= ?2 AND (?3 IS NULL OR dev <> ?3) AND (?4 IS NULL OR dev = ?4)
         ORDER BY seq LIMIT ?5",
    )?;
    let mut rows = st.query(params![after, upto, skip_dev, only_dev, (limit as i64).saturating_add(1)])?;
    let mut ops: Vec<Op> = Vec::new();
    let mut last = after;
    let mut n = 0usize;
    let mut more = false;
    while let Some(row) = rows.next()? {
        n += 1;
        if n > limit {
            more = true;
            break;
        }
        let seq: i64 = row.get(0)?;
        let (e, k, f): (String, String, String) = (row.get(1)?, row.get(2)?, row.get(3)?);
        let v = to_json(row.get_ref(4)?);
        let (h, d): (i64, String) = (row.get(5)?, row.get(6)?);
        last = seq;
        match ops.last_mut() {
            Some(op) if op.e == e && op.k == k && op.h == h && op.d == d => {
                op.f.insert(f, v);
            }
            _ => ops.push(Op { e, k, h, d, f: BTreeMap::from([(f, v)]) }),
        }
    }
    Ok((ops, if more { last } else { upto.max(after) }, more))
}

/// Every field, whoever wrote it (a peer that lost its history asks for this).
pub fn all_ops(conn: &Connection) -> rusqlite::Result<Vec<Op>> {
    Ok(ops_since(conn, 0, i64::MAX, None, None, usize::MAX / 2)?.0)
}

fn entity_rank(e: &str) -> usize {
    SPECS.iter().position(|s| s.table == e).unwrap_or(SPECS.len())
}

/// Merge remote ops. Idempotent, order-independent, and records nothing new.
pub fn apply(conn: &Connection, ops: &[Op]) -> rusqlite::Result<Applied> {
    let tx = conn.unchecked_transaction()?;
    let was = capturing(&tx);
    tx.execute("UPDATE sync_ctl SET capture = 0 WHERE id = 1", [])?;
    let mut out = Applied::default();
    let mut dirty: BTreeSet<(usize, String, String)> = BTreeSet::new();
    let mut max_h = 0i64;
    {
        let mut cur = tx.prepare_cached(
            "SELECT hlc, dev FROM sync_field WHERE entity = ?1 AND key = ?2 AND field = ?3",
        )?;
        let mut put = tx.prepare_cached(
            "INSERT OR REPLACE INTO sync_field(entity, key, field, value, hlc, dev) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for op in ops {
            if entity_rank(&op.e) == SPECS.len() && op.e != BLOCKED {
                continue; // an entity this version doesn't know: a newer peer; skip it
            }
            max_h = max_h.max(op.h);
            for (f, v) in &op.f {
                let have: Option<(i64, String)> =
                    cur.query_row(params![op.e, op.k, f], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                let wins = match &have {
                    None => true,
                    Some((h, d)) => (op.h, op.d.as_str()) > (*h, d.as_str()),
                };
                if wins {
                    put.execute(params![op.e, op.k, f, to_sql(v), op.h, op.d])?;
                    out.fields += 1;
                    dirty.insert((entity_rank(&op.e), op.e.clone(), op.k.clone()));
                }
            }
        }
    }
    tx.execute("UPDATE sync_ctl SET hlc = max(hlc, ?1) WHERE id = 1", [max_h])?;
    for (_, e, k) in &dirty {
        if materialize(&tx, e, k)? {
            out.entities.insert(e.clone());
            if e == "settings" {
                if let Some(Value::String(key)) = serde_json::from_str::<Vec<Value>>(k).ok().and_then(|v| v.into_iter().next()) {
                    out.settings.push(key);
                }
            }
        } else {
            tx.execute("INSERT OR IGNORE INTO sync_pending(entity, key) VALUES(?1, ?2)", params![e, k])?;
        }
    }
    // Rows that couldn't be built before (their playlist hadn't arrived yet) may be buildable now.
    let pending: Vec<(String, String)> = {
        let mut st = tx.prepare("SELECT entity, key FROM sync_pending")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.flatten().collect()
    };
    let mut pending = pending;
    pending.sort_by_key(|(e, _)| entity_rank(e));
    for (e, k) in pending {
        if materialize(&tx, &e, &k)? {
            tx.execute("DELETE FROM sync_pending WHERE entity = ?1 AND key = ?2", params![e, k])?;
            out.entities.insert(e);
        }
    }
    tx.execute("UPDATE sync_ctl SET capture = ?1 WHERE id = 1", [was as i64])?;
    tx.commit()?;
    Ok(out)
}

/// A wire value of a column back to its local value. `None`: it points at something this device
/// doesn't have (yet).
fn in_value(conn: &Connection, col: &Col, v: &Value) -> rusqlite::Result<Option<Sql>> {
    Ok(match col.kind {
        Kind::Plain | Kind::Uid => Some(to_sql(v)),
        Kind::PlaylistRef => match v.as_str() {
            None => Some(Sql::Null),
            Some(uid) => conn
                .query_row(LOCAL_ID_OF_UID, [uid], |r| r.get::<_, i64>(0))
                .optional()?
                .map(Sql::Integer),
        },
        Kind::ItemRef => match v.as_str() {
            Some(s) if s.starts_with(&global_prefix()) => {
                let uid = &s[global_prefix().len()..];
                conn.query_row(LOCAL_ID_OF_UID, [uid], |r| r.get::<_, i64>(0))
                    .optional()?
                    .map(|id| Sql::Text(format!("{LOCAL_PLAYLIST_PREFIX}{id}")))
            }
            _ => Some(to_sql(v)),
        },
        Kind::ListenRef => {
            let arr = match v {
                Value::Array(a) => Some(a.clone()),
                Value::String(s) => serde_json::from_str::<Vec<Value>>(s).ok(),
                _ => None,
            };
            match arr.as_deref() {
                Some([vid, at]) => conn
                    .query_row(
                        "SELECT id FROM listens WHERE video_id = ?1 AND played_at = ?2 ORDER BY id LIMIT 1",
                        params![vid.as_str().unwrap_or_default(), at.as_i64().unwrap_or_default()],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?
                    .map(Sql::Integer),
                _ => None,
            }
        }
    })
}

/// Rebuild one row from its merged state. False when it can't be built yet.
fn materialize(conn: &Connection, entity: &str, key: &str) -> rusqlite::Result<bool> {
    let state: BTreeMap<String, Value> = {
        let mut st = conn.prepare_cached("SELECT field, value FROM sync_field WHERE entity = ?1 AND key = ?2")?;
        let rows = st.query_map(params![entity, key], |r| Ok((r.get::<_, String>(0)?, to_json(r.get_ref(1)?))))?;
        rows.flatten().collect()
    };
    let deleted = state.get("_del").and_then(Value::as_i64) == Some(1);
    let key_vals: Vec<Value> = serde_json::from_str(key).unwrap_or_default();
    if entity == BLOCKED {
        return materialize_blocked(conn, key_vals.first(), deleted, state.get("v"));
    }
    let Some(spec) = SPECS.iter().find(|s| s.table == entity) else { return Ok(true) };
    if key_vals.len() != spec.key.len() {
        return Ok(true); // malformed; nothing to build, ever
    }
    let mut keys = Vec::new();
    for (col, v) in spec.key.iter().zip(&key_vals) {
        match in_value(conn, col, v)? {
            Some(x) => keys.push(x),
            None => return Ok(deleted), // a tombstone for something we don't have is done
        }
    }
    let wh: Vec<String> = spec.key.iter().enumerate().map(|(i, k)| format!("{} = ?{}", k.name, i + 1)).collect();
    let wh = wh.join(" AND ");
    if deleted {
        let sql = format!("DELETE FROM {} WHERE {wh}", spec.table);
        conn.execute(&sql, rusqlite::params_from_iter(keys.iter()))?;
        return Ok(true);
    }
    if spec.episode.is_some() {
        if let Some(ep) = state.get("_ep").and_then(|v| v.as_str()).and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok()) {
            insert_episode(conn, &ep)?;
        }
    }
    let mut cols = Vec::new();
    let mut vals = Vec::new();
    for f in spec.fields {
        let Some(v) = state.get(f.name) else { continue };
        match in_value(conn, f, v)? {
            Some(x) => {
                cols.push(f.name);
                vals.push(x);
            }
            None => return Ok(false),
        }
    }
    let n = keys.len();
    let changed = if cols.is_empty() {
        let sql = format!("SELECT COUNT(*) FROM {} WHERE {wh}", spec.table);
        conn.query_row(&sql, rusqlite::params_from_iter(keys.iter()), |r| r.get::<_, i64>(0))? as usize
    } else {
        let set: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} = ?{}", n + i + 1)).collect();
        let sql = format!("UPDATE {} SET {} WHERE {wh}", spec.table, set.join(", "));
        conn.execute(&sql, rusqlite::params_from_iter(keys.iter().chain(vals.iter())))?
    };
    if changed == 0 {
        let names: Vec<&str> = spec.key.iter().map(|k| k.name).chain(cols.iter().copied()).collect();
        let marks: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect();
        let sql = format!("INSERT INTO {}({}) VALUES({})", spec.table, names.join(", "), marks.join(", "));
        if let Err(e) = conn.execute(&sql, rusqlite::params_from_iter(keys.iter().chain(vals.iter()))) {
            // Not all of the row has arrived yet (a NOT NULL column): wait for the rest.
            tracing::debug!(entity, error = %e, "sync: row not buildable yet");
            return Ok(false);
        }
    }
    Ok(true)
}

fn insert_episode(conn: &Connection, ep: &serde_json::Map<String, Value>) -> rusqlite::Result<()> {
    if !table_exists(conn, "podcast_episodes") {
        return Ok(());
    }
    let cols: Vec<&str> = EPISODE_COLS.iter().copied().filter(|c| ep.contains_key(*c)).collect();
    if !cols.contains(&"id") {
        return Ok(());
    }
    let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
    let vals: Vec<Sql> = cols.iter().map(|c| to_sql(&ep[*c])).collect();
    let sql = format!("INSERT OR IGNORE INTO podcast_episodes({}) VALUES({})", cols.join(", "), marks.join(", "));
    let _ = conn.execute(&sql, rusqlite::params_from_iter(vals.iter()));
    Ok(())
}

fn materialize_blocked(conn: &Connection, key: Option<&Value>, deleted: bool, v: Option<&Value>) -> rusqlite::Result<bool> {
    let Some(key) = key.and_then(Value::as_str) else { return Ok(true) };
    let cur: Option<String> =
        conn.query_row("SELECT value FROM settings WHERE key = ?1", [BLOCKED_SETTING], |r| r.get(0)).optional()?;
    let mut list: Vec<Value> = cur.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let key_of = |x: &Value| x.get("id").and_then(Value::as_str).or_else(|| x.get("name").and_then(Value::as_str)).map(str::to_owned);
    list.retain(|x| key_of(x).as_deref() != Some(key));
    if !deleted {
        let entry = match v {
            Some(Value::String(s)) => serde_json::from_str(s).ok(),
            Some(o @ Value::Object(_)) => Some(o.clone()),
            _ => None,
        };
        let Some(entry) = entry else { return Ok(false) };
        list.push(entry);
    }
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![BLOCKED_SETTING, Value::Array(list).to_string()],
    )?;
    Ok(true)
}

/// A playlist uid → its local id, also for a playlist deleted here (so a late tombstone for a pin
/// or a track row pointing at it still finds the row to remove).
const LOCAL_ID_OF_UID: &str = "SELECT id FROM local_playlists WHERE uid = ?1
     UNION ALL SELECT id FROM sync_gone_playlists WHERE uid = ?1 LIMIT 1";
