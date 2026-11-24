//! Shuffle that feels random (research 07 "Shuffle").
//!
//! 1. **Artist spread**, ported from LMS `balancedShuffle` (`Slim/Player/Playlist.pm`, the Spotify
//!    2014 "How to shuffle songs?" method): group by primary artist, shuffle each group (spread by
//!    album inside it, the same way), then place a group of `k` in a list of `n` at
//!    `offset + i * n/k + jitter` with a random offset in `[0, n/k)`, and sort by position.
//! 2. **Fewer repeats** (Spotify 2025 "making random feel more human"): build several such orders
//!    and keep the one with the fewest recently played songs near the top and the fewest
//!    same-artist neighbours.
//!
//! The `"standard"` shuffle mode setting turns both off: plain Fisher-Yates.
//!
//! The queue calls [`shuffle_slice`] where it used to call `slice.shuffle(&mut thread_rng())`.

use std::collections::HashMap;

use innertube::SongItem;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use crate::identity;

/// Candidate orders tried by the fewer-repeats pass.
const CANDIDATES: usize = 10;
/// What counts as "recently played" for fewer repeats.
const RECENT_SECS: i64 = 72 * 3600;
/// Jitter width as a share of a group's step.
const JITTER: f64 = 0.2;

pub struct ShuffleCtx {
    pub smart: bool,
    /// videoId -> last played (unix secs).
    pub recent: HashMap<String, i64>,
    pub now: i64,
}

impl ShuffleCtx {
    #[allow(dead_code)]
    pub fn plain() -> ShuffleCtx {
        ShuffleCtx { smart: false, recent: HashMap::new(), now: 0 }
    }

    /// Settings and recent plays from the registered database, or a smart shuffle with no
    /// history when there is none (unit tests of the queue).
    pub fn load() -> ShuffleCtx {
        let now = super::now_secs();
        match super::db() {
            Some(db) => ShuffleCtx {
                smart: super::settings::smart_shuffle(db),
                recent: db.last_played(now - RECENT_SECS).into_iter().collect(),
                now,
            },
            None => ShuffleCtx { smart: true, recent: HashMap::new(), now },
        }
    }
}

/// The queue's hook: shuffle `items` in place the way the user's setting says.
pub fn shuffle_slice(items: &mut [SongItem]) {
    if items.len() < 2 {
        return;
    }
    let ctx = ShuffleCtx::load();
    shuffle_with(items, &ctx, &mut rand::thread_rng());
}

#[allow(dead_code)]
/// Deterministic for a given `seed`: what tests and a frozen/restorable shuffle use.
pub fn shuffle_seeded(items: &mut [SongItem], ctx: &ShuffleCtx, seed: u64) {
    shuffle_with(items, ctx, &mut rand::rngs::StdRng::seed_from_u64(seed));
}

pub fn shuffle_with<R: Rng>(items: &mut [SongItem], ctx: &ShuffleCtx, rng: &mut R) {
    if items.len() < 2 {
        return;
    }
    if !ctx.smart {
        items.shuffle(rng);
        return;
    }
    let idx: Vec<usize> = (0..items.len()).collect();
    let artists: Vec<String> = items.iter().map(artist_key).collect();
    let albums: Vec<String> = items.iter().map(album_key).collect();
    let mut best: Option<(f64, Vec<usize>)> = None;
    for _ in 0..CANDIDATES {
        let mut order = balanced(&idx, &|i| artists[i].clone(), Some(&|i| albums[i].clone()), rng);
        fix_adjacent(&mut order, &|i| &artists[i]);
        let s = penalty(&order, &artists, items, ctx);
        if best.as_ref().map_or(true, |(b, _)| s < *b) {
            best = Some((s, order));
        }
    }
    let order = best.map(|(_, o)| o).unwrap_or(idx);
    let old: Vec<SongItem> = items.to_vec();
    for (slot, i) in items.iter_mut().zip(order) {
        *slot = old[i].clone();
    }
}

