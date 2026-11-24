//! Spotify listening history (stream import2): read the streaming-history files of Spotify's data
//! export and turn them into this app's own history, so Home, Made for you and Recently played
//! are personal the moment an import finishes.
//!
//! Two shapes exist, both documented on Spotify's "Understanding my data" page:
//!
//! - **Account data** (the zip from spotify.com/account/privacy, a few days):
//!   `StreamingHistory_music_<n>.json` (older exports: `StreamingHistory<n>.json`), the last year,
//!   `[{endTime: "2024-05-01 18:23", artistName, trackName, msPlayed}]`, times in UTC.
//!   `StreamingHistory_podcast_<n>.json` is podcasts and is left out.
//! - **Extended streaming history** (asked for separately, up to 30 days):
//!   `Streaming_History_Audio_<years>_<n>.json` (older: `endsong_<n>.json`), the whole account,
//!   `[{ts: "2024-05-01T18:23:45Z", ms_played, master_metadata_track_name,
//!   master_metadata_album_artist_name, master_metadata_album_album_name, spotify_track_uri,
//!   reason_end, skipped, incognito_mode, episode_name, ...}]`.
//!
//! The plays are written into the tables the live app writes (`listens` + `listen_songs` for a
//! play past 30 s, the threshold Spotify itself counts a stream at, and `smart_events` for every
//! play), with their original times, so the smart layer reads them like any other play without
//! knowing about imports. Each written play is also filed in `imported_plays`, which marks it as
//! imported (stats or a "forget imported history" can tell them apart) and makes a second import
//! of the same file a no-op.

use std::collections::HashMap;

use innertube::SongItem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Db;
use crate::spotify::SourceTrack;

/// Spotify counts a stream at 30 s; so does this import.
pub const COUNTED_MS: u64 = 30_000;
/// How many history-only songs (in no imported list) are worth a search each. The rest of the
/// history still lands when its song is matched through a playlist or Liked Songs.
pub const TOP_SONGS: usize = 250;
/// At most this many plays are written, the most recent ones. Years of extended history can be
/// 100k+ plays; the taste profile halves a play's weight every 30 days, so the old tail adds
/// nothing to recommendations and would only slow every Home build.
pub const MAX_PLAYS: usize = 25_000;

/// One play from the export.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub track: SourceTrack,
    /// When the play ended (unix seconds, UTC).
    pub end: i64,
    pub ms: u64,
    /// From the extended history: the user skipped it.
    pub skipped: bool,
}

impl Play {
    pub fn start(&self) -> i64 {
        self.end - (self.ms / 1000) as i64
    }
}

/// The export's file names that hold music history.
pub fn is_history_file(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if !base.ends_with(".json") || base.contains("podcast") || base.contains("video") {
        return false;
    }
    base.starts_with("streaminghistory")
        || base.starts_with("streaming_history_audio")
        || base.starts_with("endsong")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Basic {
    end_time: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    ms_played: u64,
}

#[derive(Deserialize)]
struct Extended {
    ts: String,
    #[serde(default)]
    ms_played: u64,
    #[serde(default)]
    master_metadata_track_name: Option<String>,
    #[serde(default)]
    master_metadata_album_artist_name: Option<String>,
    #[serde(default)]
    master_metadata_album_album_name: Option<String>,
    #[serde(default)]
    spotify_track_uri: Option<String>,
    #[serde(default)]
    skipped: Option<bool>,
    #[serde(default)]
    incognito_mode: Option<bool>,
}

fn clean(s: &str) -> Option<String> {
    let s = s.trim();
    // What the account-data file writes for a track Spotify no longer knows.
    (!s.is_empty() && s != "Unknown Track" && s != "Unknown Artist").then(|| s.to_owned())
}

/// "2024-05-01 18:23" (UTC) → unix seconds.
fn basic_time(s: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

/// "2024-05-01T18:23:45Z" → unix seconds.
fn extended_time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim()).ok().map(|t| t.timestamp())
}

