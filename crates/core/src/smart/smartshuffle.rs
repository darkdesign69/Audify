//! Smart Shuffle (stream smart2): Spotify's third shuffle state. In your own playlists (and Liked
//! Songs) it shuffles and puts a recommended song after every 3 playlist songs. The suggestions
//! come from the benchmark-winning seed voting (`recs::playlist_recommendations`, RRF over the
//! radios of the playlist's songs) and carry a sparkle in the UI, with add (to the playlist) and
//! remove ("don't suggest", a negative taste signal) buttons.
//!
//! The button cycles off → shuffle → smart shuffle → off. The suggestions are not part of the
//! pre-shuffle snapshot, so turning shuffle off drops them on its own.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use innertube::SongItem;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::state::AppState;

/// One suggestion after every `EVERY` playlist songs.
pub const EVERY: usize = 3;
const MAX_RECS: usize = 30;
const STATE_KEY: &str = "smart_shuffle_state";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartShuffle {
    pub on: bool,
    /// The playlist it was turned on for.
    pub source_id: Option<String>,
    /// The suggested rows in the queue (the sparkle rows).
    pub rec_ids: Vec<String>,
}

static STATE: Mutex<Option<SmartShuffle>> = Mutex::new(None);

/// Smart Shuffle works in playlists you own: local ones and Liked Songs.
pub fn eligible(source_id: Option<&str>) -> bool {
    source_id.is_some_and(|id| {
        crate::state::is_local_playlist(id) || id == crate::library::LIKED_ID
    })
}

pub fn current(db: &Db) -> SmartShuffle {
    let mut g = STATE.lock().unwrap();
    if g.is_none() {
        *g = Some(
            db.get_setting(STATE_KEY)
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default(),
        );
    }
    g.clone().unwrap_or_default()
}

fn store(state: &AppState, s: SmartShuffle) -> SmartShuffle {
    if let Ok(j) = serde_json::to_string(&s) {
        state.db.set_setting(STATE_KEY, &j);
    }
    *STATE.lock().unwrap() = Some(s.clone());
    use crate::host::HostExt;
    let _ = state.host.emit("smart-shuffle-changed", &s);
    s
}

/// The suggestions to mix in (pure): drop anything already queued, at most `n`.
pub fn pick(recs: Vec<SongItem>, queued: &HashSet<String>, n: usize) -> Vec<SongItem> {
    let mut seen = HashSet::new();
    recs.into_iter()
        .filter(|s| !queued.contains(&s.video_id) && seen.insert(s.video_id.clone()))
        .take(n)
        .map(|mut s| {
            s.autoplay = false;
            s.queued = false;
            s.queued_end = false;
            s
        })
        .collect()
}

/// Turn Smart Shuffle on or off for what is playing. On: shuffle goes on too and the suggestions
/// go in. Off: the suggestions come out and shuffle goes off (the button's next state is "off").
pub async fn set(state: &Arc<AppState>, on: bool) -> Result<SmartShuffle, String> {
    let prev = current(&state.db);
    if !prev.rec_ids.is_empty() {
        let ids: HashSet<String> = prev.rec_ids.iter().cloned().collect();
        state.smart_remove(&ids).await;
    }
    if !on {
        if state.shuffle_is_on().await {
            state.toggle_shuffle().await;
        }
        return Ok(store(state, SmartShuffle::default()));
    }
    let (source_id, _, _) = state.queue_source().await;
    if !eligible(source_id.as_deref()) {
        return Err("Smart Shuffle works in your own playlists".into());
    }
    let source_id = source_id.unwrap_or_default();
    if !state.shuffle_is_on().await {
        state.toggle_shuffle().await;
    }
    let upcoming = state.upcoming_playlist_len().await;
    let n = (upcoming / EVERY).clamp(1, MAX_RECS);
    let recs = super::recs::playlist_recommendations(state, &source_id, n + 5, 0)
        .await
        .unwrap_or_default();
    let queued: HashSet<String> = {
        let snap = state.queue_snapshot().await;
        snap.get("items")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|i| {
                        i.get("video_id").or_else(|| i.get("videoId")).and_then(|v| v.as_str())
                    })
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let recs = pick(recs.into_iter().map(|r| r.song).collect(), &queued, n);
    let ids: Vec<String> = recs.iter().map(|s| s.video_id.clone()).collect();
    let put = state.smart_insert(recs, EVERY).await;
    Ok(store(
        state,
        SmartShuffle { on: true, source_id: Some(source_id), rec_ids: ids.into_iter().take(put).collect() },
    ))
}

/// The user removed one suggestion: out of the queue, and a "not for me" signal for taste.
pub async fn remove(state: &Arc<AppState>, video_id: &str) -> SmartShuffle {
    let mut s = current(&state.db);
    let ids: HashSet<String> = [video_id.to_owned()].into_iter().collect();
    state.smart_remove(&ids).await;
    s.rec_ids.retain(|i| i != video_id);
    state.db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_events(video_id, artist_key, artist, kind, secs, duration,
                                      from_rec, private, at)
             VALUES(?1, '', '', 'skip', 0, NULL, 1, 0, ?2)",
            rusqlite::params![video_id, super::now_secs()],
        );
    });
    store(state, s)
}

