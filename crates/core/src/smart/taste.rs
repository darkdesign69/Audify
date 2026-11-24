//! The local taste profile (research 07 §4(e)): a score per track and per artist from what the
//! user did, with exponential time decay (half-life 30 days). It only re-ranks candidates; it
//! never generates them.
//!
//! | Signal | Weight |
//! |---|---|
//! | play past the listen threshold (`listens`) | +1 |
//! | ... heard to the end (>= 90 %) | +1 more |
//! | replay within a day of the previous play | +1 more |
//! | like (`liked_tracks`) | +3 |
//! | added to a local playlist | +2 |
//! | followed artist (`library_items`, kind `artist`) | +2 to the artist |
//! | skip under 30 s (`smart_events`) | -2, -3 under 10 s; halved when the user picked the song |
//!
//! Plays and skips fade with a 30-day half-life. Likes, playlist adds and follows are durable
//! choices: they fade with a 365-day half-life and never below a quarter, so a library imported
//! from Spotify (years-old likes) seeds taste on day 1 (smart2; docs/bench/recommendations.md).
//!
//! Plays in a private session never count: their `listens` rows are listed in
//! `smart_private_listens` and their events carry `private = 1`.
//!
//! Anything the user excluded ("Exclude from your taste profile" on a song or playlist,
//! [`set_excluded`]) stops counting: its tracks' plays, skips and playlist adds are left out. Likes
//! and follows stay, because they are a choice made outside any context.
//!
//! Events: `listens` only records plays past the listen threshold, so the queue reports every
//! play's end here ([`track_started`] / [`track_ended`], two one-line hooks in `state.rs`) and
//! the skips and completes land in `smart_events`.

use std::collections::HashMap;
use std::sync::Mutex;

use innertube::SongItem;
use serde::Serialize;

use crate::db::Db;
use crate::identity;

const HALF_LIFE_DAYS: f64 = 30.0;
const DURABLE_HALF_LIFE_DAYS: f64 = 365.0;
const DURABLE_FLOOR: f64 = 0.25;
/// Weight of the long-term clock for live plays.
const LONG_TERM_W: f64 = 0.25;
const SKIP_SECS: f64 = 30.0;
const COMPLETE_SHARE: f64 = 0.9;

struct Playing {
    item: SongItem,
}

static PLAYING: Mutex<Option<Playing>> = Mutex::new(None);

/// Hook: a track started (the queue's now-playing announcement).
pub fn track_started(item: &SongItem) {
    *PLAYING.lock().unwrap() = Some(Playing { item: item.clone() });
}

/// Hook: the play of the current track is over (`AppState::end_listen`), after `heard` seconds.
/// `listen_row` is its `listens` row when it got far enough to be counted as a play.
pub fn track_ended(db: &Db, heard: f64, listen_row: Option<i64>) {
    let Some(item) = PLAYING.lock().unwrap().as_ref().map(|p| p.item.clone()) else { return };
    if heard < 1.0 {
        return;
    }
    let private = super::settings::private_session(db);
    let duration = item.duration.as_deref().and_then(identity::secs).filter(|d| *d > 0.0);
    let kind = classify_end(heard, duration);
    let artist_key = super::shuffle::artist_key(&item);
    let artist = identity::primary_artist(&item.artists);
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_events(video_id, artist_key, artist, kind, secs, duration,
                                      from_rec, private, at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                item.video_id,
                artist_key,
                artist,
                kind,
                heard,
                duration,
                item.autoplay as i64,
                private as i64,
                super::now_secs()
            ],
        );
        if private {
            if let Some(row) = listen_row {
                let _ = c.execute(
                    "INSERT OR IGNORE INTO smart_private_listens(listen_id) VALUES(?1)",
                    [row],
                );
            }
        }
    });
}

pub fn classify_end(heard: f64, duration: Option<f64>) -> &'static str {
    match duration {
        Some(d) if heard >= d * COMPLETE_SHARE => "complete",
        _ if heard < SKIP_SECS => "skip",
        _ => "partial",
    }
}

