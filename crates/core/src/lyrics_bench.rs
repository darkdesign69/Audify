//! Lyrics-selection benchmark (stream: bench2, docs/bench/lyrics.md): which of the 8 providers'
//! answers `fetch` shows for a labelled set of YouTube Music uploads (studio, live, remix, radio
//! edit, acoustic, sped-up, J/K/C-pop, Latin, French, Hindi, long-tail, instrumental, music
//! videos).
//!
//! `tests/fixtures/lyrics/lyrics_bench.json` holds, per track, the request the app sends, every
//! provider's captured answer (`answers`), and per answer a verdict from the scorer
//! (`docs/bench/lyrics-scripts/score.py`): right lyrics for this exact cut, synced, and the
//! median line-start error against the reference timing (YouTube Music's own timed lyrics of the
//! song, shifted onto the upload for music videos).
//!
//! - `fixture_bench` (default suite, offline, < 1 s): replays `Picker` (fetch's choice) over the
//!   captured answers in the default provider order and holds the benchmarked numbers.
//! - `live_capture` (`#[ignore]`): asks every provider live with the app's own code and rewrites
//!   the answers (`LYRICS_BENCH_TRACKS=<tracks.json>` to start from a new track list).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Lyrics, LyricsRequest, Picker, PROVIDERS};

const FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/lyrics/lyrics_bench.json");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Lyrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

impl Answer {
    fn result(&self) -> Result<Option<Lyrics>, String> {
        match &self.err {
            Some(e) => Err(e.clone()),
            None => Ok(self.ok.clone()),
        }
    }
}