/// The user added a suggestion to the playlist: it is no longer a suggestion.
pub fn keep(state: &AppState, video_id: &str) -> SmartShuffle {
    let mut s = current(&state.db);
    s.rec_ids.retain(|i| i != video_id);
    store(state, s)
}

/// The queue moved to something else: Smart Shuffle stays on as a mode only while the same
/// playlist plays. Called when the UI sees a new source.
pub async fn sync(state: &Arc<AppState>) -> SmartShuffle {
    let s = current(&state.db);
    if !s.on {
        return s;
    }
    let (source_id, _, _) = state.queue_source().await;
    if source_id == s.source_id {
        return s;
    }
    if eligible(source_id.as_deref()) && state.shuffle_is_on().await {
        // A new own playlist started with shuffle on: smart again, with its own suggestions.
        *STATE.lock().unwrap() = Some(SmartShuffle { rec_ids: Vec::new(), ..s });
        return set(state, true).await.unwrap_or_default();
    }
    store(state, SmartShuffle::default())
}

// ── lane magic-d3: Enhance (MAGIC-GAPS #13) ───────────────────────────────────────────────────
//
// Spotify's Enhance button on your own playlists: a recommended song after about every 2 of the
// playlist's songs, each with a sparkle, an add and a remove button. Turning it off takes them all
// out again. Unlike Smart Shuffle it doesn't shuffle: the suggestions sit in the playlist's own
// order, on the page and in the queue. The suggestions are the same seed-voting recommendations
// (`recs::playlist_recommendations`); they're kept per playlist so the page shows the same ones
// until they're a day old.

/// One suggestion after every `ENHANCE_EVERY` playlist songs.
pub const ENHANCE_EVERY: usize = 2;
const ENHANCE_MAX: usize = 50;
/// Suggestions older than this are swapped for fresh ones the next time the page asks.
const ENHANCE_TTL: i64 = 24 * 3600;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Enhance {
    pub playlist_id: String,
    pub on: bool,
    /// The suggestions, in the order they go in (the first after the playlist's 2nd song).
    pub recs: Vec<SongItem>,
    /// When they were picked (unix seconds).
    pub at: i64,
}

fn enhance_key(playlist_id: &str) -> String {
    format!("enhance:{playlist_id}")
}

/// Enhance works where Smart Shuffle does: your own playlists and Liked Songs.
pub fn enhance_eligible(playlist_id: &str) -> bool {
    eligible(Some(playlist_id))
}

fn enhance_load(db: &Db, playlist_id: &str) -> Enhance {
    db.get_setting(&enhance_key(playlist_id))
        .and_then(|j| serde_json::from_str::<Enhance>(&j).ok())
        .filter(|e| e.playlist_id == playlist_id)
        .unwrap_or_else(|| Enhance { playlist_id: playlist_id.to_owned(), ..Default::default() })
}

fn enhance_store(state: &AppState, e: Enhance) -> Enhance {
    if e.on {
        if let Ok(j) = serde_json::to_string(&e) {
            state.db.set_setting(&enhance_key(&e.playlist_id), &j);
        }
    } else {
        state.db.delete_setting(&enhance_key(&e.playlist_id));
    }
    use crate::host::HostExt;
    let _ = state.host.emit("enhance-changed", &e);
    e
}

/// How many suggestions a playlist of `len` songs gets.
pub fn enhance_count(len: usize) -> usize {
    (len / ENHANCE_EVERY).clamp(1, ENHANCE_MAX)
}

/// The playlist with its suggestions mixed in (pure): one after every `every` songs, none after
/// the last song. What the page shows and what Play queues.
pub fn interleave<T: Clone>(tracks: &[T], recs: &[T], every: usize) -> Vec<T> {
    let every = every.max(1);
    let mut out = Vec::with_capacity(tracks.len() + recs.len());
    let mut recs = recs.iter();
    for (i, t) in tracks.iter().enumerate() {
        out.push(t.clone());
        if (i + 1) % every == 0 && i + 1 < tracks.len() {
            match recs.next() {
                Some(r) => out.push(r.clone()),
                None => {
                    out.extend(tracks[i + 1..].iter().cloned());
                    break;
                }
            }
        }
    }
    out
}

async fn enhance_queue_in(state: &Arc<AppState>, e: &Enhance) {
    let (source_id, _, _) = state.queue_source().await;
    if source_id.as_deref() == Some(e.playlist_id.as_str()) && !state.shuffle_is_on().await {
        state.smart_insert(e.recs.clone(), ENHANCE_EVERY).await;
    }
}

async fn enhance_queue_out(state: &Arc<AppState>, ids: HashSet<String>) {
    if !ids.is_empty() {
        state.smart_remove(&ids).await;
    }
}

async fn enhance_pick(state: &Arc<AppState>, playlist_id: &str, n: usize) -> Vec<SongItem> {
    let recs = super::recs::playlist_recommendations(state, playlist_id, n + 5, 0)
        .await
        .unwrap_or_default();
    pick(recs.into_iter().map(|r| r.song).collect(), &HashSet::new(), n)
}

