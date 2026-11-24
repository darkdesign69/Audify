//! Shuffle benchmark (stream: bench). Our shuffle against plain Fisher–Yates, Martin Fiedler's
//! "Balanced Shuffle" (keyj.emphy.de/balanced-shuffle, 2007), Spotify's 2014 dithering (Lukáš
//! Poláček, "How to shuffle songs?") and the LMS original (`Slim/Player/Playlist.pm`
//! `balancedShuffle`, including its alphabetical sort of the weights).
//!
//! Playlists: `tests/fixtures/smart/shuffle_playlists.json`, the primary artist of every track of
//! 27 public Spotify editorial playlists (read from their embed pages, 2026-11-15), plus 12
//! skewed "personal library" lists resampled (Zipf) from those playlists' artists and two small
//! synthetic edge cases.
//!
//! `cargo test -p limusic-app --lib shuffle_bench -- --ignored --nocapture` prints the tables
//! that `docs/bench/shuffle.md` quotes. The quick (not ignored) test guards the headline result.

use std::collections::HashMap;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use super::*;
use crate::identity::tests::song;

#[derive(serde::Deserialize)]
struct Playlist {
    name: String,
    #[allow(dead_code)]
    source: String,
    artists: Vec<String>,
}

fn playlists() -> Vec<Playlist> {
    serde_json::from_str(include_str!("../../tests/fixtures/smart/shuffle_playlists.json")).unwrap()
}

// --- the contenders ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Algo {
    FisherYates,
    Fiedler,
    Spotify2014,
    LmsOriginal,
    LmsNumeric,
    OursNoBest,
    Ours,
}

const ALGOS: [Algo; 7] = [
    Algo::FisherYates,
    Algo::Fiedler,
    Algo::Spotify2014,
    Algo::LmsOriginal,
    Algo::LmsNumeric,
    Algo::OursNoBest,
    Algo::Ours,
];

impl Algo {
    fn name(self) -> &'static str {
        match self {
            Algo::FisherYates => "Fisher–Yates",
            Algo::Fiedler => "Fiedler balanced",
            Algo::Spotify2014 => "Spotify 2014 dithering",
            Algo::LmsOriginal => "LMS original (alpha sort)",
            Algo::LmsNumeric => "LMS, numeric sort",
            Algo::OursNoBest => "Ours, one pass (no best-of-N)",
            Algo::Ours => "Ours (smart)",
        }
    }
}

fn group(artists: &[String]) -> Vec<Vec<usize>> {
    let mut at: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, a) in artists.iter().enumerate() {
        let g = *at.entry(a.as_str()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(i);
    }
    groups
}

/// Fiedler's fill: `k` real items spread over `n` slots (None = dummy), segments of n/k ±10 %,
/// then rotated by a random offset. Inverted when there are more items than dummies.
fn fiedler_fill<R: Rng>(items: Vec<usize>, n: usize, rng: &mut R) -> Vec<Option<usize>> {
    let k_items = items.len();
    let dummies = n - k_items;
    if dummies == 0 {
        return items.into_iter().map(Some).collect();
    }
    let invert = k_items > dummies;
    let k0 = if invert { dummies } else { k_items };
    // true = the "fewer" element at the head of each segment.
    let mut pattern = Vec::with_capacity(n);
    let (mut rem, mut k) = (n, k0);
    while k > 0 {
        let r = if k == 1 {
            rem
        } else {
            let opt = rem as f64 / k as f64;
            let r = (opt * (1.0 + rng.gen_range(-0.1..0.1))).round() as usize;
            r.clamp(1, rem - k + 1)
        };
        pattern.push(true);
        pattern.extend(std::iter::repeat(false).take(r - 1));
        rem -= r;
        k -= 1;
    }
    let off = rng.gen_range(0..n);
    pattern.rotate_right(off);
    let mut it = items.into_iter();
    pattern.into_iter().map(|few| if few != invert { it.next() } else { None }).collect()
}

fn fiedler<R: Rng>(artists: &[String], rng: &mut R) -> Vec<usize> {
    let mut groups = group(artists);
    for g in &mut groups {
        g.shuffle(rng);
    }
    let longest = groups.iter().map(Vec::len).max().unwrap_or(0);
    let filled: Vec<Vec<Option<usize>>> =
        groups.into_iter().map(|g| fiedler_fill(g, longest, rng)).collect();
    let mut out: Vec<usize> = Vec::with_capacity(artists.len());
    for col in 0..longest {
        let mut c: Vec<usize> = filled.iter().filter_map(|g| g[col]).collect();
        c.shuffle(rng);
        if let (Some(&last), Some(&first)) = (out.last(), c.first()) {
            if artists[last] == artists[first] {
                let f = c.remove(0);
                c.push(f);
            }
        }
        out.extend(c);
    }
    out
}