/// Read one history file. `Ok(false)`: the JSON is not streaming history (some other file).
/// Podcast and audiobook rows, plays in a private session and rows with no song are dropped.
pub fn read_history_json(bytes: &[u8], out: &mut Vec<Play>) -> Result<bool, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| "unreadable")?;
    let Some(rows) = v.as_array() else { return Ok(false) };
    let Some(first) = rows.first() else { return Ok(true) };
    if first.get("ts").is_some() {
        for row in rows {
            let Ok(e) = Extended::deserialize(row) else { continue };
            if e.incognito_mode == Some(true) {
                continue;
            }
            let (Some(title), Some(end)) =
                (e.master_metadata_track_name.as_deref().and_then(clean), extended_time(&e.ts))
            else {
                continue;
            };
            out.push(Play {
                track: SourceTrack {
                    id: e
                        .spotify_track_uri
                        .as_deref()
                        .and_then(|u| u.strip_prefix("spotify:track:"))
                        .map(str::to_owned),
                    title,
                    artists: e
                        .master_metadata_album_artist_name
                        .as_deref()
                        .and_then(clean)
                        .into_iter()
                        .collect(),
                    album: e.master_metadata_album_album_name.as_deref().and_then(clean),
                    ..Default::default()
                },
                end,
                ms: e.ms_played,
                skipped: e.skipped == Some(true),
            });
        }
        Ok(true)
    } else if first.get("endTime").is_some() {
        for row in rows {
            let Ok(b) = Basic::deserialize(row) else { continue };
            let (Some(title), Some(end)) = (clean(&b.track_name), basic_time(&b.end_time)) else {
                continue;
            };
            out.push(Play {
                track: SourceTrack {
                    title,
                    artists: clean(&b.artist_name).into_iter().collect(),
                    ..Default::default()
                },
                end,
                ms: b.ms_played,
                skipped: false,
            });
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Tidy what was read: when the zip held both shapes, the extended history (exact times, track
/// ids) wins over the account-data copy of the same plays; newest first; capped at [`MAX_PLAYS`].
pub fn settle(plays: &mut Vec<Play>) {
    if plays.iter().any(|p| p.track.id.is_some()) {
        plays.retain(|p| p.track.id.is_some());
    }
    plays.sort_by(|a, b| b.end.cmp(&a.end));
    plays.dedup_by(|a, b| a.end == b.end && a.track == b.track);
    plays.truncate(MAX_PLAYS);
}

/// The normalized "title|first artist" a track is known by when it has no Spotify id.
fn name_key(t: &SourceTrack) -> String {
    format!(
        "{}|{}",
        crate::import::norm(&t.title),
        t.artists.first().map(|a| crate::import::norm(a)).unwrap_or_default()
    )
}

/// Give every play the track as the imported lists know it. The account-data history names songs
/// only by title and artist; the same song in Liked Songs carries Spotify's id and album, and so
/// is one search, not two, and one cache entry.
pub fn adopt_list_tracks<'a>(plays: &mut [Play], lists: impl Iterator<Item = &'a SourceTrack>) {
    let mut by_id: HashMap<&str, &SourceTrack> = HashMap::new();
    let mut by_name: HashMap<String, &SourceTrack> = HashMap::new();
    for t in lists {
        if let Some(id) = t.id.as_deref() {
            by_id.entry(id).or_insert(t);
        }
        by_name.entry(name_key(t)).or_insert(t);
    }
    for p in plays.iter_mut() {
        let known = match p.track.id.as_deref() {
            Some(id) => by_id.get(id).copied(),
            None => by_name.get(&name_key(&p.track)).copied(),
        };
        if let Some(t) = known {
            p.track = t.clone();
        }
    }
}

/// The songs worth a search: most counted plays first, then the most recent. Each once.
pub fn top_tracks(plays: &[Play], n: usize) -> Vec<SourceTrack> {
    let mut score: HashMap<String, (usize, i64, &SourceTrack)> = HashMap::new();
    for p in plays {
        let e = score.entry(crate::import::key(&p.track)).or_insert((0, p.end, &p.track));
        if p.ms >= COUNTED_MS {
            e.0 += 1;
        }
        e.1 = e.1.max(p.end);
    }
    let mut ranked: Vec<_> = score.into_values().filter(|(n, _, _)| *n > 0).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    ranked.into_iter().take(n).map(|(_, _, t)| t.clone()).collect()
}

/// What the preview says about the history.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub plays: usize,
    pub songs: usize,
    /// Unix seconds of the oldest and newest play.
    pub from: i64,
    pub to: i64,
}

pub fn summary(plays: &[Play]) -> Option<Summary> {
    let counted: Vec<&Play> = plays.iter().filter(|p| p.ms >= COUNTED_MS).collect();
    if counted.is_empty() {
        return None;
    }
    let songs: std::collections::HashSet<String> =
        counted.iter().map(|p| crate::import::key(&p.track)).collect();
    Some(Summary {
        plays: counted.len(),
        songs: songs.len(),
        from: counted.iter().map(|p| p.end).min().unwrap_or(0),
        to: counted.iter().map(|p| p.end).max().unwrap_or(0),
    })
}

pub fn migrate(db: &Db) {
    db.with_conn(|c| {
        let r = c.execute_batch(
            "CREATE TABLE IF NOT EXISTS imported_plays (
                video_id    TEXT NOT NULL,
                at          INTEGER NOT NULL,
                ms          INTEGER NOT NULL,
                listen_id   INTEGER,
                event_id    INTEGER,
                imported_at INTEGER NOT NULL,
                PRIMARY KEY(video_id, at)
            );",
        );
        if let Err(e) = r {
            tracing::warn!(error = %e, "history: creating imported_plays failed");
        }
    });
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    /// Plays past 30 s, now in `listens` (Recently played, stats, taste).
    pub listens: usize,
    /// Shorter plays, written as skips (taste only).
    pub skips: usize,
    /// Already there from an earlier import of the same file.
    pub already: usize,
    /// Plays of songs that found no match.
    pub unmatched: usize,
}