/// One thing the user did, as the profile reads it.
#[derive(Debug, Clone)]
pub struct Signal {
    pub video_id: String,
    pub artist_key: String,
    pub artist_name: String,
    pub at: i64,
    pub kind: SignalKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalKind {
    /// A counted play: seconds heard, and the track's length when known.
    Play {
        secs: f64,
        duration: Option<f64>,
        /// Came from a Spotify history import (`imported_plays`): long-term taste only.
        imported: bool,
    },
    Skip {
        secs: f64,
        from_rec: bool,
    },
    Like,
    PlaylistAdd,
    /// A followed artist: counts for the artist only.
    Follow,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Taste {
    pub tracks: HashMap<String, f64>,
    pub artists: HashMap<String, f64>,
    /// artist key -> display name.
    pub artist_names: HashMap<String, String>,
    /// track -> its artist key.
    pub track_artist: HashMap<String, String>,
}

impl Taste {
    pub fn track(&self, id: &str) -> f64 {
        self.tracks.get(id).copied().unwrap_or(0.0)
    }

    pub fn artist(&self, key: &str) -> f64 {
        self.artists.get(key).copied().unwrap_or(0.0)
    }

    /// Track + artist taste for a candidate, squashed to about -1..1.
    pub fn affinity(&self, s: &SongItem) -> f64 {
        let t = self.track(&s.video_id);
        let a = self
            .artist(&super::shuffle::artist_key(s))
            .max(self.artist(&identity::primary_artist(&s.artists)));
        0.6 * (t / 3.0).tanh() + 0.4 * (a / 6.0).tanh()
    }

    /// Artists by score, best first.
    pub fn top_artists(&self, n: usize) -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> =
            self.artists.iter().filter(|(_, s)| **s > 0.0).map(|(k, s)| (k.clone(), *s)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }
}

fn decay(now: i64, at: i64) -> f64 {
    let days = (now - at).max(0) as f64 / 86_400.0;
    0.5f64.powf(days / HALF_LIFE_DAYS)
}

fn durable_decay(now: i64, at: i64) -> f64 {
    let days = (now - at).max(0) as f64 / 86_400.0;
    0.5f64.powf(days / DURABLE_HALF_LIFE_DAYS).max(DURABLE_FLOOR)
}

/// The profile from a list of signals (pure, for tests and the offline harness).
pub fn compute(signals: &[Signal], now: i64) -> Taste {
    let mut taste = Taste::default();
    let mut last_play: HashMap<&str, i64> = HashMap::new();
    let mut sorted: Vec<&Signal> = signals.iter().collect();
    sorted.sort_by_key(|s| s.at);
    for s in sorted {
        let w = match s.kind {
            SignalKind::Play { secs, duration, .. } => {
                let mut w = 1.0;
                if duration.is_some_and(|d| d > 0.0 && secs >= d * COMPLETE_SHARE) {
                    w += 1.0;
                }
                if let Some(prev) = last_play.insert(&s.video_id, s.at) {
                    if s.at - prev < 86_400 {
                        w += 1.0;
                    }
                }
                w
            }
            SignalKind::Skip { secs, from_rec } => {
                let w = if secs < 10.0 { -3.0 } else { -2.0 };
                if from_rec {
                    w
                } else {
                    w / 2.0
                }
            }
            SignalKind::Like => 3.0,
            SignalKind::PlaylistAdd => 2.0,
            SignalKind::Follow => 2.0,
        } * match s.kind {
            SignalKind::Like | SignalKind::PlaylistAdd | SignalKind::Follow => {
                durable_decay(now, s.at)
            }
            // Two profiles in one, like Spotify's short- and long-term taste: a live play counts
            // fully on the 30-day clock and a quarter on the long-term (365-day, floored) one;
            // an imported Spotify play is long-term history and counts on that clock alone, so
            // years of it make Home and Made for you personal on the first launch after import.
            // LONG_TERM_W = 0.25 is neutral on the benchmark (NDCG@10 0.2247 -> 0.2239).
            SignalKind::Play { imported: true, .. } => durable_decay(now, s.at),
            SignalKind::Play { .. } => decay(now, s.at) + LONG_TERM_W * durable_decay(now, s.at),
            _ => decay(now, s.at),
        };
        if s.kind != SignalKind::Follow {
            *taste.tracks.entry(s.video_id.clone()).or_default() += w;
        }
        if !s.artist_key.is_empty() {
            *taste.artists.entry(s.artist_key.clone()).or_default() += w;
            taste.artist_names.entry(s.artist_key.clone()).or_insert_with(|| s.artist_name.clone());
            if s.kind != SignalKind::Follow {
                taste.track_artist.insert(s.video_id.clone(), s.artist_key.clone());
            }
        }
    }
    taste
}

fn signal_from_song(json: &str, at: i64, kind: SignalKind) -> Option<Signal> {
    let s: SongItem = serde_json::from_str(json).ok()?;
    Some(Signal {
        artist_key: super::shuffle::artist_key(&s),
        artist_name: display_artist(&s),
        video_id: s.video_id,
        at,
        kind,
    })
}

/// The first credited artist as written.
pub fn display_artist(s: &SongItem) -> String {
    s.artist_runs
        .iter()
        .map(|r| r.text.trim())
        .find(|t| !t.is_empty() && !matches!(*t, "," | "&" | "\u{2022}"))
        .map(str::to_owned)
        .unwrap_or_else(|| s.artists.split([',', '&']).next().unwrap_or("").trim().to_owned())
}

/// Every signal on record, private plays left out.
pub fn load_signals(db: &Db) -> Vec<Signal> {
    let mut out = Vec::new();
    db.with_conn(|c| {
        // Counted plays; the ones a Spotify import wrote are marked in `imported_plays` (import2).
        let with_import = c
            .prepare(
                "SELECT s.song_json, l.played_at, l.secs,
                        l.id IN (SELECT listen_id FROM imported_plays WHERE listen_id IS NOT NULL)
                 FROM listens l
                 JOIN listen_songs s ON s.video_id = l.video_id
                 WHERE l.id NOT IN (SELECT listen_id FROM smart_private_listens)",
            )
            .or_else(|_| {
                c.prepare(
                    "SELECT s.song_json, l.played_at, l.secs, 0 FROM listens l
                     JOIN listen_songs s ON s.video_id = l.video_id
                     WHERE l.id NOT IN (SELECT listen_id FROM smart_private_listens)",
                )
            });
        if let Ok(mut st) = with_import {
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)? != 0,
                ))
            });
            if let Ok(rows) = rows {
                // stream: perf — a history repeats its songs many times over, and each row used to
                // be parsed twice (a Value for the duration, then a SongItem). Parse each song once:
                // 60k listens over 4k songs went from 967 ms to 186 ms (debug build), all of it
                // under the DB lock that every other command waits on.
                let mut parsed: HashMap<String, Option<(String, String, String, Option<f64>)>> =
                    HashMap::new();
                for (json, at, secs, imported) in rows.flatten() {
                    let song = parsed.entry(json).or_insert_with_key(|json| {
                        let s: SongItem = serde_json::from_str(json).ok()?;
                        let dur = s.duration.as_deref().and_then(identity::secs);
                        Some((super::shuffle::artist_key(&s), display_artist(&s), s.video_id, dur))
                    });
                    if let Some((artist_key, artist_name, video_id, dur)) = song {
                        out.push(Signal {
                            artist_key: artist_key.clone(),
                            artist_name: artist_name.clone(),
                            video_id: video_id.clone(),
                            at,
                            kind: SignalKind::Play { secs: secs as f64, duration: *dur, imported },
                        });
                    }
                }
            }
        }
        // Skips.
        if let Ok(mut st) = c.prepare(
            "SELECT video_id, COALESCE(artist_key, ''), COALESCE(artist, ''), secs, from_rec, at
             FROM smart_events WHERE kind = 'skip' AND private = 0",
        ) {
            let rows = st.query_map([], |r| {
                Ok(Signal {
                    video_id: r.get(0)?,
                    artist_key: r.get(1)?,
                    artist_name: r.get(2)?,
                    at: r.get(5)?,
                    kind: SignalKind::Skip { secs: r.get(3)?, from_rec: r.get::<_, i64>(4)? != 0 },
                })
            });
            if let Ok(rows) = rows {
                out.extend(rows.flatten());
            }
        }
        // Playlist adds.
        if let Ok(mut st) = c.prepare("SELECT song_json, added_at FROM local_playlist_tracks") {
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
            if let Ok(rows) = rows {
                for (json, at) in rows.flatten() {
                    out.extend(signal_from_song(&json, at, SignalKind::PlaylistAdd));
                }
            }
        }
    });
    // Followed artists (stream library's `library_items`; Spotify import writes them too).
    db.with_conn(|c| {
        if let Ok(mut st) =
            c.prepare("SELECT id, title, added_at FROM library_items WHERE kind = 'artist'")
        {
            let rows = st.query_map([], |r| {
                Ok(Signal {
                    video_id: String::new(),
                    artist_key: r.get(0)?,
                    artist_name: r.get(1)?,
                    at: r.get(2)?,
                    kind: SignalKind::Follow,
                })
            });
            if let Ok(rows) = rows {
                out.extend(rows.flatten());
            }
        }
    });
    // Likes.
    use super::library::Library;
    for (id, at, json) in super::library::DbLibrary(db).liked() {
        let at = if at > 0 { at } else { super::now_secs() };
        match json.as_deref().and_then(|j| signal_from_song(j, at, SignalKind::Like)) {
            Some(s) => out.push(s),
            None => out.push(Signal {
                video_id: id,
                artist_key: String::new(),
                artist_name: String::new(),
                at,
                kind: SignalKind::Like,
            }),
        }
    }
    drop_excluded(&mut out, &excluded_tracks(db)); // ── lane magic-d1 ──
    out
}