/// Poláček 2014: each artist's songs spread over [0, 1) at 1/k steps from a random offset, each
/// position nudged a little (±10 % of the step), then everything sorted by position.
fn spotify2014<R: Rng>(artists: &[String], rng: &mut R) -> Vec<usize> {
    let mut placed = Vec::with_capacity(artists.len());
    for mut g in group(artists) {
        g.shuffle(rng);
        let step = 1.0 / g.len() as f64;
        let offset = rng.gen::<f64>() * step;
        for (j, i) in g.into_iter().enumerate() {
            let jitter = rng.gen_range(-0.1..0.1) * step;
            placed.push((offset + j as f64 * step + jitter, i));
        }
    }
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    placed.into_iter().map(|(_, i)| i).collect()
}

/// LMS `balancedShuffle` as written: offset `rand(1/k)*count`, spacer `count/k`, jitter
/// `±k/10` *slots* (it grows with the group, not the step), and, as `reshuffle` calls it with
/// `'alpha'`, the weights sorted as strings.
fn lms<R: Rng>(artists: &[String], rng: &mut R, alpha: bool) -> Vec<usize> {
    let count = artists.len() as f64;
    let mut placed: Vec<(f64, usize)> = Vec::with_capacity(artists.len());
    for mut g in group(artists) {
        g.shuffle(rng);
        let k = g.len() as f64;
        let offset = rng.gen::<f64>() * (1.0 / k) * count;
        let spacer = count / k;
        for (j, i) in g.into_iter().enumerate() {
            let jitter = -(k / 10.0) + rng.gen::<f64>() * (k / 10.0 * 2.0);
            placed.push((offset + j as f64 * spacer + jitter, i));
        }
    }
    if alpha {
        // Perl stringifies with %.15g.
        let mut keyed: Vec<(String, usize)> =
            placed.into_iter().map(|(w, i)| (perl_num(w), i)).collect();
        keyed.sort();
        keyed.into_iter().map(|(_, i)| i).collect()
    } else {
        placed.sort_by(|a, b| a.0.total_cmp(&b.0));
        placed.into_iter().map(|(_, i)| i).collect()
    }
}

/// Perl's number-to-string (%.15g); Rust's shortest form sorts the same way for these weights.
fn perl_num(w: f64) -> String {
    format!("{w}")
}

struct Case {
    artists: Vec<String>,
    items: Vec<SongItem>,
    recent: HashMap<String, i64>,
}

const NOW: i64 = 10_000_000;

fn case(p: &Playlist, seed: u64) -> Case {
    let items: Vec<SongItem> = p
        .artists
        .iter()
        .enumerate()
        .map(|(i, a)| song(&format!("v{i}"), &format!("t{i}"), a, None, "3:00"))
        .collect();
    // ~15 % played in the last 72 h (10 h ago).
    let mut rng = StdRng::seed_from_u64(seed ^ 0x5eed);
    let mut ids: Vec<usize> = (0..items.len()).collect();
    ids.shuffle(&mut rng);
    let k = ((items.len() as f64) * 0.15).round() as usize;
    let recent = ids[..k].iter().map(|&i| (format!("v{i}"), NOW - 36_000)).collect();
    Case { artists: p.artists.clone(), items, recent }
}

fn run(algo: Algo, c: &Case, with_recent: bool, rng: &mut StdRng) -> Vec<usize> {
    match algo {
        Algo::FisherYates => {
            let mut v: Vec<usize> = (0..c.artists.len()).collect();
            v.shuffle(rng);
            v
        }
        Algo::Fiedler => fiedler(&c.artists, rng),
        Algo::Spotify2014 => spotify2014(&c.artists, rng),
        Algo::LmsOriginal => lms(&c.artists, rng, true),
        Algo::LmsNumeric => lms(&c.artists, rng, false),
        Algo::OursNoBest => {
            let idx: Vec<usize> = (0..c.items.len()).collect();
            let keys: Vec<String> = c.items.iter().map(artist_key).collect();
            let mut o = balanced(&idx, &|i| keys[i].clone(), None, rng);
            fix_adjacent(&mut o, &|i| &keys[i]);
            o
        }
        Algo::Ours => {
            let ctx = ShuffleCtx {
                smart: true,
                recent: if with_recent { c.recent.clone() } else { HashMap::new() },
                now: NOW,
            };
            let mut v = c.items.clone();
            shuffle_with(&mut v, &ctx, rng);
            v.iter().map(|s| s.video_id[1..].parse::<usize>().unwrap()).collect()
        }
    }
}

