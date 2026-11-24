//! Candidate filtering and re-ranking (research 07 §4): every generator (radio, autoplay, Home,
//! playlist suggestions, mixes) runs the same final pass:
//!
//! 1. drop blocked songs/artists and, with the filter on, explicit songs;
//! 2. collapse versions (one row per work, the audio track over the video);
//! 3. at most two songs per artist in any ten;
//! 4. re-rank by taste, YouTube's order staying the base.
//!
//! Playlist suggestions are seed agreement ("voting", the Metrolist Quick-picks maths): radios of
//! several playlist tracks, each candidate scored by how many seeds point to it.

use std::collections::{HashMap, HashSet};

use innertube::SongItem;
use serde::Serialize;

use super::library::Filter;
use super::taste::Taste;
use crate::db::Db;
use crate::identity;
use crate::state::AppState;

/// Lookups to Deezer for unknown explicit flags per batch, when the filter is on.
const EXPLICIT_LOOKUPS: usize = 12;

/// Hook for radio/autoplay: drop what must never play from a fetched radio page. `keep` is the
/// seed song, which a radio still opens on. With the explicit filter on, unknown flags are asked
/// of Deezer first (radio rows carry no badge).
pub async fn filter_radio(db: &Db, items: &mut Vec<SongItem>, keep: Option<&str>) {
    let mut f = Filter::load(db);
    if f.explicit_on() {
        super::explicit::resolve_unknown(db, items, &mut f, EXPLICIT_LOOKUPS).await;
    }
    f.retain(items, keep);
}

/// Hook for radio/autoplay ordering, after Limusic's `freshen` (unheard first, YouTube's order).
/// Re-ranks the unheard part by taste and co-vote; the heard tail keeps its order.
pub fn rerank_radio(db: &Db, items: &mut [SongItem], heard: &HashMap<String, i64>) {
    let unheard = items.iter().take_while(|i| !heard.contains_key(&i.video_id)).count();
    if unheard < 3 {
        return;
    }
    let taste = super::taste::profile(db);
    let covote = covote_sets(db, COVOTE_SEEDS);
    rerank_with(&mut items[..unheard], &taste, &covote);
}

/// ── stream: autoplay ── [`rerank_radio`] without the co-vote term: autoplay after a single pick
/// or a context. bench5 (docs/streams/autoplay.md): co-vote with the last songs heard is neutral
/// after a search pick (+0.0002 NDCG@10) and costs 0.004 at a context's end, where those songs
/// are the context itself and already seed the radio.
pub fn rerank_autoplay(db: &Db, items: &mut [SongItem], heard: &HashMap<String, i64>) {
    let unheard = items.iter().take_while(|i| !heard.contains_key(&i.video_id)).count();
    if unheard < 3 {
        return;
    }
    let taste = super::taste::profile(db);
    rerank_with(&mut items[..unheard], &taste, &[]);
}

// Radio re-rank weights, from the offline benchmark (docs/bench/recommendations.md, task A:
// Last.fm-1K, 848 users). YouTube's position is the prior (-0.04 per row); taste at 1.0 and a
// 0.15 co-vote nudge took NDCG@10 from 0.204 (YouTube order) / 0.155 (v1: taste 0.25 + 2-per-10
// spread) to 0.225. A strict artist spread was the main loss, so only runs of more than
// `RUN_MAX` songs by one artist are broken up.
const POS_STEP: f64 = 0.04;
const TASTE_W: f64 = 1.0;
const COVOTE_W: f64 = 0.15;
const RUN_MAX: usize = 2;
/// Recently heard songs whose (cached) radios vote on a new radio's rows.
const COVOTE_SEEDS: usize = 5;

/// Seed agreement for a radio: the cached radios of the last `n` songs heard, as id sets. Cache
/// only (no network): radios land there from playlist suggestions, mixes and Smart Shuffle.
pub fn covote_sets(db: &Db, n: usize) -> Vec<HashSet<String>> {
    super::home::recent_songs(db, n)
        .iter()
        .filter_map(|h| {
            super::cache_get::<Vec<SongItem>>(db, &radio_key(&h.song.video_id), 7 * RADIO_TTL)
        })
        .map(|rows| rows.into_iter().map(|s| s.video_id).collect())
        .collect()
}