/// The scorer's call on one answer (or on "none", showing nothing).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Verdict {
    /// The right lyrics for this cut (or, for "none", the track has none to show).
    pub correct: bool,
    /// Right and line-synced.
    #[serde(default)]
    pub synced: bool,
    /// Median |line start − reference| in ms, when both sides are synced.
    #[serde(default)]
    pub err_ms: Option<f64>,
    /// The right song, but another cut or version (timing or text off).
    #[serde(default)]
    pub wrong_version: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BenchTrack {
    pub video_id: String,
    pub title: String,
    pub artists: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// For a music video: the audio track whose timed lyrics are the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_video_id: Option<String>,
    #[serde(default)]
    pub answers: BTreeMap<String, Answer>,
    /// `ref_video_id`'s YouTube Music answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_youtube: Option<Answer>,
    /// LRCLIB `/api/get` alone (a contender).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lrclib_exact: Option<Answer>,
    #[serde(default)]
    pub verdict: BTreeMap<String, Verdict>,
    /// The track has synced lyrics somewhere (the synced-coverage denominator).
    #[serde(default)]
    pub synced_exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Fixture {
    pub tracks: Vec<BenchTrack>,
}

fn load() -> Fixture {
    serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap()
}

/// What `fetch` shows for `t` with providers asked in `order`: the provider id, or "none".
pub fn pick(t: &BenchTrack, order: &[&str], prefer_words: bool) -> String {
    pick_as(t, order, prefer_words, true)
}

/// [`pick`]; `title_rule` off replays the selection without `fetch`'s instrumental-title rule.
pub fn pick_as(t: &BenchTrack, order: &[&str], prefer_words: bool, title_rule: bool) -> String {
    if title_rule && super::instrumental_cut(&t.title) {
        return "none".to_owned(); // `fetch` answers "Instrumental" without asking anyone
    }
    let mut p = Picker::new(prefer_words);
    for id in order {
        let a = t.answers.get(*id).map(Answer::result).unwrap_or(Ok(None));
        if let Some(l) = p.offer(a.map(|o| o.map(|l| Lyrics { provider: id.to_string(), ..l }))) {
            return l.provider;
        }
    }
    p.finish().0.map_or_else(|| "none".to_owned(), |l| l.provider)
}

#[derive(Debug, Default)]
pub struct Tally {
    pub n: usize,
    pub correct: usize,
    pub synced_n: usize,
    pub synced: usize,
    pub wrong_version: usize,
    pub errs: Vec<f64>,
}

impl Tally {
    pub fn add(&mut self, t: &BenchTrack, v: Option<&Verdict>) {
        self.n += 1;
        let v = v.cloned().unwrap_or_default();
        self.correct += v.correct as usize;
        self.wrong_version += v.wrong_version as usize;
        if t.synced_exists {
            self.synced_n += 1;
            self.synced += v.synced as usize;
        }
        if let Some(e) = v.err_ms.filter(|_| v.correct && v.synced) {
            self.errs.push(e);
        }
    }
    pub fn correct_rate(&self) -> f64 {
        self.correct as f64 / self.n.max(1) as f64
    }
    pub fn synced_rate(&self) -> f64 {
        self.synced as f64 / self.synced_n.max(1) as f64
    }
    pub fn mean_err(&self) -> f64 {
        self.errs.iter().sum::<f64>() / self.errs.len().max(1) as f64
    }
    pub fn line(&self, name: &str) -> String {
        let mut e = self.errs.clone();
        e.sort_by(f64::total_cmp);
        let med = e.get(e.len() / 2).copied().unwrap_or(0.0);
        format!(
            "{name:<28} correct {:.3} ({}/{})  synced {:.3} ({}/{})  wrong-version {}  err mean {:.0} ms median {:.0} ms",
            self.correct_rate(),
            self.correct,
            self.n,
            self.synced_rate(),
            self.synced,
            self.synced_n,
            self.wrong_version,
            self.mean_err(),
            med
        )
    }
}

pub fn evaluate(fx: &Fixture, order: &[&str], prefer_words: bool, verbose: bool) -> Tally {
    let mut tally = Tally::default();
    for t in &fx.tracks {
        let id = pick(t, order, prefer_words);
        let v = t.verdict.get(&id);
        if verbose && !v.is_some_and(|v| v.correct) {
            println!("MISS {} — {} [{}] → {id}", t.title, t.artists, t.tags.join(","));
        }
        tally.add(t, v);
    }
    tally
}

#[test]
fn fixture_bench() {
    // The captured answers are full song lyrics, so the public source tree leaves them out
    // (scripts/publish-snapshot.sh); without them there is nothing to score.
    if !std::path::Path::new(FIXTURE).exists() {
        eprintln!("lyrics bench: no {FIXTURE}, skipped");
        return;
    }
    let fx = load();
    assert!(fx.tracks.len() >= 100, "the labelled set has {} tracks", fx.tracks.len());
    let verbose = std::env::var("LYRICS_BENCH_VERBOSE").is_ok();
    let ours = evaluate(&fx, &PROVIDERS, false, verbose);
    let mut before = Tally::default();
    for t in &fx.tracks {
        before.add(t, t.verdict.get(&pick_as(t, &BEFORE, false, false)));
    }
    println!("{}\n{}", ours.line("default order"), before.line("order before bench2"));
    println!("{}", evaluate(&fx, &PROVIDERS, true, false).line("default, prefer word-timed"));
    // The order change has to keep beating the old one on this set.
    assert!(ours.correct > before.correct, "{}", before.line("before"));
    assert!(ours.correct_rate() >= BENCH_CORRECT, "{}", ours.line("default order"));
    assert!(ours.synced_rate() >= BENCH_SYNCED, "{}", ours.line("default order"));
    assert!(ours.mean_err() <= BENCH_MEAN_ERR_MS, "{}", ours.line("default order"));
}

/// The default order before this benchmark.
const BEFORE: [&str; 8] =
    ["boidu", "unison", "lyricsplus", "lrclib", "youtube", "netease", "qq", "kugou"];

/// Floors from the run recorded in docs/bench/lyrics.md.
const BENCH_CORRECT: f64 = 0.98;
const BENCH_SYNCED: f64 = 0.98;
/// Against YouTube Music's own timing, so mostly a guard that YouTube stays first.
const BENCH_MEAN_ERR_MS: f64 = 150.0;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "asks every lyrics provider live (~10 requests per track, paced)"]
async fn live_capture() {
    use innertube::{Clients, InnerTube, Session};
    use std::time::Duration;
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let mut fx = match std::env::var("LYRICS_BENCH_TRACKS") {
        Ok(p) => {
            Fixture { tracks: serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() }
        }
        Err(_) => load(),
    };
    let only = std::env::var("LYRICS_BENCH_ONLY").ok();
    let skip_done = std::env::var("LYRICS_BENCH_RESUME").is_ok();
    let refill = std::env::var("LYRICS_BENCH_REFILL").is_ok();
    let out = std::env::var("LYRICS_BENCH_OUT").unwrap_or_else(|_| FIXTURE.to_owned());
    let budget = Duration::from_secs(12);
    let n = fx.tracks.len();
    for i in 0..n {
        let t = &mut fx.tracks[i];
        if only.as_deref().is_some_and(|o| !o.split(',').any(|v| v == t.video_id)) {
            continue;
        }
        if skip_done && !t.answers.is_empty() {
            continue;
        }
        // LYRICS_BENCH_REFILL: re-ask only the providers whose captured answer was an error
        // (rate limits, timeouts), keeping every other answer.
        let refill: Option<Vec<&str>> = refill.then(|| {
            PROVIDERS
                .iter()
                .copied()
                .filter(|id| t.answers.get(*id).is_none_or(|a| a.err.is_some()))
                .collect()
        });
        if refill.as_ref().is_some_and(|r| r.is_empty()) {
            continue;
        }
        let mut req = LyricsRequest {
            video_id: t.video_id.clone(),
            title: t.title.clone(),
            artists: t.artists.clone(),
            album: t.album.clone(),
            duration: t.duration,
            searched: false,
        };
        let next = super::resolve_with(&it, &clients, &mut req).await;
        if let Err(e) = &next {
            eprintln!("next {}: {e}", t.video_id);
        }
        t.album = req.album.clone();
        t.duration = req.duration;
        let mut answers = std::mem::take(&mut t.answers);
        for id in PROVIDERS {
            if refill.as_ref().is_some_and(|r| !r.contains(&id)) {
                continue;
            }
            let a = tokio::time::timeout(budget, super::ask_with(id, &it, &clients, &req, &next))
                .await
                .unwrap_or_else(|_| Err("timed out".into()));
            answers.insert(
                id.to_owned(),
                match a {
                    Ok(ok) => Answer { ok, err: None },
                    Err(e) => Answer { ok: None, err: Some(e) },
                },
            );
        }
        t.answers = answers;
        if refill.is_some() && t.lrclib_exact.as_ref().is_some_and(|a| a.err.is_none()) {
            eprintln!("[{}/{n}] refilled {}", i + 1, t.title);
            std::fs::write(&out, serde_json::to_string(&fx).unwrap()).ok();
            tokio::time::sleep(Duration::from_millis(1500)).await;
            continue;
        }
        let exact = super::lrclib_get(&req).await;
        t.lrclib_exact = Some(match exact {
            Ok(hit) => Answer { ok: hit.as_ref().and_then(super::lrclib_to_lyrics), err: None },
            Err(e) => Answer { ok: None, err: Some(e.to_string()) },
        });
        if let Some(rid) = t.ref_video_id.clone() {
            let mut rreq = LyricsRequest {
                video_id: rid,
                title: String::new(),
                artists: String::new(),
                album: None,
                duration: None,
                searched: false,
            };
            let rnext = super::resolve_with(&it, &clients, &mut rreq).await;
            let a = super::youtube_get(&it, &clients, &rnext).await;
            t.ref_youtube = Some(match a {
                Ok(ok) => Answer { ok, err: None },
                Err(e) => Answer { ok: None, err: Some(e) },
            });
        }
        let got: Vec<String> = t
            .answers
            .iter()
            .map(|(k, a)| {
                let s = match (&a.ok, &a.err) {
                    (Some(l), _) if l.synced => "S",
                    (Some(_), _) => "p",
                    (None, Some(_)) => "E",
                    _ => "-",
                };
                format!("{k}:{s}")
            })
            .collect();
        eprintln!("[{}/{n}] {} — {} | {}", i + 1, t.title, t.artists, got.join(" "));
        // Checkpoint, so a stopped run keeps what it has.
        std::fs::write(&out, serde_json::to_string(&fx).unwrap()).ok();
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    std::fs::write(&out, serde_json::to_string(&fx).unwrap()).unwrap();
}