/// The page's view of Enhance for a playlist. A day-old set of suggestions is replaced.
pub async fn enhance_state(state: &Arc<AppState>, playlist_id: &str) -> Enhance {
    let e = enhance_load(&state.db, playlist_id);
    if !e.on || super::now_secs() - e.at < ENHANCE_TTL {
        return e;
    }
    let old: HashSet<String> = e.recs.iter().map(|s| s.video_id.clone()).collect();
    let recs = enhance_pick(state, playlist_id, e.recs.len().max(1)).await;
    if recs.is_empty() {
        return e;
    }
    enhance_queue_out(state, old).await;
    let e = Enhance { recs, at: super::now_secs(), ..e };
    enhance_queue_in(state, &e).await;
    enhance_store(state, e)
}

/// Turn Enhance on or off for a playlist of `len` songs. If that playlist is playing in order,
/// its upcoming queue follows.
pub async fn enhance_set(
    state: &Arc<AppState>,
    playlist_id: &str,
    on: bool,
    len: usize,
) -> Result<Enhance, String> {
    let prev = enhance_load(&state.db, playlist_id);
    enhance_queue_out(state, prev.recs.iter().map(|s| s.video_id.clone()).collect()).await;
    if !on {
        return Ok(enhance_store(state, Enhance { playlist_id: playlist_id.to_owned(), ..Default::default() }));
    }
    if !enhance_eligible(playlist_id) {
        return Err("Enhance works in your own playlists".into());
    }
    let recs = enhance_pick(state, playlist_id, enhance_count(len)).await;
    if recs.is_empty() {
        return Err("No suggestions for this playlist yet".into());
    }
    let e = Enhance { playlist_id: playlist_id.to_owned(), on: true, recs, at: super::now_secs() };
    enhance_queue_in(state, &e).await;
    Ok(enhance_store(state, e))
}

/// "Remove" on a suggestion: out of the page and the queue, and a "not for me" signal for taste.
pub async fn enhance_remove(state: &Arc<AppState>, playlist_id: &str, video_id: &str) -> Enhance {
    let mut e = enhance_load(&state.db, playlist_id);
    e.recs.retain(|s| s.video_id != video_id);
    enhance_queue_out(state, [video_id.to_owned()].into_iter().collect()).await;
    state.db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_events(video_id, artist_key, artist, kind, secs, duration,
                                      from_rec, private, at)
             VALUES(?1, '', '', 'skip', 0, NULL, 1, 0, ?2)",
            rusqlite::params![video_id, super::now_secs()],
        );
    });
    enhance_store(state, e)
}

/// The user added a suggestion to the playlist: it is a playlist song now, no longer a suggestion.
pub fn enhance_keep(state: &AppState, playlist_id: &str, video_id: &str) -> Enhance {
    let mut e = enhance_load(&state.db, playlist_id);
    e.recs.retain(|s| s.video_id != video_id);
    enhance_store(state, e)
}
// ── end lane magic-d3 ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    #[test]
    fn only_own_playlists() {
        assert!(eligible(Some("LOCALPLAYLIST:3")));
        assert!(eligible(Some(crate::library::LIKED_ID)));
        assert!(!eligible(Some("PL123")));
        assert!(!eligible(None));
    }

    #[test]
    fn suggestions_skip_what_is_queued() {
        let recs = vec![
            song("a", "A", "X", None, "3:00"),
            song("b", "B", "Y", None, "3:00"),
            song("a", "A", "X", None, "3:00"),
            song("c", "C", "Z", None, "3:00"),
        ];
        let queued: HashSet<String> = ["b".to_string()].into_iter().collect();
        let out = pick(recs, &queued, 5);
        assert_eq!(out.iter().map(|s| s.video_id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(out.iter().all(|s| !s.autoplay));
    }

    // lane magic-d3: Enhance
    #[test]
    fn enhance_mixes_one_in_every_two() {
        let t: Vec<u32> = (1..=7).collect();
        let r = [100, 200, 300, 400];
        assert_eq!(interleave(&t, &r, ENHANCE_EVERY), [1, 2, 100, 3, 4, 200, 5, 6, 300, 7]);
        // Fewer suggestions than slots: the rest of the playlist follows unchanged.
        assert_eq!(interleave(&t, &r[..1], ENHANCE_EVERY), [1, 2, 100, 3, 4, 5, 6, 7]);
        // Never one after the last song.
        assert_eq!(interleave(&[1, 2], &[9], 2), [1, 2]);
        assert_eq!(interleave::<u32>(&[], &[9], 2), Vec::<u32>::new());
    }

    #[test]
    fn enhance_count_scales_with_the_playlist() {
        assert_eq!(enhance_count(0), 1);
        assert_eq!(enhance_count(9), 4);
        assert_eq!(enhance_count(1000), ENHANCE_MAX);
        assert!(enhance_eligible("LOCALPLAYLIST:1") && !enhance_eligible("PLabc"));
    }
}