/// Standard scores (all zero when the values don't vary).
fn zscores(v: &[f64]) -> Vec<f64> {
    let n = v.len().max(1) as f64;
    let mean = v.iter().sum::<f64>() / n;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    v.iter().map(|x| if sd > 1e-12 { (x - mean) / sd } else { 0.0 }).collect()
}

/// YouTube's position as the prior, plus taste and co-vote, then no more than `RUN_MAX` in a row
/// by one artist.
pub fn rerank_with(items: &mut [SongItem], taste: &Taste, covote: &[HashSet<String>]) {
    if items.len() < 2 {
        return;
    }
    let votes: Vec<f64> = items
        .iter()
        .map(|s| covote.iter().filter(|set| set.contains(&s.video_id)).count() as f64)
        .collect();
    let votes = zscores(&votes);
    let mut scored: Vec<(f64, SongItem)> = items
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (-POS_STEP * i as f64 + TASTE_W * taste.affinity(s) + COVOTE_W * votes[i], s.clone())
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let ordered =
        spread_artists(scored.into_iter().map(|(_, s)| s).collect(), RUN_MAX, RUN_MAX + 1);
    for (slot, s) in items.iter_mut().zip(ordered) {
        *slot = s;
    }
}

/// At most `per` songs by one artist in any `window` consecutive rows, where the list allows:
/// an over-quota row waits for the first slot it fits. Nothing is dropped.
pub fn spread_artists(items: Vec<SongItem>, per: usize, window: usize) -> Vec<SongItem> {
    let mut out: Vec<SongItem> = Vec::with_capacity(items.len());
    let mut waiting: Vec<SongItem> = Vec::new();
    let fits = |out: &[SongItem], s: &SongItem| {
        let key = super::shuffle::artist_key(s);
        let from = out.len().saturating_sub(window - 1);
        out[from..].iter().filter(|o| super::shuffle::artist_key(o) == key).count() < per
    };
    for s in items {
        // Anything waiting that fits now goes first, in order.
        let mut i = 0;
        while i < waiting.len() {
            if fits(&out, &waiting[i]) {
                out.push(waiting.remove(i));
            } else {
                i += 1;
            }
        }
        if fits(&out, &s) {
            out.push(s);
        } else {
            waiting.push(s);
        }
    }
    while !waiting.is_empty() {
        let at = waiting.iter().position(|w| fits(&out, w)).unwrap_or(0);
        out.push(waiting.remove(at));
    }
    out
}

/// One row per work (identity's work key): the audio track over a video, the earlier row over a
/// later one. Research 07 §4 step 2.
pub fn collapse_versions(items: &mut Vec<SongItem>) {
    let mut best: HashMap<identity::WorkKey, usize> = HashMap::new();
    let mut drop = HashSet::new();
    for (i, s) in items.iter().enumerate() {
        let k = identity::work_key_song(s);
        match best.get(&k) {
            Some(&j) if items[j].is_video && !s.is_video => {
                drop.insert(j);
                best.insert(k, i);
            }
            Some(_) => {
                drop.insert(i);
            }
            None => {
                best.insert(k, i);
            }
        }
    }
    let mut i = 0;
    items.retain(|_| {
        let keep = !drop.contains(&i);
        i += 1;
        keep
    });
}

/// Hook for "play this list": what the user started, minus what must never play (hidden songs,
/// blocked artists, explicit songs with the filter on). `start` follows the clicked row, or the
/// next playable one when the clicked row itself is filtered.
pub fn playable(
    db: &Db,
    items: Vec<SongItem>,
    start: Option<usize>,
) -> (Vec<SongItem>, Option<usize>) {
    let f = Filter::load(db);
    if items.iter().all(|s| !f.rejects(s)) {
        return (items, start);
    }
    let mut out = Vec::with_capacity(items.len());
    let mut new_start = None;
    for (i, s) in items.into_iter().enumerate() {
        let ok = !f.rejects(&s);
        if start.is_some_and(|st| i >= st) && new_start.is_none() && ok {
            new_start = Some(out.len());
        }
        if ok {
            out.push(s);
        }
    }
    let start = start.map(|_| new_start.unwrap_or(0));
    (out, start)
}

// --- search --------------------------------------------------------------------------------------

