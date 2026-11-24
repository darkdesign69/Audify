//! Matcher benchmark (stream: bench, docs/bench/matcher.md): the Spotify → YouTube Music import
//! matcher scored on a hand-labelled set of hard cases (remasters, feat., live/acoustic/remix,
//! cover and karaoke traps, non-Latin and regional titles, classical, same-title songs, deluxe
//! editions, Taylor's Versions).
//!
//! `tests/fixtures/smart/matcher_bench.json` holds, per Spotify track, the labels (`correct`: the
//! YouTube Music audio tracks of the same recording; `ok_video`: the official video of the same
//! version, accepted when no audio track exists; `wrong_version`: the same song in another
//! version) and the candidates YouTube Music answered to the import's own query (`songs`, then
//! `videos`), captured with the app's InnerTube client.
//!
//! - `fixture_bench` (default suite, offline, < 1 s): runs the import pipeline (`import::query`,
//!   `import::score`, `identity::sort_ranked`, `identity::tier`) over the captured candidates and
//!   holds precision/recall at or above the benchmarked numbers.
//! - `live_bench` (`#[ignore]`): the same against live YouTube Music; with `MATCHER_BENCH_CAPTURE`
//!   set it rewrites the fixture's candidates from the live answers.

use crate::identity::{self, MatchTier, Score};
use crate::spotify::SourceTrack;
use innertube::SongItem;
use serde::{Deserialize, Serialize};

const FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/matcher_bench.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchTrack {
    pub sid: String,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    pub duration_ms: u64,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub correct: Vec<String>,
    #[serde(default)]
    pub ok_video: Vec<String>,
    #[serde(default)]
    pub wrong_version: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Left out of the scores: no unambiguous answer could be labelled (see `note`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude: bool,
    #[serde(default)]
    pub songs: Vec<SongItem>,
    #[serde(default)]
    pub videos: Vec<SongItem>,
}

impl BenchTrack {
    fn source(&self) -> SourceTrack {
        SourceTrack {
            id: Some(self.sid.clone()),
            title: self.title.clone(),
            artists: self.artists.clone(),
            album: self.album.clone(),
            duration_ms: Some(self.duration_ms),
            explicit: Some(self.explicit),
        }
    }

    /// Whether YouTube Music has the track at all (else the right answer is "not found").
    fn exists(&self) -> bool {
        !self.correct.is_empty() || !self.ok_video.is_empty()
    }

    fn is_correct(&self, id: &str) -> bool {
        self.correct.iter().any(|c| c == id) || self.ok_video.iter().any(|c| c == id)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub tracks: Vec<BenchTrack>,
}

pub fn load() -> Fixture {
    let path = std::env::var("MATCHER_BENCH_FIXTURE").unwrap_or_else(|_| FIXTURE.to_owned());
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The import pipeline (`import::search` + `import::classify`) over already-fetched answers:
/// songs first, videos only when no song is even a plausible guess.
pub fn run(t: &BenchTrack) -> (MatchTier, Option<SongItem>) {
    let src = t.source();
    let mut out: Vec<(Score, SongItem)> = Vec::new();
    let mut rank = |items: &[SongItem], out: &mut Vec<(Score, SongItem)>| {
        for (i, c) in items.iter().enumerate() {
            if c.video_id.is_empty() || out.iter().any(|(_, o)| o.video_id == c.video_id) {
                continue;
            }
            out.push((crate::import::score(&src, c, i), c.clone()));
        }
    };
    rank(&t.songs, &mut out);
    if out.iter().all(|(s, _)| s.value < identity::CHECK) {
        rank(&t.videos, &mut out);
    }
    identity::sort_ranked(&mut out);
    out.truncate(6);
    if std::env::var("MATCHER_BENCH_DEBUG").is_ok_and(|d| d.split(',').any(|x| x == t.sid)) {
        for (s, c) in &out {
            println!(
                "  {:.3} rank {:.3} {:?} {} | {} | {:?} | {:?}",
                s.value, s.rank, s.gates, c.video_id, c.title, c.artists, c.duration
            );
        }
    }
    let tier = identity::tier(&out.iter().map(|(s, _)| s).collect::<Vec<_>>());
    let pick = (tier != MatchTier::Missing).then(|| out[0].1.clone());
    (tier, pick)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Metrics {
    pub n: usize,
    pub exists: usize,
    pub answered: usize,
    pub correct: usize,
    pub wrong_version: usize,
}

impl Metrics {
    fn add(&mut self, t: &BenchTrack, pick: Option<&str>) {
        self.n += 1;
        self.exists += t.exists() as usize;
        if let Some(id) = pick {
            self.answered += 1;
            if t.is_correct(id) {
                self.correct += 1;
            } else if t.wrong_version.iter().any(|w| w == id) {
                self.wrong_version += 1;
            }
        }
    }
    pub fn precision(&self) -> f64 {
        self.correct as f64 / self.answered.max(1) as f64
    }
    pub fn recall(&self) -> f64 {
        self.correct as f64 / self.exists.max(1) as f64
    }
    pub fn f1(&self) -> f64 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
    fn line(&self, name: &str) -> String {
        format!(
            "{name:<10} n={} answered={} correct={} P={:.3} R={:.3} F1={:.3} wrong-version={:.3}",
            self.n,
            self.answered,
            self.correct,
            self.precision(),
            self.recall(),
            self.f1(),
            self.wrong_version as f64 / self.answered.max(1) as f64
        )
    }
}

/// `(auto, top1)`: auto counts only "Matched" rows as answered (what the import accepts without
/// review); top1 also counts "To check" rows, whose pick is pre-selected in the review list.
pub fn evaluate(fx: &Fixture, verbose: bool) -> (Metrics, Metrics) {
    let (mut auto, mut top1) = (Metrics::default(), Metrics::default());
    let mut picks = serde_json::Map::new();
    for t in fx.tracks.iter().filter(|t| !t.exclude) {
        let (tier, pick) = run(t);
        let id = pick.as_ref().map(|p| p.video_id.as_str());
        top1.add(t, id);
        auto.add(t, if tier == MatchTier::Matched { id } else { None });
        picks.insert(t.sid.clone(), serde_json::json!({ "tier": format!("{tier:?}"), "pick": id }));
        if verbose && id.map_or(t.exists(), |i| !t.is_correct(i)) {
            let p = pick.as_ref();
            println!(
                "MISS {:?} {} — {} | got {:?} {:?} / {:?}",
                tier,
                t.title,
                t.artists.join(", "),
                p.map(|p| &p.video_id),
                p.map(|p| &p.title),
                p.map(|p| &p.artists),
            );
        }
    }
    if let Ok(path) = std::env::var("MATCHER_BENCH_PICKS") {
        std::fs::write(path, serde_json::to_string_pretty(&picks).unwrap()).unwrap();
    }
    (auto, top1)
}

#[test]
fn fixture_bench() {
    let fx = load();
    assert!(fx.tracks.len() >= 150, "the labelled set has {} tracks", fx.tracks.len());
    let (auto, top1) = evaluate(&fx, std::env::var("MATCHER_BENCH_VERBOSE").is_ok());
    println!("{}\n{}", auto.line("auto"), top1.line("top1"));
    // The benchmarked numbers (docs/bench/matcher.md), with a little slack for float noise.
    assert!(auto.precision() >= BENCH_AUTO_PRECISION, "{}", auto.line("auto"));
    assert!(top1.f1() >= BENCH_TOP1_F1, "{}", top1.line("top1"));
}

/// Floors from the run recorded in docs/bench/matcher.md.
const BENCH_AUTO_PRECISION: f64 = 0.98;
const BENCH_TOP1_F1: f64 = 0.96;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live YouTube Music (~2 requests per track, paced)"]
async fn live_bench() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();
    let mut fx = load();
    // With MATCHER_BENCH_CAPTURE only tracks with no captured answers are fetched (new labels);
    // MATCHER_BENCH_REFRESH refetches everything.
    let only_new = std::env::var("MATCHER_BENCH_CAPTURE").is_ok()
        && std::env::var("MATCHER_BENCH_REFRESH").is_err();
    for t in &mut fx.tracks {
        if only_new && !t.songs.is_empty() {
            continue;
        }
        let q = crate::import::query(&t.source());
        match it.search_songs(c, &q, false).await {
            Ok(r) => t.songs = r.items,
            Err(e) => eprintln!("songs {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        match it.search_videos(c, &q).await {
            Ok(r) => t.videos = r.items,
            Err(e) => eprintln!("videos {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    }
    let (auto, top1) = evaluate(&fx, true);
    println!("{}\n{}", auto.line("auto"), top1.line("top1"));
    if std::env::var("MATCHER_BENCH_CAPTURE").is_ok() {
        std::fs::write(FIXTURE, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
    }
}

/// The live-import misses that started this (docs/streams/import2-e2e-report.tsv).
#[test]
fn version_tails_name_other_recordings() {
    use identity::{parse, Version};
    for (title, v) in [
        ("Ordinary (Wedding Version)", Version::Alternate),
        ("Ordinary - Wedding Version", Version::Alternate),
        ("Never - Tik Tok Version", Version::Alternate),
        ("Sola (English Version)", Version::Alternate),
        ("Sunn Raha Hai (Female Version)", Version::Alternate),
        ("Let's Hurt Tonight - Stripped Version", Version::Acoustic),
        ("Lights - Sped Up Version", Version::SpedUp),
        ("Animals - Speed", Version::SpedUp),
        ("Fantasy - Single Version", Version::Studio),
        ("Time - 2011 Remastered Version", Version::Studio),
        ("Song (Album Version)", Version::Studio),
        ("Song (Radio Version)", Version::Studio),
        ("Song (Explicit Version)", Version::Studio),
    ] {
        assert_eq!(parse(title, "", None).version, v, "{title}");
    }
    // "Goodbye Lullaby" is an Avril Lavigne album, not a lullaby-cover mill.
    assert_eq!(parse("Smile", "Avril Lavigne", Some("Goodbye Lullaby")).version, Version::Studio);
    assert_eq!(
        parse("Smile", "Rockabye Baby!", Some("Lullaby Renditions of Avril Lavigne")).version,
        Version::Cover
    );
}