pub fn artist_key(s: &SongItem) -> String {
    s.artist_id
        .clone()
        .or_else(|| s.artist_runs.iter().find_map(|r| r.id.clone()))
        .unwrap_or_else(|| identity::primary_artist(&s.artists))
}

fn album_key(s: &SongItem) -> String {
    s.album_id.clone().or_else(|| s.album.as_deref().map(identity::norm)).unwrap_or_default()
}

/// LMS `balancedShuffle`: spread each `group` evenly over the list with a random offset and a
/// little jitter. Inside a group, the members are themselves spread by `sub` (albums), so an
/// artist's songs also alternate between their albums.
fn balanced<R: Rng>(
    idx: &[usize],
    group: &dyn Fn(usize) -> String,
    sub: Option<&dyn Fn(usize) -> String>,
    rng: &mut R,
) -> Vec<usize> {
    let n = idx.len();
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    let mut keys: Vec<String> = Vec::new();
    for &i in idx {
        let k = group(i);
        if !groups.contains_key(&k) {
            keys.push(k.clone());
        }
        groups.entry(k).or_default().push(i);
    }
    let mut placed: Vec<(f64, usize)> = Vec::with_capacity(n);
    for k in keys {
        let mut members = groups.remove(&k).unwrap_or_default();
        members.shuffle(rng);
        let members = match sub {
            Some(sub) if members.len() > 2 => balanced(&members, sub, None, rng),
            _ => members,
        };
        let count = members.len();
        let step = n as f64 / count as f64;
        let offset = rng.gen::<f64>() * step;
        for (j, m) in members.into_iter().enumerate() {
            // Jitter under half a step keeps a group's own order.
            let jitter = if count > 1 { (rng.gen::<f64>() - 0.5) * JITTER * step } else { 0.0 };
            placed.push((offset + j as f64 * step + jitter, m));
        }
    }
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    placed.into_iter().map(|(_, i)| i).collect()
}

/// Break up same-artist neighbours where a swap with a later slot can do it without making a new
/// pair. Leaves them when nothing fits (one artist with most of the list).
fn fix_adjacent<'a>(order: &mut [usize], artist: &dyn Fn(usize) -> &'a String) {
    let n = order.len();
    for i in 1..n {
        if artist(order[i]) != artist(order[i - 1]) {
            continue;
        }
        let bad = artist(order[i]).clone();
        let fits = |order: &[usize], j: usize| -> bool {
            let a = artist(order[j]);
            if *a == bad || *a == *artist(order[i - 1]) {
                return false;
            }
            if i + 1 < n && i + 1 != j && *artist(order[i + 1]) == *a {
                return false;
            }
            // After the swap, slot j holds `bad`: its neighbours must not be `bad`.
            let left = if j - 1 == i { None } else { Some(artist(order[j - 1])) };
            let right = order.get(j + 1).map(|&x| artist(x));
            left.map_or(true, |l| *l != bad) && right.map_or(true, |r| *r != bad)
        };
        if let Some(j) = (i + 1..n).find(|&j| fits(order, j)) {
            order.swap(i, j);
        }
    }
}

/// Lower is better: recently played songs near the top (weighted by how near), and same-artist
/// neighbours anywhere.
fn penalty(order: &[usize], artists: &[String], items: &[SongItem], ctx: &ShuffleCtx) -> f64 {
    let n = order.len();
    let window = ((n as f64 * 0.2).ceil() as usize).max(3).min(n);
    let mut p = 0.0;
    for (pos, &i) in order.iter().take(window).enumerate() {
        if let Some(&at) = ctx.recent.get(&items[i].video_id) {
            let age = (ctx.now - at).max(0) as f64;
            let recency = if age < 24.0 * 3600.0 { 1.5 } else { 1.0 };
            p += recency * (1.0 - pos as f64 / window as f64);
        }
    }
    let adjacent = order.windows(2).filter(|w| artists[w[0]] == artists[w[1]]).count();
    p + 10.0 * adjacent as f64
}