/// Hook for the Songs search: remember the badges, then re-rank (studio first, versions the query
/// didn't ask for lower, duplicates of one work behind the rest).
pub fn rerank_search_songs(db: &Db, query: &str, items: Vec<SongItem>) -> Vec<SongItem> {
    super::explicit::remember_rows(db, &items, true);
    identity::rerank_search_songs(query, items)
}

/// Hook for the unfiltered search page: re-rank the Songs shelf, and when the Top Result is a
/// music video or a version the query didn't ask for, swap in the audio track of the same song
/// from the Songs shelf (research 22 "If the Top Result is an OMV, swap in the ATV").
pub fn rerank_search_all(
    db: &Db,
    query: &str,
    r: innertube::SearchResults,
) -> innertube::SearchResults {
    super::explicit::remember_cards(db, &r.songs);
    super::explicit::remember_cards(db, &r.top);
    identity::rerank_search_all(query, r) // ── stream: bench2 ── pure part moved to identity
}

// --- playlist suggestions ------------------------------------------------------------------------

/// Seeds asked per batch.
const SEEDS: usize = 5;
pub const BASIS_PLAYLIST: &str = "playlist";
pub const BASIS_LISTENING: &str = "listening";
const RADIO_TTL: i64 = 24 * 3600;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub song: SongItem,
    /// How many of the seeds recommended it.
    pub votes: usize,
    /// The seed titles that did, for a "Because you have X" hint.
    pub because: Vec<String>,
    /// What the seeds were: `playlist` (its own songs) or `listening` (an empty playlist falls
    /// back on what you play, see [`listening_seeds`]). The UIs word the section's subtitle by it.
    pub basis: &'static str,
}

/// RRF constant `k` and scale (benchmark task B; k = 10 tied k = 3 within noise).
const RRF_K: f64 = 10.0;
const RRF_SCALE: f64 = 10.0;