// ── lane magic-d1 ── "Exclude from your taste profile" (MAGIC-GAPS #16) ──────────────────────
//
// `listens` doesn't record which context a play came from, so a context is excluded by its tracks:
// excluding a playlist snapshots its songs, and plays of those songs stop shaping taste wherever
// they were heard. Coarser than Spotify (a song in two playlists is out from both), and the right
// side to err on for "this playlist is my kid's, don't learn from it".

pub(crate) fn ensure_excluded_tables(c: &rusqlite::Connection) {
    ensure_excluded(c)
}

fn ensure_excluded(c: &rusqlite::Connection) {
    let _ = c.execute_batch(
        "CREATE TABLE IF NOT EXISTS taste_excluded (
             id    TEXT PRIMARY KEY,
             kind  TEXT NOT NULL,
             title TEXT,
             at    INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS taste_excluded_tracks (
             context_id TEXT NOT NULL,
             video_id   TEXT NOT NULL,
             PRIMARY KEY (context_id, video_id)
         );",
    );
}

/// Exclude (`on`) or re-include a song or playlist. `video_ids` are the tracks it covers: the song
/// itself, or the playlist's songs as the UI has them. Excluding again refreshes the snapshot.
pub fn set_excluded(db: &Db, id: &str, kind: &str, title: Option<&str>, video_ids: &[String], on: bool) {
    db.with_conn(|c| {
        ensure_excluded(c);
        let _ = c.execute("DELETE FROM taste_excluded_tracks WHERE context_id = ?1", [id]);
        if !on {
            let _ = c.execute("DELETE FROM taste_excluded WHERE id = ?1", [id]);
            return;
        }
        let _ = c.execute(
            "INSERT OR REPLACE INTO taste_excluded(id, kind, title, at) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![id, kind, title, super::now_secs()],
        );
        let mut ids: Vec<&str> = video_ids.iter().map(String::as_str).collect();
        if kind == "song" && !ids.contains(&id) {
            ids.push(id);
        }
        for v in ids {
            let _ = c.execute(
                "INSERT OR IGNORE INTO taste_excluded_tracks(context_id, video_id) VALUES(?1, ?2)",
                [id, v],
            );
        }
    });
}