// --- metrics -------------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct M {
    adj_rate: f64,
    excess: f64,
    clumpy_runs: f64,
    min_gap: f64,
    mean_gap: f64,
    spread: f64,
    recent_top: f64,
    chi_df: f64,
    nonuniform: f64,
}

const CHI_001: [f64; 9] = [10.83, 13.82, 16.27, 18.47, 20.52, 22.46, 24.32, 26.12, 27.88];

fn measure(algo: Algo, p: &Playlist, runs: usize, uruns: usize, seed: u64) -> M {
    let c = case(p, seed);
    let n = c.artists.len();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for a in &c.artists {
        *counts.entry(a).or_default() += 1;
    }
    let maxc = *counts.values().max().unwrap();
    let forced = maxc.saturating_sub(n - maxc + 1);
    let window = ((n as f64 * 0.2).ceil() as usize).max(3).min(n);
    let mut m = M::default();
    let (mut gap_runs, mut spread_n) = (0usize, 0usize);
    for _ in 0..runs {
        let o = run(algo, &c, true, &mut rng);
        assert_eq!(o.len(), n);
        let pairs = o.windows(2).filter(|w| c.artists[w[0]] == c.artists[w[1]]).count();
        m.adj_rate += pairs as f64 / (n - 1) as f64;
        m.excess += (pairs - forced.min(pairs)) as f64;
        m.clumpy_runs += (pairs > forced) as u8 as f64;
        let mut last: HashMap<&str, usize> = HashMap::new();
        let mut mins: HashMap<&str, usize> = HashMap::new();
        let (mut gsum, mut gn, mut gmin) = (0.0, 0usize, usize::MAX);
        for (pos, &i) in o.iter().enumerate() {
            let a = c.artists[i].as_str();
            if let Some(prev) = last.insert(a, pos) {
                let d = pos - prev;
                gsum += d as f64;
                gn += 1;
                gmin = gmin.min(d);
                let e = mins.entry(a).or_insert(usize::MAX);
                *e = (*e).min(d);
            }
        }
        if gn > 0 {
            m.mean_gap += gsum / gn as f64;
            m.min_gap += gmin as f64;
            gap_runs += 1;
        }
        for (a, d) in mins {
            let ideal = n as f64 / counts[a] as f64;
            m.spread += (d as f64 / ideal).min(1.0);
            spread_n += 1;
        }
        let hits =
            o.iter().take(window).filter(|&&i| c.recent.contains_key(&format!("v{i}"))).count();
        m.recent_top += hits as f64 / window as f64;
    }
    let r = runs as f64;
    m.adj_rate /= r;
    m.excess /= r;
    m.clumpy_runs /= r;
    m.recent_top /= r;
    if gap_runs > 0 {
        m.mean_gap /= gap_runs as f64;
        m.min_gap /= gap_runs as f64;
    }
    if spread_n > 0 {
        m.spread /= spread_n as f64;
    }
    if uruns == 0 {
        return m;
    }
    // Uniformity: every track's position histogram (B buckets) against uniform, no history.
    let b = n.min(10);
    let mut hist = vec![vec![0u32; b]; n];
    for _ in 0..uruns {
        let o = run(algo, &c, false, &mut rng);
        for (pos, &i) in o.iter().enumerate() {
            hist[i][pos * b / n] += 1;
        }
    }
    let df = (b - 1) as f64;
    let (mut chi_sum, mut bad) = (0.0, 0usize);
    for h in &hist {
        let chi: f64 = (0..b)
            .map(|k| {
                // Buckets may differ in width by one slot.
                let lo = (k * n).div_ceil(b);
                let hi = ((k + 1) * n).div_ceil(b);
                let e = uruns as f64 * (hi - lo) as f64 / n as f64;
                (h[k] as f64 - e).powi(2) / e
            })
            .sum();
        chi_sum += chi / df;
        bad += (chi > CHI_001[b - 2]) as usize;
    }
    m.chi_df = chi_sum / n as f64;
    m.nonuniform = bad as f64 / n as f64;
    m
}