/// Seed agreement over seed radios (pure). `playlist` is what the playlist holds; `radios` are
/// `(seed, its radio rows in YouTube's order)`.
pub fn vote(
    playlist: &[SongItem],
    radios: &[(SongItem, Vec<SongItem>)],
    filter: &Filter,
    taste: &Taste,
    n: usize,
) -> Vec<Recommendation> {
    let in_ids: HashSet<&str> = playlist.iter().map(|s| s.video_id.as_str()).collect();
    let in_works: HashSet<identity::WorkKey> =
        playlist.iter().map(identity::work_key_song).collect();
    let in_artists: HashSet<String> = playlist.iter().map(super::shuffle::artist_key).collect();
    struct Tally {
        song: SongItem,
        votes: usize,
        rrf: f64,
        because: Vec<String>,
    }
    let mut tally: HashMap<identity::WorkKey, Tally> = HashMap::new();
    let mut order: Vec<identity::WorkKey> = Vec::new();
    for (seed, rows) in radios {
        let mut seen_here = HashSet::new();
        for (i, s) in rows.iter().enumerate() {
            if in_ids.contains(s.video_id.as_str()) || filter.rejects(s) {
                continue;
            }
            let k = identity::work_key_song(s);
            if in_works.contains(&k) || !seen_here.insert(k.clone()) {
                continue;
            }
            let t = tally.entry(k.clone()).or_insert_with(|| {
                order.push(k.clone());
                Tally { song: s.clone(), votes: 0, rrf: 0.0, because: Vec::new() }
            });
            // The audio track wins over a video of the same work.
            if t.song.is_video && !s.is_video {
                t.song = s.clone();
            }
            t.votes += 1;
            t.rrf += 1.0 / (i as f64 + RRF_K);
            t.because.push(seed.title.clone());
        }
    }
    let mut ranked: Vec<(f64, Tally)> = order
        .into_iter()
        .filter_map(|k| tally.remove(&k))
        .map(|t| {
            // Reciprocal-rank fusion over the seed radios (benchmark task B: NDCG@10 0.034 ->
            // 0.051 against v1's votes + overlap - average rank). Ties go to the playlist's
            // artists.
            let overlap = in_artists.contains(&super::shuffle::artist_key(&t.song)) as u8 as f64;
            let s = RRF_SCALE * t.rrf + 0.3 * taste.affinity(&t.song) + 1e-3 * overlap;
            (s, t)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let songs: Vec<SongItem> = ranked.iter().map(|(_, t)| t.song.clone()).collect();
    let spread = spread_artists(songs, 2, 10);
    let mut by_id: HashMap<String, Tally> =
        ranked.into_iter().map(|(_, t)| (t.song.video_id.clone(), t)).collect();
    spread
        .into_iter()
        .filter_map(|s| by_id.remove(&s.video_id))
        .take(n)
        .map(|t| Recommendation {
            song: t.song,
            votes: t.votes,
            because: t.because,
            basis: BASIS_PLAYLIST,
        })
        .collect()
}

/// Which playlist tracks seed batch `refresh`: most played and most recently added first,
/// alternating, then rotating by `SEEDS` per refresh.
pub fn pick_seeds(
    tracks: &[(SongItem, i64)],
    plays: &HashMap<String, i64>,
    refresh: usize,
) -> Vec<SongItem> {
    let mut by_plays: Vec<&(SongItem, i64)> = tracks.iter().collect();
    by_plays.sort_by_key(|(s, at)| {
        (std::cmp::Reverse(plays.get(&s.video_id).copied().unwrap_or(0)), std::cmp::Reverse(*at))
    });
    let mut by_recent: Vec<&(SongItem, i64)> = tracks.iter().collect();
    by_recent.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    let mut order: Vec<SongItem> = Vec::new();
    let mut seen = HashSet::new();
    for (a, b) in by_plays.iter().zip(by_recent.iter()) {
        for (s, _) in [a, b] {
            if seen.insert(s.video_id.clone()) {
                order.push(s.clone());
            }
        }
    }
    order.retain(|s| !crate::local::is_local_song(&s.video_id));
    if order.is_empty() {
        return order;
    }
    let start = (refresh * SEEDS) % order.len();
    order.iter().cycle().skip(start).take(SEEDS.min(order.len())).cloned().collect()
}

/// ── stream: canonical ── `radio2:`: rows cached before the canonical pass held music videos.
fn radio_key(video_id: &str) -> String {
    format!("radio2:{video_id}")
}

/// A song's radio rows, cached for a day.
pub async fn song_radio(state: &AppState, video_id: &str) -> Vec<SongItem> {
    let key = radio_key(video_id); // ── stream: canonical ──
    if let Some(v) = super::cache_get::<Vec<SongItem>>(&state.db, &key, RADIO_TTL) {
        return v;
    }
    let Some(client) = state.clients.get(innertube::METADATA_CLIENT) else { return Vec::new() };
    let seed = format!("RDAMVM{video_id}");
    match state.it.next(client, Some(video_id), Some(&seed)).await {
        Ok(r) => {
            // ── stream: canonical ── a music video's radio is a radio of videos: run it on the
            // video's song, then songs only (studio versions, music videos mapped, deduped).
            let mut rows = r.items;
            let mut on = video_id.to_owned();
            let seed_row = rows.iter().find(|s| s.video_id == video_id).cloned();
            let atv = match seed_row.as_ref() {
                Some(s) => crate::canonical::audio_seed(state, s).await,
                None => None,
            };
            if let Some(atv) = atv {
                if let Ok(r2) = state.it.next(client, Some(&atv), Some(&format!("RDAMVM{atv}"))).await {
                    rows = r2.items;
                    on = atv;
                }
            }
            let seed_row = rows.iter().find(|s| s.video_id == on).cloned().or(seed_row);
            let rows = crate::canonical::canonicalize(state, rows, seed_row.as_ref()).await;
            let items: Vec<SongItem> =
                rows.into_iter().filter(|s| s.video_id != video_id && s.video_id != on).collect();
            // ── end stream: canonical ──
            super::cache_put(&state.db, &key, &items);
            items
        }
        Err(e) => {
            tracing::warn!(error = %e, "smart: radio fetch failed");
            Vec::new()
        }
    }
}

/// The tracks of a playlist with when each was added: a local one from SQLite, a YouTube one
/// from its first page.
async fn playlist_tracks(
    state: &AppState,
    playlist_id: &str,
) -> Result<Vec<(SongItem, i64)>, String> {
    if let Some(n) = playlist_id.strip_prefix(crate::state::LOCAL_PLAYLIST_PREFIX) {
        let id: i64 = n.parse().map_err(|_| "bad playlist id")?;
        return Ok(state
            .db
            .local_playlist_tracks(id)
            .into_iter()
            .filter_map(|(_, json, at)| serde_json::from_str(&json).ok().map(|s| (s, at)))
            .collect());
    }
    // ── smart2 ── Liked Songs is local (Smart Shuffle runs there too).
    if playlist_id == crate::library::LIKED_ID {
        return Ok(state.db.with_conn(|c| {
            let Ok(mut st) = c.prepare("SELECT song_json, added_at FROM liked_songs") else {
                return Vec::new();
            };
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map(|rows| {
                    rows.flatten()
                        .filter_map(|(j, at)| serde_json::from_str(&j).ok().map(|s| (s, at)))
                        .collect()
                })
                .unwrap_or_default()
        }));
    }
    let client = state.clients.get(innertube::METADATA_CLIENT).ok_or("metadata client missing")?;
    let browse = if playlist_id.starts_with("VL") {
        playlist_id.to_owned()
    } else {
        format!("VL{playlist_id}")
    };
    let page = state.it.playlist(client, &browse, None).await.map_err(|e| e.to_string())?;
    Ok(page.items.into_iter().map(|s| (s.clone(), s.added_at.unwrap_or(0))).collect())
}

/// "Recommended" under a playlist: `n` songs that fit it, batch `refresh` (0, 1, 2 ... for each
/// press of Refresh, which moves to the next seeds).
pub async fn playlist_recommendations(
    state: &AppState,
    playlist_id: &str,
    n: usize,
    refresh: usize,
) -> Result<Vec<Recommendation>, String> {
    let tracks = playlist_tracks(state, playlist_id).await?;
    let plays: HashMap<String, i64> = state.db.play_counts(0).into_iter().collect();
    let taste = super::taste::profile(&state.db);
    let mut seeds = pick_seeds(&tracks, &plays, refresh);
    let mut basis = BASIS_PLAYLIST;
    if seeds.is_empty() {
        // A new or empty playlist (or one of files only): suggest from what you listen to.
        seeds = pick_seeds(&listening_seeds(&state.db, &taste), &plays, refresh);
        basis = BASIS_LISTENING;
    }
    if seeds.is_empty() {
        return Ok(Vec::new());
    }
    let radios =
        futures_util::future::join_all(seeds.iter().map(|s| song_radio(state, &s.video_id))).await;
    let mut radios: Vec<(SongItem, Vec<SongItem>)> = seeds.into_iter().zip(radios).collect();
    let mut filter = Filter::load(&state.db);
    if filter.explicit_on() {
        let all: Vec<SongItem> = radios.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        super::explicit::resolve_unknown(&state.db, &all, &mut filter, EXPLICIT_LOOKUPS).await;
    }
    for (_, r) in &mut radios {
        r.iter_mut().for_each(|s| s.autoplay = false);
    }
    let playlist: Vec<SongItem> = tracks.into_iter().map(|(s, _)| s).collect();
    let mut out = vote(&playlist, &radios, &filter, &taste, n);
    out.iter_mut().for_each(|r| r.basis = basis);
    Ok(out)
}

/// Seeds for a playlist with nothing to seed from: your recent songs and your most played of the
/// last 90 days (the stats tables), then your latest Liked Songs, each with when it was last
/// heard or liked so [`pick_seeds`] can alternate plays and recency. Songs the taste layer has
/// learned you skip, podcast episodes and local files are left out.
pub fn listening_seeds(db: &Db, taste: &Taste) -> Vec<(SongItem, i64)> {
    const EACH: usize = 25;
    const TOP_WINDOW: i64 = 90 * 24 * 3600;
    let now = crate::db::now_secs();
    let last: HashMap<String, i64> = db.last_played(0).into_iter().collect();
    let mut out: Vec<(SongItem, i64)> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |s: SongItem, at: i64, out: &mut Vec<(SongItem, i64)>| {
        if crate::podcasts::is_episode_item(&s)
            || crate::local::is_local_song(&s.video_id)
            || taste.track(&s.video_id) < 0.0
            || !seen.insert(s.video_id.clone())
        {
            return;
        }
        out.push((s, at));
    };
    for r in db.with_conn(|c| crate::library::recent_tracks(c, EACH)) {
        push(r.song, r.played_at, &mut out);
    }
    for (json, _) in db.top_plays(now - TOP_WINDOW, EACH) {
        if let Ok(s) = serde_json::from_str::<SongItem>(&json) {
            let at = last.get(&s.video_id).copied().unwrap_or(0);
            push(s, at, &mut out);
        }
    }
    let liked: Vec<(String, i64)> = db.with_conn(|c| {
        let Ok(mut st) =
            c.prepare("SELECT song_json, added_at FROM liked_songs ORDER BY added_at DESC LIMIT ?1")
        else {
            return Vec::new();
        };
        st.query_map([EACH as i64], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    });
    for (json, at) in liked {
        if let Ok(s) = serde_json::from_str::<SongItem>(&json) {
            push(s, at, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    fn s(id: &str, artist: &str) -> SongItem {
        song(id, &format!("Title {id}"), artist, None, "3:00")
    }

    #[test]
    fn seeds_that_agree_win() {
        let playlist = vec![s("p1", "A"), s("p2", "B"), s("p3", "C")];
        let radios = vec![
            (s("p1", "A"), vec![s("x", "X"), s("both", "D"), s("p2", "B")]),
            (s("p2", "B"), vec![s("both", "D"), s("y", "Y")]),
            (s("p3", "C"), vec![s("both", "D"), s("z", "Z"), s("blocked", "Bad")]),
        ];
        let f = Filter::new(&[], vec![(None, "Bad".into())], false);
        let out = vote(&playlist, &radios, &f, &Taste::default(), 10);
        let ids: Vec<&str> = out.iter().map(|r| r.song.video_id.as_str()).collect();
        assert_eq!(ids[0], "both");
        assert_eq!(out[0].votes, 3);
        assert!(!ids.contains(&"p2"), "already in the playlist");
        assert!(!ids.contains(&"blocked"));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn another_version_of_a_playlist_song_is_not_suggested() {
        let playlist = vec![song("p", "Creep", "Radiohead", None, "3:59")];
        let mut video = song("v", "Creep (Official Video)", "Radiohead", None, "4:20");
        video.is_video = true;
        let radios = vec![(playlist[0].clone(), vec![video, s("n", "Muse")])];
        let out = vote(&playlist, &radios, &Filter::new(&[], vec![], false), &Taste::default(), 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].song.video_id, "n");
    }

    /// An empty playlist used to get no Recommended at all: it now seeds from your listening.
    #[test]
    fn an_empty_playlist_seeds_from_what_you_listen_to() {
        let db = super::super::test_db();
        let j = |x: &SongItem| serde_json::to_string(x).unwrap();
        let (a, b, skipped) = (s("a", "A"), s("b", "B"), s("skip", "C"));
        let now = crate::db::now_secs();
        db.record_listen("a", &j(&a), now - 50, 200);
        db.record_listen("a", &j(&a), now - 40, 200);
        db.record_listen("skip", &j(&skipped), now - 30, 5);
        let mut ep = s("ep", "Show");
        ep.is_episode = true;
        db.record_episode_listen("ep", &j(&ep), None, now - 20, 900);
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO liked_songs(video_id, title, artists, song_json, added_at)
                 VALUES('b', 'B', 'B', ?1, 7)",
                [j(&b)],
            )
            .unwrap()
        });
        let mut taste = Taste::default();
        taste.tracks.insert("skip".into(), -2.0);

        let seeds = listening_seeds(&db, &taste);
        let ids: Vec<&str> = seeds.iter().map(|(x, _)| x.video_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "recent first, then liked; no skips, no episodes, no repeats");
        let picked = pick_seeds(&seeds, &HashMap::new(), 0);
        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn refresh_moves_to_the_next_seeds() {
        let tracks: Vec<(SongItem, i64)> =
            (0..12).map(|i| (s(&format!("t{i}"), "A"), i as i64)).collect();
        let plays = HashMap::new();
        let a = pick_seeds(&tracks, &plays, 0);
        let b = pick_seeds(&tracks, &plays, 1);
        assert_eq!(a.len(), 5);
        assert!(a.iter().all(|x| !b.iter().any(|y| y.video_id == x.video_id)));
        // Most recently added first.
        assert_eq!(a[0].video_id, "t11");
    }

    #[test]
    fn artists_are_spread() {
        let items: Vec<SongItem> =
            vec![s("a1", "A"), s("a2", "A"), s("a3", "A"), s("b1", "B"), s("a4", "A")];
        let out = spread_artists(items, 2, 3);
        let ids: Vec<&str> = out.iter().map(|x| x.video_id.as_str()).collect();
        assert_eq!(ids, ["a1", "a2", "b1", "a3", "a4"]);
    }

    #[test]
    fn taste_nudges_but_youtube_order_stays_the_base() {
        let mut items: Vec<SongItem> =
            (0..4).map(|i| s(&format!("r{i}"), &format!("Ar{i}"))).collect();
        let mut taste = Taste::default();
        taste.tracks.insert("r3".into(), 10.0);
        taste.tracks.insert("r1".into(), -10.0);
        rerank_with(&mut items, &taste, &[]);
        let ids: Vec<&str> = items.iter().map(|x| x.video_id.as_str()).collect();
        assert_eq!(ids[0], "r3");
        assert_eq!(*ids.last().unwrap(), "r1");
        assert_eq!(ids[1], "r0");
    }

    #[test]
    fn covote_lifts_rows_your_recent_radios_agree_on() {
        let mut items: Vec<SongItem> =
            (0..6).map(|i| s(&format!("r{i}"), &format!("Ar{i}"))).collect();
        let set: HashSet<String> = ["r5".to_string()].into_iter().collect();
        rerank_with(&mut items, &Taste::default(), &[set.clone(), set.clone(), set]);
        assert_eq!(items[0].video_id, "r5");
        assert_eq!(items[1].video_id, "r0");
    }

    #[test]
    fn radio_runs_by_one_artist_stop_at_two() {
        let mut items = vec![s("a1", "A"), s("a2", "A"), s("a3", "A"), s("b1", "B"), s("a4", "A")];
        rerank_with(&mut items, &Taste::default(), &[]);
        let ids: Vec<&str> = items.iter().map(|x| x.video_id.as_str()).collect();
        assert_eq!(ids, ["a1", "a2", "b1", "a3", "a4"]);
    }

    #[test]
    fn rrf_prefers_a_seed_top_pick_over_a_deep_double_vote() {
        let playlist = vec![s("p1", "A"), s("p2", "B")];
        let mut r1: Vec<SongItem> = vec![s("top", "T")];
        r1.extend((0..40).map(|i| s(&format!("f{i}"), &format!("F{i}"))));
        r1.push(s("deep", "D"));
        let mut r2: Vec<SongItem> = (0..40).map(|i| s(&format!("g{i}"), &format!("G{i}"))).collect();
        r2.push(s("deep", "D"));
        let radios = vec![(playlist[0].clone(), r1), (playlist[1].clone(), r2)];
        let out = vote(&playlist, &radios, &Filter::new(&[], vec![], false), &Taste::default(), 50);
        let pos = |id: &str| out.iter().position(|r| r.song.video_id == id).unwrap();
        assert!(pos("top") < pos("deep"));
        assert_eq!(out[pos("deep")].votes, 2);
    }

    #[test]
    fn playable_skips_hidden_rows_and_follows_the_start() {
        let db = super::super::test_db();
        db.with_conn(|c| {
            c.execute("INSERT INTO hidden_songs(video_id, context_id, hidden_at) VALUES('b', '', 0)", []).unwrap()
        });
        let items = vec![s("a", "A"), s("b", "B"), s("c", "C")];
        let (out, start) = playable(&db, items.clone(), Some(1));
        assert_eq!(out.iter().map(|x| x.video_id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(start, Some(1));
        let (_, start) = playable(&db, items, Some(2));
        assert_eq!(start, Some(1));
    }

    #[test]
    fn collapse_keeps_the_audio_track() {
        let mut v = song("v", "Hello (Official Video)", "Adele", None, "6:07");
        v.is_video = true;
        let mut items = vec![v, song("a", "Hello", "Adele", None, "4:55"), s("o", "X")];
        collapse_versions(&mut items);
        assert_eq!(items.iter().map(|x| x.video_id.as_str()).collect::<Vec<_>>(), ["a", "o"]);
    }
}