/// The ids (songs and playlists) currently excluded, for the menus' label.
pub fn excluded_ids(db: &Db) -> Vec<String> {
    db.with_conn(|c| {
        ensure_excluded(c);
        let Ok(mut st) = c.prepare("SELECT id FROM taste_excluded ORDER BY at DESC") else {
            return Vec::new();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    })
}

/// Every track an exclusion covers.
fn excluded_tracks(db: &Db) -> std::collections::HashSet<String> {
    db.with_conn(|c| {
        ensure_excluded(c);
        let Ok(mut st) = c.prepare("SELECT DISTINCT video_id FROM taste_excluded_tracks") else {
            return Default::default();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    })
}

/// Drop what an exclusion covers: plays, skips and playlist adds of excluded tracks.
fn drop_excluded(signals: &mut Vec<Signal>, excluded: &std::collections::HashSet<String>) {
    if excluded.is_empty() {
        return;
    }
    signals.retain(|s| {
        !(matches!(s.kind, SignalKind::Play { .. } | SignalKind::Skip { .. } | SignalKind::PlaylistAdd)
            && excluded.contains(&s.video_id))
    });
}

/// The profile now. Cheap enough to compute per request (one pass over the history).
pub fn profile(db: &Db) -> Taste {
    compute(&load_signals(db), super::now_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 10_000_000;
    const DAY: i64 = 86_400;

    fn sig(id: &str, artist: &str, days_ago: i64, kind: SignalKind) -> Signal {
        Signal {
            video_id: id.into(),
            artist_key: artist.into(),
            artist_name: artist.into(),
            at: NOW - days_ago * DAY,
            kind,
        }
    }

    fn play(secs: f64) -> SignalKind {
        SignalKind::Play { secs, duration: Some(200.0), imported: false }
    }

    #[test]
    fn imported_history_is_long_term_taste() {
        let imported = SignalKind::Play { secs: 200.0, duration: Some(200.0), imported: true };
        let t = compute(
            &[
                sig("spotify_fav", "A", 2 * 365, imported),
                sig("spotify_fav", "A", 2 * 365 - 3, imported),
                sig("old_live", "B", 2 * 365, play(200.0)),
                sig("new_live", "C", 0, play(200.0)),
            ],
            NOW,
        );
        // Two years old: floored at a quarter on the long-term clock, not 2^-24.
        assert!((t.track("spotify_fav") - 2.0 * 2.0 * 0.25).abs() < 1e-2);
        // An old live play keeps only its long-term quarter of a quarter.
        assert!((t.track("old_live") - 2.0 * 0.25 * 0.25).abs() < 1e-3);
        // A fresh live play: full short-term + a quarter long-term.
        assert!((t.track("new_live") - 2.0 * 1.25).abs() < 1e-6);
    }

    #[test]
    fn likes_completes_and_replays_beat_skips() {
        let t = compute(
            &[
                sig("loved", "A", 1, SignalKind::Like),
                sig("loved", "A", 1, play(200.0)),
                sig("loved", "A", 1, play(200.0)),
                sig("meh", "B", 1, play(60.0)),
                sig("skipped", "C", 1, SignalKind::Skip { secs: 5.0, from_rec: true }),
            ],
            NOW,
        );
        assert!(t.track("loved") > t.track("meh"));
        assert!(t.track("meh") > 0.0);
        assert!(t.track("skipped") < 0.0);
        assert_eq!(t.top_artists(5)[0].0, "A");
        assert!(t.top_artists(5).iter().all(|(a, _)| a != "C"));
    }

    #[test]
    fn old_signals_fade() {
        let t = compute(
            &[sig("old", "A", 120, SignalKind::Like), sig("new", "B", 0, SignalKind::Like)],
            NOW,
        );
        // A like fades on the durable clock: 120 days of a 365-day half-life.
        assert!((t.track("old") - 3.0 * 0.5f64.powf(120.0 / 365.0)).abs() < 1e-6);
        assert!((t.track("new") - 3.0).abs() < 1e-6);
    }

    #[test]
    fn likes_and_follows_are_durable() {
        let t = compute(
            &[
                sig("old_like", "A", 3 * 365, SignalKind::Like),
                sig("", "UCf", 400, SignalKind::Follow),
                sig("old_play", "B", 365, play(200.0)),
            ],
            NOW,
        );
        // Floored at a quarter: an imported library from years ago still counts.
        assert!((t.track("old_like") - 0.75).abs() < 1e-6);
        assert!(t.artist("UCf") > 0.5);
        assert!(t.tracks.get("").is_none(), "a follow is not a track");
        // A year-old live play keeps its long-term share: 2 x 0.25 x 0.5.
        assert!((t.track("old_play") - 0.25).abs() < 1e-3);
    }

    #[test]
    fn a_skip_of_your_own_pick_costs_less() {
        let t = compute(
            &[
                sig("rec", "A", 0, SignalKind::Skip { secs: 20.0, from_rec: true }),
                sig("mine", "B", 0, SignalKind::Skip { secs: 20.0, from_rec: false }),
            ],
            NOW,
        );
        assert_eq!(t.track("rec"), -2.0);
        assert_eq!(t.track("mine"), -1.0);
    }

    #[test]
    fn excluded_tracks_stop_shaping_taste_but_likes_stay() {
        let db = super::super::test_db();
        let mk = |id: &str, kind| sig(id, "a", 1, kind);
        let mut signals = vec![
            mk("kid1", play(200.0)),
            mk("kid1", SignalKind::Skip { secs: 3.0, from_rec: false }),
            mk("kid2", SignalKind::PlaylistAdd),
            mk("kid2", SignalKind::Like),
            mk("mine", play(200.0)),
        ];
        set_excluded(&db, "LOCALPLAYLIST:1", "playlist", Some("Kids"), &["kid1".into(), "kid2".into()], true);
        assert_eq!(excluded_ids(&db), ["LOCALPLAYLIST:1"]);
        drop_excluded(&mut signals, &excluded_tracks(&db));
        let left: Vec<(&str, SignalKind)> = signals.iter().map(|s| (s.video_id.as_str(), s.kind)).collect();
        assert_eq!(left.len(), 2);
        assert!(left.iter().any(|(id, k)| *id == "kid2" && *k == SignalKind::Like));
        assert!(left.iter().any(|(id, _)| *id == "mine"));

        // Re-including brings everything back.
        set_excluded(&db, "LOCALPLAYLIST:1", "playlist", None, &[], false);
        assert!(excluded_ids(&db).is_empty());
        assert!(excluded_tracks(&db).is_empty());
    }

    #[test]
    fn excluding_a_song_covers_the_song_itself() {
        let db = super::super::test_db();
        set_excluded(&db, "vid", "song", Some("Song"), &[], true);
        assert!(excluded_tracks(&db).contains("vid"));
    }

    #[test]
    fn end_kinds() {
        assert_eq!(classify_end(5.0, Some(200.0)), "skip");
        assert_eq!(classify_end(100.0, Some(200.0)), "partial");
        assert_eq!(classify_end(190.0, Some(200.0)), "complete");
        assert_eq!(classify_end(40.0, None), "partial");
    }

    #[test]
    fn private_session_plays_and_skips_never_count() {
        let db = super::super::test_db();
        let song = crate::identity::tests::song("p", "Song", "Artist", None, "3:00");
        let json = serde_json::to_string(&song).unwrap();
        let row = db.record_listen("p", &json, NOW, 100).unwrap();
        crate::library::set_private_session(&db.conn(), true, crate::db::now_secs()).unwrap();
        track_started(&song);
        track_ended(&db, 100.0, Some(row));
        let skipped = crate::identity::tests::song("s", "Other", "Artist", None, "3:00");
        track_started(&skipped);
        track_ended(&db, 3.0, None);
        assert!(load_signals(&db).is_empty());
        // Out of private mode the same plays count.
        crate::library::set_private_session(&db.conn(), false, crate::db::now_secs()).unwrap();
        let row = db.record_listen("p", &json, NOW, 100).unwrap();
        track_started(&song);
        track_ended(&db, 100.0, Some(row));
        track_started(&skipped);
        track_ended(&db, 3.0, None);
        let sigs = load_signals(&db);
        assert_eq!(sigs.len(), 2);
    }

    /// `cargo test -p limusic-app --lib bench_profile_big_history -- --ignored --nocapture`
    /// (stream: perf): 60k listens over 4k songs, about a heavy Spotify import.
    #[test]
    #[ignore]
    fn bench_profile_big_history() {
        let db = super::super::test_db();
        {
            let c = db.conn();
            c.execute_batch("BEGIN").unwrap();
            for i in 0..4000 {
                let song = crate::identity::tests::song(
                    &format!("v{i}"),
                    &format!("Song {i}"),
                    &format!("Artist {}", i % 300),
                    None,
                    "3:00",
                );
                let json = serde_json::to_string(&song).unwrap();
                c.execute("INSERT INTO listen_songs(video_id, song_json) VALUES(?1, ?2)", [&format!("v{i}"), &json])
                    .unwrap();
            }
            for n in 0..60_000i64 {
                c.execute(
                    "INSERT INTO listens(video_id, played_at, secs) VALUES(?1, ?2, 120)",
                    rusqlite::params![format!("v{}", (n * 7919) % 4000), NOW - n * 600],
                )
                .unwrap();
            }
            c.execute_batch("COMMIT").unwrap();
        }
        let t = std::time::Instant::now();
        let p = profile(&db);
        eprintln!("profile over 60k listens: {:?} ({} artists)", t.elapsed(), p.artists.len());
    }
}