fn add(a: &mut M, b: &M) {
    a.adj_rate += b.adj_rate;
    a.excess += b.excess;
    a.clumpy_runs += b.clumpy_runs;
    a.min_gap += b.min_gap;
    a.mean_gap += b.mean_gap;
    a.spread += b.spread;
    a.recent_top += b.recent_top;
    a.chi_df += b.chi_df;
    a.nonuniform += b.nonuniform;
}

fn div(a: &mut M, n: f64) {
    for x in [
        &mut a.adj_rate,
        &mut a.excess,
        &mut a.clumpy_runs,
        &mut a.min_gap,
        &mut a.mean_gap,
        &mut a.spread,
        &mut a.recent_top,
        &mut a.chi_df,
        &mut a.nonuniform,
    ] {
        *x /= n;
    }
}

fn table(title: &str, rows: &[(Algo, M)]) {
    println!("\n### {title}\n");
    println!("| Algorithm | Same-artist neighbours | Avoidable pairs / shuffle | Shuffles with an avoidable pair | Min gap | Mean gap | Spread (min gap / ideal) | Recent in top 20 % | χ²/df | Tracks non-uniform (p<0.001) |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (a, m) in rows {
        println!(
            "| {} | {:.2} % | {:.3} | {:.1} % | {:.2} | {:.1} | {:.3} | {:.1} % | {:.2} | {:.1} % |",
            a.name(),
            m.adj_rate * 100.0,
            m.excess,
            m.clumpy_runs * 100.0,
            m.min_gap,
            m.mean_gap,
            m.spread,
            m.recent_top * 100.0,
            m.chi_df,
            m.nonuniform * 100.0
        );
    }
}

#[test]
#[ignore = "benchmark: prints the tables in docs/bench/shuffle.md (~1-3 min in debug)"]
fn shuffle_benchmark() {
    let ps = playlists();
    let runs: usize = std::env::var("BENCH_RUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let uruns: usize =
        std::env::var("BENCH_URUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    let mut per: Vec<(String, Vec<(Algo, M)>)> = Vec::new();
    for (pi, p) in ps.iter().enumerate() {
        let rows: Vec<(Algo, M)> =
            ALGOS.iter().map(|&a| (a, measure(a, p, runs, uruns, 1000 + pi as u64))).collect();
        per.push((p.name.clone(), rows));
    }
    let summary = |filter: &dyn Fn(usize) -> bool| -> Vec<(Algo, M)> {
        ALGOS
            .iter()
            .enumerate()
            .map(|(ai, &a)| {
                let mut s = M::default();
                let mut k = 0.0;
                for (pi, (_, rows)) in per.iter().enumerate() {
                    if filter(pi) {
                        add(&mut s, &rows[ai].1);
                        k += 1.0;
                    }
                }
                div(&mut s, k);
                (a, s)
            })
            .collect()
    };
    println!("runs={runs} uniformity_runs={uruns} playlists={}", ps.len());
    table("All playlists", &summary(&|_| true));
    table("Editorial playlists (27, real, low skew)", &summary(&|i| i < 27));
    table("Skewed libraries (12)", &summary(&|i| i >= 27));
    println!("\n### Per playlist: avoidable same-artist pairs per shuffle / non-uniform tracks\n");
    print!("| Playlist |");
    for a in ALGOS {
        print!(" {} |", a.name());
    }
    println!();
    println!("|---{}|", "|---".repeat(ALGOS.len()));
    for (name, rows) in &per {
        print!("| {name} |");
        for (_, m) in rows {
            print!(" {:.2} / {:.0} % |", m.excess, m.nonuniform * 100.0);
        }
        println!();
    }
}

/// The headline result, quick enough for every run: on skewed lists ours leaves no avoidable
/// same-artist pair where Fisher–Yates leaves several, and recent songs stay out of the top.
#[test]
fn beats_fisher_yates_on_skewed_playlists() {
    let ps = playlists();
    for p in
        ps.iter().filter(|p| ["Rap faves (skewed)", "Short skewed 30"].contains(&p.name.as_str()))
    {
        let ours = measure(Algo::Ours, p, 20, 0, 7);
        let fy = measure(Algo::FisherYates, p, 20, 0, 7);
        assert!(
            ours.excess < 0.2 && fy.excess > 1.0,
            "{}: {} vs {}",
            p.name,
            ours.excess,
            fy.excess
        );
        assert!(ours.recent_top < fy.recent_top, "{}", p.name);
    }
}