/// Write the plays whose song `song_of` knows. One transaction; idempotent per (video, time).
pub fn write_plays(
    db: &Db,
    plays: &[Play],
    song_of: impl Fn(&SourceTrack) -> Option<SongItem>,
) -> Written {
    // `smart::migrate` creates `smart_events`; a database that never ran it (a unit test) would
    // lose every play to a missing table.
    crate::smart::migrate(db);
    migrate(db);
    let now = crate::db::now_secs();
    let mut w = Written::default();
    db.with_conn(|c| {
        if c.execute_batch("BEGIN").is_err() {
            return;
        }
        for p in plays {
            let Some(song) = song_of(&p.track) else {
                w.unmatched += 1;
                continue;
            };
            let at = p.start();
            let fresh = c
                .execute(
                    "INSERT OR IGNORE INTO imported_plays(video_id, at, ms, imported_at)
                     VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![song.video_id, at, p.ms as i64, now],
                )
                .unwrap_or(0);
            if fresh == 0 {
                w.already += 1;
                continue;
            }
            let secs = p.ms as f64 / 1000.0;
            let duration = song.duration.as_deref().and_then(crate::identity::secs);
            let listen_id = if p.ms >= COUNTED_MS {
                let json = serde_json::to_string(&song).unwrap_or_default();
                // Never over a fresher copy the live app wrote.
                let _ = c.execute(
                    "INSERT OR IGNORE INTO listen_songs(video_id, song_json) VALUES(?1, ?2)",
                    [&song.video_id, &json],
                );
                c.execute(
                    "INSERT INTO listens(video_id, played_at, secs) VALUES(?1, ?2, ?3)",
                    rusqlite::params![song.video_id, at, secs as i64],
                )
                .ok()
                .map(|_| c.last_insert_rowid())
            } else {
                None
            };
            let kind = if p.skipped && p.ms < COUNTED_MS {
                "skip"
            } else {
                crate::smart::taste::classify_end(secs, duration)
            };
            let event_id = c
                .execute(
                    "INSERT INTO smart_events(video_id, artist_key, artist, kind, secs, duration,
                                              from_rec, private, at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7)",
                    rusqlite::params![
                        song.video_id,
                        crate::smart::shuffle::artist_key(&song),
                        crate::identity::primary_artist(&song.artists),
                        kind,
                        secs,
                        duration,
                        p.end
                    ],
                )
                .ok()
                .map(|_| c.last_insert_rowid());
            let _ = c.execute(
                "UPDATE imported_plays SET listen_id = ?3, event_id = ?4
                 WHERE video_id = ?1 AND at = ?2",
                rusqlite::params![song.video_id, at, listen_id, event_id],
            );
            if listen_id.is_some() {
                w.listens += 1;
            } else {
                w.skips += 1;
            }
        }
        if c.execute_batch("COMMIT").is_err() {
            let _ = c.execute_batch("ROLLBACK");
            w = Written::default();
        }
    });
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = include_str!("../tests/fixtures/spotify/StreamingHistory_music_0.json");
    const EXTENDED: &str =
        include_str!("../tests/fixtures/spotify/Streaming_History_Audio_2023-2024_0.json");

    fn song(id: &str, title: &str, artist: &str) -> SongItem {
        serde_json::from_value(serde_json::json!({
            "video_id": id, "title": title, "artists": artist, "duration": "3:20"
        }))
        .unwrap()
    }

    #[test]
    fn file_names() {
        assert!(is_history_file("Spotify Account Data/StreamingHistory_music_0.json"));
        assert!(is_history_file("MyData/StreamingHistory0.json"));
        assert!(is_history_file(
            "Spotify Extended Streaming History/Streaming_History_Audio_2023-2024_0.json"
        ));
        assert!(is_history_file("MyData/endsong_3.json"));
        assert!(!is_history_file("StreamingHistory_podcast_0.json"));
        assert!(!is_history_file("Streaming_History_Video_2023-2024.json"));
        assert!(!is_history_file("YourLibrary.json"));
    }

    #[test]
    fn reads_account_data_history() {
        let mut plays = Vec::new();
        assert!(read_history_json(BASIC.as_bytes(), &mut plays).unwrap());
        assert_eq!(plays.len(), 5, "the Unknown Track row is dropped");
        assert_eq!(plays[0].track.title, "Blinding Lights");
        assert_eq!(plays[0].track.artists, vec!["The Weeknd"]);
        assert_eq!(plays[0].end, 1_714_587_780); // 2024-05-01 18:23 UTC
        assert_eq!(plays[0].start(), 1_714_587_780 - 200);
    }

    #[test]
    fn reads_extended_history() {
        let mut plays = Vec::new();
        assert!(read_history_json(EXTENDED.as_bytes(), &mut plays).unwrap());
        assert_eq!(plays.len(), 3, "podcast and incognito rows are dropped");
        assert_eq!(plays[0].track.id.as_deref(), Some("0VjIjW4GlUZAMYd2vXMi3b"));
        assert_eq!(plays[0].track.album.as_deref(), Some("After Hours"));
        assert_eq!(plays[0].end, 1_714_587_825);
        assert!(plays[2].skipped);
    }

    #[test]
    fn other_json_is_not_history() {
        let mut plays = Vec::new();
        assert!(!read_history_json(br#"{"tracks": []}"#, &mut plays).unwrap());
        assert!(!read_history_json(br#"[{"name": "x"}]"#, &mut plays).unwrap());
    }

    #[test]
    fn settle_prefers_extended_and_caps() {
        let mut plays = Vec::new();
        read_history_json(BASIC.as_bytes(), &mut plays).unwrap();
        read_history_json(EXTENDED.as_bytes(), &mut plays).unwrap();
        settle(&mut plays);
        assert!(plays.iter().all(|p| p.track.id.is_some()));
        assert!(plays.windows(2).all(|w| w[0].end >= w[1].end), "newest first");
    }

    #[test]
    fn top_tracks_rank_by_counted_plays() {
        let mut plays = Vec::new();
        read_history_json(BASIC.as_bytes(), &mut plays).unwrap();
        let top = top_tracks(&plays, 10);
        assert_eq!(top[0].title, "Blinding Lights", "three counted plays");
        assert!(!top.iter().any(|t| t.title == "Levitating"), "only ever skipped");
        assert_eq!(top_tracks(&plays, 1).len(), 1);
    }

    #[test]
    fn adopts_the_list_copy_of_a_song() {
        let mut plays = Vec::new();
        read_history_json(BASIC.as_bytes(), &mut plays).unwrap();
        let liked = SourceTrack {
            id: Some("0VjIjW4GlUZAMYd2vXMi3b".into()),
            title: "Blinding Lights".into(),
            artists: vec!["The Weeknd".into()],
            album: Some("After Hours".into()),
            ..Default::default()
        };
        adopt_list_tracks(&mut plays, std::iter::once(&liked));
        assert_eq!(plays[0].track, liked);
    }

    #[test]
    fn summary_counts_streams() {
        let mut plays = Vec::new();
        read_history_json(BASIC.as_bytes(), &mut plays).unwrap();
        let s = summary(&plays).unwrap();
        assert_eq!(s.plays, 4);
        assert_eq!(s.songs, 2);
        assert!(s.from < s.to);
    }

    #[test]
    fn writes_listens_and_events_once() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let mut plays = Vec::new();
        read_history_json(BASIC.as_bytes(), &mut plays).unwrap();
        let lookup = |t: &SourceTrack| match t.title.as_str() {
            "Blinding Lights" => Some(song("vid_bl", "Blinding Lights", "The Weeknd")),
            "Levitating" => Some(song("vid_lv", "Levitating", "Dua Lipa")),
            _ => None,
        };
        let w = write_plays(&db, &plays, lookup);
        assert_eq!(w, Written { listens: 3, skips: 1, already: 0, unmatched: 1 });
        let again = write_plays(&db, &plays, lookup);
        assert_eq!(again.already, 4, "a second import of the same file writes nothing");
        db.with_conn(|c| {
            let listens: i64 =
                c.query_row("SELECT COUNT(*) FROM listens", [], |r| r.get(0)).unwrap();
            assert_eq!(listens, 3);
            let (at, secs): (i64, i64) = c
                .query_row(
                    "SELECT played_at, secs FROM listens ORDER BY played_at DESC LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((at, secs), (1_714_587_780 - 200, 200), "original start time");
            let skips: i64 = c
                .query_row("SELECT COUNT(*) FROM smart_events WHERE kind = 'skip'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(skips, 1);
            let marked: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM imported_plays WHERE event_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(marked, 4);
        });
        // The taste profile reads them like live plays.
        let taste = crate::smart::taste::profile(&db);
        assert!(taste.tracks.get("vid_bl").copied().unwrap_or(0.0) > 0.0);
    }
}