#[cfg(test)]
#[path = "shuffle_bench.rs"]
mod bench;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    fn library() -> Vec<SongItem> {
        let mut v = Vec::new();
        for (a, n) in [("A", 6), ("B", 4), ("C", 3), ("D", 2), ("E", 1)] {
            for i in 0..n {
                let mut s = song(&format!("{a}{i}"), &format!("Song {i}"), a, None, "3:00");
                s.album = Some(format!("{a} album {}", i % 2));
                v.push(s);
            }
        }
        v
    }

    fn smart() -> ShuffleCtx {
        ShuffleCtx { smart: true, recent: HashMap::new(), now: 1_000_000 }
    }

    fn ids(v: &[SongItem]) -> Vec<String> {
        v.iter().map(|s| s.video_id.clone()).collect()
    }

    #[test]
    fn keeps_every_track() {
        let mut v = library();
        shuffle_seeded(&mut v, &smart(), 7);
        let mut got = ids(&v);
        got.sort();
        let mut want = ids(&library());
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn no_adjacent_same_artist_when_avoidable() {
        for seed in 0..200 {
            let mut v = library();
            shuffle_seeded(&mut v, &smart(), seed);
            let runs = v.windows(2).filter(|w| w[0].artists == w[1].artists).count();
            assert_eq!(runs, 0, "seed {seed}: {:?}", ids(&v));
        }
    }

    #[test]
    fn unavoidable_neighbours_are_kept_to_a_minimum() {
        // 5 of A and 1 of B: at least 3 A-A pairs are forced; never all five in a row.
        let mut v: Vec<SongItem> =
            (0..5).map(|i| song(&format!("a{i}"), "x", "A", None, "3:00")).collect();
        v.push(song("b", "x", "B", None, "3:00"));
        shuffle_seeded(&mut v, &smart(), 3);
        let runs = v.windows(2).filter(|w| w[0].artists == w[1].artists).count();
        assert!(runs <= 4);
        assert_eq!(v.len(), 6);
    }

    #[test]
    fn recently_played_songs_stay_out_of_the_top() {
        let lib = library();
        let recent: HashMap<String, i64> =
            ["A0", "B0", "C0"].iter().map(|id| (id.to_string(), 1_000_000 - 3600)).collect();
        let ctx = ShuffleCtx { smart: true, recent, now: 1_000_000 };
        let mut hits = 0;
        for seed in 0..100 {
            let mut v = lib.clone();
            shuffle_seeded(&mut v, &ctx, seed);
            hits += v.iter().take(3).filter(|s| ctx.recent.contains_key(&s.video_id)).count();
        }
        // Plain random would put ~ 3 * 3/16 * 100 = 56 of them in the top three.
        assert!(hits < 10, "recent songs reached the top {hits} times");
    }

    #[test]
    fn deterministic_with_a_seed() {
        let (mut a, mut b) = (library(), library());
        shuffle_seeded(&mut a, &smart(), 42);
        shuffle_seeded(&mut b, &smart(), 42);
        assert_eq!(ids(&a), ids(&b));
        let mut c = library();
        shuffle_seeded(&mut c, &smart(), 43);
        assert_ne!(ids(&a), ids(&c));
    }

    #[test]
    fn standard_mode_is_plain_random() {
        let mut v = library();
        shuffle_seeded(&mut v, &ShuffleCtx::plain(), 1);
        let mut got = ids(&v);
        assert_ne!(got, ids(&library()));
        got.sort();
        let mut want = ids(&library());
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn albums_alternate_inside_an_artist() {
        // One artist, two albums of three: the spread alternates them.
        let mut v: Vec<SongItem> = (0..6)
            .map(|i| {
                let mut s = song(&format!("t{i}"), "x", "A", None, "3:00");
                s.album = Some(format!("al{}", i % 2));
                s
            })
            .collect();
        shuffle_seeded(&mut v, &smart(), 9);
        let same = v.windows(2).filter(|w| w[0].album == w[1].album).count();
        assert!(same <= 1, "{:?}", v.iter().map(|s| s.album.clone()).collect::<Vec<_>>());
    }
}
