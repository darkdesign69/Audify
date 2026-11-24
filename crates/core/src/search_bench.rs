//! Search ranking benchmark (stream: bench2, docs/bench/search.md).
//!
//! `tests/fixtures/smart/search_bench.json` holds ~160 realistic queries, each labelled with what
//! Spotify's search shows for it (the Top result, and the first songs of the Songs shelf), plus the
//! YouTube Music answers to the same query captured with the app's own InnerTube client
//! (`search_all` and the song-filter `search`, twice, since YouTube reshuffles that one).
//!
//! - `fixture_bench` (default suite, offline, < 1 s): scores YouTube's raw order, our re-rank and
//!   the alternatives (popularity only, BM25, fuzzy, hybrids) by Top-1, MRR and NDCG@5 against the
//!   Spotify labels, and holds ours at the benchmarked numbers.
//! - `live_capture` (`#[ignore]`): refetches the YouTube Music answers (SEARCH_BENCH_QUERIES=<file>
//!   with `[{q, cat}]` for a new set).

use innertube::{BrowseItem, SearchResults, SongItem};
use serde::{Deserialize, Serialize};

const FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search_bench.json");

/// [`BrowseItem`] as stored in the fixture (its `kind` is a `&'static str`, so no `Deserialize`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub kind: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub play_count: Option<String>,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub duration: Option<String>,
}

impl Card {
    fn of(b: &BrowseItem) -> Card {
        Card {
            kind: b.kind.to_string(),
            id: b.id.clone(),
            title: b.title.clone(),
            subtitle: b.subtitle.clone(),
            play_count: b.play_count.clone(),
            is_video: b.is_video,
            album_id: b.album_id.clone(),
            duration: b.duration.clone(),
        }
    }
    pub fn item(&self) -> BrowseItem {
        let kind: &'static str = match self.kind.as_str() {
            "song" => "song",
            "album" => "album",
            "artist" => "artist",
            "playlist" => "playlist",
            "podcast" => "podcast",
            "episode" => "episode",
            _ => "other",
        };
        BrowseItem {
            kind,
            id: self.id.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            thumbnail: None,
            duration: self.duration.clone(),
            album_id: self.album_id.clone(),
            artist_runs: Vec::new(),
            play_count: self.play_count.clone(),
            is_video: self.is_video,
            is_upload: false,
            explicit: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Captured {
    pub top: Vec<Card>,
    pub songs: Vec<Card>,
    pub albums: Vec<Card>,
    pub artists: Vec<Card>,
    pub playlists: Vec<Card>,
}

impl Captured {
    pub fn of_results(r: &SearchResults) -> Captured {
        Self::of(r)
    }
    fn of(r: &SearchResults) -> Captured {
        let m = |v: &Vec<BrowseItem>| v.iter().map(Card::of).collect();
        Captured {
            top: m(&r.top),
            songs: m(&r.songs),
            albums: m(&r.albums),
            artists: m(&r.artists),
            playlists: m(&r.playlists),
        }
    }
    pub fn results(&self) -> SearchResults {
        let m = |v: &Vec<Card>| v.iter().map(Card::item).collect();
        SearchResults {
            top: m(&self.top),
            songs: m(&self.songs),
            albums: m(&self.albums),
            artists: m(&self.artists),
            playlists: m(&self.playlists),
        }
    }
}

/// What Spotify shows: `kind` is artist / song / album / playlist / podcast / episode.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Entity {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub artist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BenchQuery {
    pub q: String,
    pub cat: String,
    /// Spotify's Top result.
    #[serde(default)]
    pub top: Option<Entity>,
    /// Spotify's Songs shelf, in order.
    #[serde(default)]
    pub songs: Vec<Entity>,
    #[serde(default)]
    pub note: Option<String>,
    /// YouTube Music `search_all`.
    #[serde(default)]
    pub all: Captured,
    /// YouTube Music song-filter search, two captures (the order reshuffles between requests).
    #[serde(default)]
    pub ytm_songs: Vec<Vec<SongItem>>,
    /// ytmusicapi 1.12's own answer to the query (`search(q)` Top result, `search(q, "songs")`).
    #[serde(default)]
    pub ytmusicapi: Option<YtmusicapiAnswer>,
    // ── stream: podtop ──
    /// The app's podcast show search (YouTube Music podcasts + iTunes, merged as `podcast_search`
    /// merges them). `None` = not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pods: Option<Vec<crate::podcasts::Show>>,
    /// The YouTube Music artist page of the search's first artist card: its "Top songs" shelf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_top: Option<ArtistTop>,
    /// stream: search2 — `search::search_lyrics` for the query (captured only where
    /// `looks_like_lyrics` lets it run; `None` elsewhere, which the app treats as no matches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<Vec<crate::search::LyricMatch>>,
}

/// An artist page's "Top songs" shelf, in the page's order (stream: podtop).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtistTop {
    pub id: String,
    pub name: String,
    pub songs: Vec<SongItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct YtmusicapiAnswer {
    #[serde(default)]
    pub top: Option<Entity>,
    #[serde(default)]
    pub songs: Vec<SongItem>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Fixture {
    pub queries: Vec<BenchQuery>,
}

pub fn load() -> Fixture {
    serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("fixture")).expect("fixture json")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live YouTube Music (3 requests per query, paced)"]
async fn live_capture() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();
    let out = std::env::var("SEARCH_BENCH_OUT").unwrap_or_else(|_| FIXTURE.to_string());
    let mut fx = match std::env::var("SEARCH_BENCH_QUERIES") {
        Ok(p) => {
            Fixture { queries: serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() }
        }
        Err(_) => load(),
    };
    let ms =
        std::env::var("SEARCH_BENCH_PAUSE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1200);
    let pause = || tokio::time::sleep(std::time::Duration::from_millis(ms));
    // Pass 1: search_all + songs; pass 2: songs again (the reshuffle), so the two orders come
    // from requests minutes apart.
    for pass in 0..2 {
        let n = fx.queries.len();
        for i in 0..n {
            let bq = &mut fx.queries[i];
            if pass == 0 && (bq.all.top.len() + bq.all.songs.len()) == 0 {
                match it.search_all(c, &bq.q, false).await {
                    Ok(r) => bq.all = Captured::of(&r),
                    Err(e) => eprintln!("all {}: {e}", bq.q),
                }
                pause().await;
            }
            if bq.ytm_songs.len() <= pass {
                match it.search_songs(c, &bq.q, false).await {
                    Ok(r) => bq.ytm_songs.push(r.items),
                    Err(e) => eprintln!("songs {}: {e}", bq.q),
                }
                pause().await;
            }
            if i % 20 == 0 {
                eprintln!("pass {pass} {i}/{n}");
                std::fs::write(&out, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
            }
        }
    }
    std::fs::write(&out, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
}

/// stream: podtop. Adds the podcast show search (YouTube Music podcasts + iTunes, merged) and the
/// first artist card's artist-page "Top songs" to every query that lacks them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live YouTube Music and iTunes (paced)"]
async fn live_capture_podtop() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();
    let out = std::env::var("SEARCH_BENCH_OUT").unwrap_or_else(|_| FIXTURE.to_string());
    let mut fx = load();
    let ms =
        std::env::var("SEARCH_BENCH_PAUSE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3200);
    let pause = || tokio::time::sleep(std::time::Duration::from_millis(ms));
    let http = reqwest::Client::new();
    let n = fx.queries.len();
    for i in 0..n {
        let bq = &mut fx.queries[i];
        if bq.pods.is_none() {
            let yt = crate::podcasts::youtube::search_shows(&it, c, &bq.q).await;
            let url = format!(
                "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=25&country=US&term={}",
                urlencoding::encode(&bq.q)
            );
            let itn = match http.get(&url).send().await {
                Ok(r) if r.status().is_success() => r
                    .text()
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
                    .map(|v: serde_json::Value| crate::podcasts::itunes::parse_shows(&v)),
                Ok(r) => Err(format!("HTTP {}", r.status())),
                Err(e) => Err(e.to_string()),
            };
            match (yt, itn) {
                (Ok(y), Ok(d)) => bq.pods = Some(crate::podcasts::search::merge_shows(y, d)),
                (y, d) => eprintln!("pods {}: {:?} {:?}", bq.q, y.err(), d.err()),
            }
            pause().await;
        }
        if bq.artist_top.is_none() {
            let id = bq
                .all
                .top
                .iter()
                .chain(bq.all.artists.iter())
                .find(|c| c.kind == "artist" && c.id.starts_with("UC"))
                .map(|c| (c.id.clone(), c.title.clone()));
            if let Some((id, name)) = id {
                match it.artist(c, &id).await {
                    Ok(p) => bq.artist_top = Some(ArtistTop { id, name, songs: p.top_songs }),
                    Err(e) => eprintln!("artist {}: {e}", bq.q),
                }
            }
        }
        if i % 10 == 0 {
            eprintln!("{i}/{n}");
            std::fs::write(&out, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
        }
    }
    std::fs::write(&out, serde_json::to_string_pretty(&fx).unwrap()).unwrap();
}

// --- scoring -------------------------------------------------------------------------------------

use crate::identity::{self, norm, parse};
use std::collections::{HashMap, HashSet};

/// Spotify's song and a YouTube Music row are the same recording: same base title and version
/// tags, and Spotify's lead artist among the row's artists (or a label alias of it).
pub fn same_song(sp: &Entity, title: &str, artists: &str, aliases: &[(String, String)]) -> bool {
    let a = parse(&sp.title, &sp.artist, None);
    let b = parse(title, artists, None);
    // YouTube writes some titles in their own script with the romanisation after a dash
    // ("死ぬのがいいわ - Shinunoga E-Wa"); labels may alias a title ("Idol = アイドル").
    let sides: Vec<String> = title.split(" - ").map(|p| parse(p, artists, None).base).collect();
    let alias = aliases
        .iter()
        .any(|(x, y)| norm(x) == a.base && (norm(y) == b.base || sides.contains(&norm(y))));
    if a.base != b.base && dice(&a.base, &b.base) < 0.9 && !sides.contains(&a.base) && !alias {
        return false;
    }
    let tags = |p: &identity::Parsed| {
        let mut t: Vec<String> = p
            .tags
            .iter()
            .filter(|v| **v != identity::Version::Studio)
            .map(|v| format!("{v:?}"))
            .collect();
        t.sort();
        t.dedup();
        t
    };
    if tags(&a) != tags(&b) && !sides.contains(&a.base) {
        return false;
    }
    same_artist(&sp.artist, artists, aliases)
}

/// The lead artist of `sp_line` is one of `line`'s artists.
pub fn same_artist(sp_line: &str, line: &str, aliases: &[(String, String)]) -> bool {
    let lead = norm(&identity::primary_artist(sp_line));
    if lead.is_empty() {
        return true;
    }
    let names: Vec<String> = identity::artist_names(line).iter().map(|n| norm(n)).collect();
    let alias: Vec<String> = aliases
        .iter()
        .filter(|(a, _)| norm(a) == lead)
        .map(|(_, b)| norm(b))
        .chain([lead.clone()])
        .collect();
    names.iter().any(|n| alias.iter().any(|a| n == a || n.replace(' ', "") == a.replace(' ', "")))
        || alias.iter().any(|a| norm(line).contains(a.as_str()))
}

fn dice(a: &str, b: &str) -> f64 {
    let bi = |s: &str| -> Vec<(char, char)> {
        let c: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
        c.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (x, y) = (bi(a), bi(b));
    if x.is_empty() || y.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let mut pool = y.clone();
    let mut hit = 0;
    for p in &x {
        if let Some(i) = pool.iter().position(|q| q == p) {
            pool.swap_remove(i);
            hit += 1;
        }
    }
    2.0 * hit as f64 / (x.len() + y.len()) as f64
}

/// Label aliases: `note` may carry "alias: Spotify name = YouTube name; …".
fn aliases(bq: &BenchQuery) -> Vec<(String, String)> {
    bq.note
        .as_deref()
        .and_then(|n| n.split("alias:").nth(1))
        .map(|s| {
            s.split(';')
                .filter_map(|p| {
                    let (a, b) = p.split_once('=')?;
                    Some((a.trim().to_string(), b.trim().to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Which of Spotify's first five tracks each captured row is (by video id), computed once per query.
pub type Matches = HashMap<String, Vec<usize>>;

pub fn matches(bq: &BenchQuery) -> Matches {
    let al = aliases(bq);
    let mut m = Matches::new();
    let extra = bq
        .ytmusicapi
        .iter()
        .flat_map(|y| y.songs.iter())
        .chain(bq.artist_top.iter().flat_map(|a| a.songs.iter()));
    for s in bq.ytm_songs.iter().flatten().chain(extra) {
        m.entry(s.video_id.clone()).or_insert_with(|| {
            bq.songs
                .iter()
                .take(5)
                .enumerate()
                .filter(|(_, sp)| same_song(sp, &s.title, &s.artists, &al))
                .map(|(j, _)| j)
                .collect()
        });
    }
    m
}

/// Score one ranked songs list: (top-1, reciprocal rank of Spotify's first track, NDCG@5).
/// Relevance is graded: 5 for Spotify's first track, 4 for its second, …; one credit per Spotify
/// track. The ideal ranking holds the Spotify tracks YouTube has at all (no penalty for gaps in
/// YouTube's catalogue).
fn score_songs(m: &Matches, ranked: &[SongItem]) -> (f64, f64, f64) {
    let mut used = HashSet::new();
    let g: Vec<f64> = ranked
        .iter()
        .map(|s| {
            for &j in m.get(&s.video_id).map(Vec::as_slice).unwrap_or(&[]) {
                if used.insert(j) {
                    return (5 - j) as f64;
                }
            }
            0.0
        })
        .collect();
    let first = g.iter().position(|x| *x == 5.0);
    let rr = first.map_or(0.0, |i| 1.0 / (i + 1) as f64);
    let top1 = if first == Some(0) { 1.0 } else { 0.0 };
    let mut present: Vec<usize> =
        ranked.iter().filter_map(|s| m.get(&s.video_id)).flatten().copied().collect();
    present.sort();
    present.dedup();
    let ideal: Vec<f64> = present.iter().map(|j| (5 - j) as f64).collect();
    let dcg = |v: &[f64]| -> f64 {
        v.iter()
            .take(5)
            .enumerate()
            .map(|(i, g)| (2f64.powf(*g) - 1.0) / ((i + 2) as f64).log2())
            .sum()
    };
    let nd = if ideal.is_empty() { 0.0 } else { dcg(&g) / dcg(&ideal) };
    (top1, rr, nd)
}

#[derive(Default, Clone)]
pub struct Agg {
    pub n: usize,
    pub top1: f64,
    pub mrr: f64,
    pub ndcg: f64,
}

impl Agg {
    fn add(&mut self, top1: f64, rr: f64, nd: f64) {
        self.n += 1;
        self.top1 += top1;
        self.mrr += rr;
        self.ndcg += nd;
    }
    pub fn line(&self, name: &str) -> String {
        let n = self.n.max(1) as f64;
        format!(
            "| {name} | {} | {:.3} | {:.3} | {:.3} |",
            self.n,
            self.top1 / n,
            self.mrr / n,
            self.ndcg / n
        )
    }
    pub fn get(&self) -> (f64, f64, f64) {
        let n = self.n.max(1) as f64;
        (self.top1 / n, self.mrr / n, self.ndcg / n)
    }
}

// --- contenders: songs list ----------------------------------------------------------------------

pub type Ranker = fn(&str, &[SongItem], &Corpus) -> Vec<SongItem>;

/// Document frequencies over every captured row of every query (the catalogue a BM25 index such
/// as tantivy's would hold).
pub struct Corpus {
    df: HashMap<String, f64>,
    n: f64,
    avgdl: f64,
    /// stream: podtop. Query → its first artist card's page "Top songs".
    tops: HashMap<String, ArtistTop>,
    /// stream: search2. Query → its lyric matches.
    lyrics: HashMap<String, Vec<crate::search::LyricMatch>>,
    /// stream: search-pop. Query → YouTube's mixed search (re-ranked): the Top card.
    results: HashMap<String, SearchResults>,
}

fn doc(s: &SongItem) -> Vec<String> {
    norm(&format!("{} {}", s.title, s.artists)).split(' ').map(str::to_owned).collect()
}

impl Corpus {
    pub fn build(fx: &Fixture) -> Corpus {
        let mut df: HashMap<String, f64> = HashMap::new();
        let (mut n, mut len) = (0.0f64, 0.0f64);
        let mut seen = HashSet::new();
        for bq in &fx.queries {
            for s in bq.ytm_songs.iter().flatten() {
                if !seen.insert(s.video_id.clone()) {
                    continue;
                }
                let d = doc(s);
                len += d.len() as f64;
                n += 1.0;
                for t in d.into_iter().collect::<HashSet<_>>() {
                    *df.entry(t).or_default() += 1.0;
                }
            }
        }
        let tops =
            fx.queries.iter().filter_map(|b| Some((b.q.clone(), b.artist_top.clone()?))).collect();
        let lyrics =
            fx.queries.iter().filter_map(|b| Some((b.q.clone(), b.lyrics.clone()?))).collect();
        let results = fx.queries.iter().map(|b| (b.q.clone(), identity::rerank_search_all(&b.q, b.all.results()))).collect();
        Corpus { df, n: n.max(1.0), avgdl: len / n.max(1.0), tops, lyrics, results }
    }
    /// Okapi BM25 (k1 1.2, b 0.75: Lucene's and tantivy's defaults).
    pub fn bm25(&self, q: &str, s: &SongItem) -> f64 {
        let d = doc(s);
        let dl = d.len() as f64;
        norm(q)
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(|t| {
                let tf = d.iter().filter(|w| *w == t).count() as f64;
                if tf == 0.0 {
                    return 0.0;
                }
                let df = self.df.get(t).copied().unwrap_or(0.0);
                let idf = ((self.n - df + 0.5) / (df + 0.5) + 1.0).ln();
                idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * dl / self.avgdl))
            })
            .sum()
    }
}

fn plays(s: &SongItem) -> f64 {
    s.play_count.as_deref().map(identity::play_count).unwrap_or(0.0)
}

fn sort_by_score(items: &[SongItem], f: impl Fn(usize, &SongItem) -> f64) -> Vec<SongItem> {
    let mut v: Vec<(f64, usize, SongItem)> =
        items.iter().enumerate().map(|(i, s)| (f(i, s), i, s.clone())).collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|x| x.2).collect()
}

/// Fuzzy (uFuzzy-style) match: the best character-bigram Dice of the query against the title,
/// the artists, and "title artists" / "artists title".
pub fn fuzzy(q: &str, s: &SongItem) -> f64 {
    let qn = norm(q);
    let (t, a) = (norm(&s.title), norm(&s.artists));
    [dice(&qn, &t), dice(&qn, &a), dice(&qn, &format!("{t} {a}")), dice(&qn, &format!("{a} {t}"))]
        .into_iter()
        .fold(0.0, f64::max)
}

pub fn r_ytm(_: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    v.to_vec()
}
pub fn r_ours(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    identity::rerank_search_songs(q, v.to_vec())
}
/// Ours as fix-data shipped it (before this benchmark): a row got plays at full weight when its
/// artist name held every query word.
pub fn r_ours_before(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    identity::rerank_by(q, v.to_vec(), identity::Track::from_song, |s, q, t| {
        let plays = plays(s);
        if plays <= 0.0 {
            return 0.0;
        }
        let artist_query = !q.words.is_empty()
            && t.names().iter().any(|n| {
                let ws: HashSet<&str> = n.split(' ').filter(|w| !w.is_empty()).collect();
                q.words.iter().all(|w| ws.contains(w.as_str()))
            });
        (if artist_query { 1.0 } else { 0.08 }) * plays.log10()
    })
}
pub fn r_ours_noplays(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    identity::rerank(q, v.to_vec(), identity::Track::from_song)
}
pub fn r_pop(_: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    sort_by_score(v, |_, s| plays(s))
}
pub fn r_bm25(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    sort_by_score(v, |_, s| c.bm25(q, s))
}
pub fn r_fuzzy(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    sort_by_score(v, |_, s| fuzzy(q, s))
}
pub fn r_bm25_pop(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    sort_by_score(v, |_, s| c.bm25(q, s) + 0.5 * plays(s).max(1.0).log10())
}
pub fn r_fuzzy_pop(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    sort_by_score(v, |_, s| 4.0 * fuzzy(q, s) + 0.3 * plays(s).max(1.0).log10())
}

/// Text tier, then plays: rows whose title is the query (or the query is title + artist, either
/// order) first, then rows holding every query word, then the rest; plays order each tier.
pub fn r_tier_pop(q: &str, v: &[SongItem], _: &Corpus) -> Vec<SongItem> {
    let qn = norm(q);
    let qw: HashSet<String> = qn.split(' ').map(str::to_owned).collect();
    sort_by_score(v, |_, s| {
        let t = parse(&s.title, &s.artists, None).base;
        let a = norm(&s.artists);
        let tier = if t == qn
            || format!("{t} {a}").starts_with(&qn)
            || format!("{a} {t}").starts_with(&qn)
        {
            2.0
        } else if norm(&format!("{} {}", s.title, s.artists))
            .split(' ')
            .filter(|w| qw.contains(*w))
            .count()
            == qw.len()
        {
            1.0
        } else {
            0.0
        };
        tier * 100.0 + plays(s).max(1.0).log10()
    })
}

/// stream: podtop. Ours, then an artist query's songs blended with the artist page's "Top songs"
/// order (Borda: `identity::apply_artist_top_songs`, what the `search` command does).
///
/// stream: search-pop: through `search::songs_for`, as the app does, so the Top card's artist
/// counts as the query's artist ("billie ei" → Billie Eilish).
pub fn r_ours_top(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    let ranked = identity::rerank_search_songs(q, v.to_vec());
    let top = c.tops.get(q).map(|a| (a.name.clone(), a.songs.clone()));
    let empty = SearchResults::default();
    crate::search::songs_for(q, ranked, c.results.get(q).unwrap_or(&empty), top.as_ref())
}

/// The podtop list as it was before search-pop (no Top card evidence), for the comparison.
pub fn r_ours_top_before_pop(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    let ranked = identity::rerank_search_songs(q, v.to_vec());
    match c.tops.get(q) {
        Some(a) => identity::apply_artist_top_songs(q, ranked, &a.name, &a.songs),
        None => ranked,
    }
}

/// stream: search2. What the Songs list shows now: the podtop list with the lyric matches blended
/// in as the search page does (`search::blend_lyrics`, the user not yet interacting).
pub fn r_ours_lyrics(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    let songs = r_ours_top(q, v, c);
    match c.lyrics.get(q) {
        Some(l) => crate::search::blend_lyrics(q, songs, l.clone(), false, None).songs,
        None => songs,
    }
}

/// Contender: the shelf's songs first, in its order, then the rest of ours.
pub fn r_ours_shelf_first(q: &str, v: &[SongItem], c: &Corpus) -> Vec<SongItem> {
    let ranked = identity::rerank_search_songs(q, v.to_vec());
    let Some(a) = c.tops.get(q) else { return ranked };
    if identity::search_artist_intent(q, &ranked).as_deref() != Some(norm(&a.name).as_str()) {
        return ranked;
    }
    let mut rest: Vec<Option<SongItem>> = ranked.into_iter().map(Some).collect();
    let mut out = Vec::new();
    for t in &a.songs {
        let k = identity::work_key_song(t);
        if let Some(i) =
            rest.iter().position(|r| r.as_ref().is_some_and(|r| identity::work_key_song(r) == k))
        {
            out.push(rest[i].take().unwrap());
        } else if !out.iter().any(|o: &SongItem| identity::work_key_song(o) == k) {
            out.push(t.clone());
        }
    }
    out.extend(rest.into_iter().flatten());
    out
}

pub const RANKERS: &[(&str, Ranker)] = &[
    ("YouTube Music raw order", r_ytm),
    ("ours (rerank_search_songs)", r_ours),
    ("ours before this benchmark (fix-data)", r_ours_before),
    ("ours without plays (rerank)", r_ours_noplays),
    ("popularity only (plays)", r_pop),
    ("BM25 (tantivy defaults)", r_bm25),
    ("fuzzy (bigram Dice)", r_fuzzy),
    ("BM25 + log plays", r_bm25_pop),
    ("fuzzy + log plays", r_fuzzy_pop),
    ("text tier, then plays", r_tier_pop),
    ("ours + artist page Top songs, Borda (podtop)", r_ours_top),
    ("ours, artist page Top songs first", r_ours_shelf_first),
    ("ours + lyrics blend (search2)", r_ours_lyrics),
    ("ours podtop before search-pop (no Top card artist)", r_ours_top_before_pop),
];

/// Songs-list scores per ranker, over every capture of every labelled query; plus how often the
/// two captures of one query gave the same top 4 (stability).
pub fn eval_songs(
    fx: &Fixture,
    rankers: &[(&str, Ranker)],
    cat: Option<&str>,
) -> Vec<(String, Agg, f64)> {
    let c = Corpus::build(fx);
    let qs: Vec<(&BenchQuery, Matches)> = fx
        .queries
        .iter()
        .filter(|b| !b.songs.is_empty() && cat.map_or(true, |c| b.cat == c))
        .map(|b| (b, matches(b)))
        .collect();
    rankers
        .iter()
        .map(|(name, r)| {
            let mut agg = Agg::default();
            let (mut stable, mut pairs) = (0.0, 0.0);
            for (bq, m) in &qs {
                let caps: Vec<&Vec<SongItem>> =
                    bq.ytm_songs.iter().filter(|v| !v.is_empty()).collect();
                let mut tops = Vec::new();
                for v in &caps {
                    let ranked = r(&bq.q, v, &c);
                    let (t, rr, nd) = score_songs(m, &ranked);
                    agg.add(t / caps.len() as f64, rr / caps.len() as f64, nd / caps.len() as f64);
                    agg.n -= 1;
                    tops.push(
                        ranked.iter().take(1).map(|s| s.video_id.clone()).collect::<Vec<_>>(),
                    );
                }
                if !caps.is_empty() {
                    agg.n += 1;
                }
                if tops.len() == 2 {
                    pairs += 1.0;
                    stable += (tops[0] == tops[1]) as u8 as f64;
                }
            }
            (name.to_string(), agg, if pairs > 0.0 { stable / pairs } else { 0.0 })
        })
        .collect()
}

// --- contenders: Top result ----------------------------------------------------------------------

/// Spotify's Top result and one of our cards are the same entity.
pub fn same_entity(bq: &BenchQuery, sp: &Entity, c: &BrowseItem) -> bool {
    let al = aliases(bq);
    let podcast = c.id.starts_with("MPSP")
        || c.subtitle
            .as_deref()
            .is_some_and(|s| s.starts_with("Podcast") || s.starts_with("Episode"));
    let artists_of = |c: &BrowseItem| {
        // "Album • Adele • 2015" / "Song • Adele • 25" / "Adele • 3.1B plays": drop type and
        // trailing facts, keep the artist part.
        let s = c.subtitle.clone().unwrap_or_default();
        let parts: Vec<&str> = s.split(" • ").collect();
        match parts.as_slice() {
            [k, a, ..]
                if ["Song", "Video", "Album", "Single", "EP", "Playlist", "Podcast", "Episode"]
                    .contains(k) =>
            {
                a.to_string()
            }
            [a, ..] => a.to_string(),
            _ => String::new(),
        }
    };
    match sp.kind.as_str() {
        "artist" => {
            c.kind == "artist"
                && (norm(&c.title) == norm(&sp.title)
                    || al
                        .iter()
                        .any(|(a, b)| norm(a) == norm(&sp.title) && norm(b) == norm(&c.title)))
        }
        "song" => c.kind == "song" && !podcast && same_song(sp, &c.title, &artists_of(c), &al),
        "album" => {
            c.kind == "album"
                && (norm(&c.title) == norm(&sp.title)
                    || parse(&c.title, "", None).base == parse(&sp.title, "", None).base)
                && same_artist(&sp.artist, &artists_of(c), &al)
        }
        "playlist" => c.kind == "playlist" && !podcast,
        "podcast" | "episode" => podcast && dice(&norm(&c.title), &norm(&sp.title)) >= 0.6,
        _ => false,
    }
}

/// What the search page shows as the Top result (`routes/search/+page.svelte`).
pub fn ui_top(r: &SearchResults) -> Option<&BrowseItem> {
    r.top.first().or(r.artists.first()).or(r.songs.first())
}

pub type TopPicker = fn(&BenchQuery, &Corpus) -> Option<BrowseItem>;

pub fn t_ytm(bq: &BenchQuery, _: &Corpus) -> Option<BrowseItem> {
    ui_top(&bq.all.results()).cloned()
}
pub fn t_ours(bq: &BenchQuery, _: &Corpus) -> Option<BrowseItem> {
    let r = identity::rerank_search_all(&bq.q, bq.all.results());
    ui_top(&r).cloned()
}

/// ytmusicapi's own Top result, as a card.
pub fn t_ytmusicapi(bq: &BenchQuery, _: &Corpus) -> Option<BrowseItem> {
    let e = bq.ytmusicapi.as_ref()?.top.as_ref()?;
    let label = match e.kind.as_str() {
        "song" => "Song",
        "album" => "Album",
        "artist" => "Artist",
        "playlist" => "Playlist",
        "podcast" => "Podcast",
        "episode" => "Episode",
        _ => "",
    };
    // ytmusicapi puts an artist Top result's name in its artists, not its title.
    let title = if e.title.is_empty() { e.artist.clone() } else { e.title.clone() };
    let artist = if e.kind == "artist" { String::new() } else { e.artist.clone() };
    Card {
        kind: e.kind.clone(),
        title,
        subtitle: Some(if e.artist.is_empty() {
            label.to_string()
        } else {
            format!("{label} • {}", e.artist)
        }),
        ..Card::default()
    }
    .item()
    .into()
}

/// stream: podtop. Ours, then a show as the Top result when `podcasts::search::top_show` says so
/// (the search page does the same with `podcast_search`'s shows).
pub fn t_ours_pod(bq: &BenchQuery, _: &Corpus) -> Option<BrowseItem> {
    let r = identity::rerank_search_all(&bq.q, bq.all.results());
    let shows = bq.pods.as_deref().unwrap_or(&[]);
    if let Some(i) =
        crate::podcasts::search::top_show(&bq.q, r.top.first(), r.artists.first(), shows)
    {
        return Some(show_card(&shows[i]));
    }
    ui_top(&r).cloned()
}

/// A show as the search page's Top result card (`showCard` in `routes/search/+page.svelte`).
pub fn show_card(s: &crate::podcasts::Show) -> BrowseItem {
    Card {
        kind: "podcast".into(),
        id: s.id.clone(),
        title: s.title.clone(),
        subtitle: Some(format!("Podcast • {}", s.author)),
        ..Card::default()
    }
    .item()
}

/// stream: search2. Ours with podcasts, then a lyric lead as the search page lifts it.
pub fn t_ours_lyrics(bq: &BenchQuery, c: &Corpus) -> Option<BrowseItem> {
    let r = identity::rerank_search_all(&bq.q, bq.all.results());
    let shows = bq.pods.as_deref().unwrap_or(&[]);
    if crate::podcasts::search::top_show(&bq.q, r.top.first(), r.artists.first(), shows).is_some() {
        return t_ours_pod(bq, c);
    }
    if let (Some(l), Some(v)) = (c.lyrics.get(&bq.q), bq.ytm_songs.iter().find(|v| !v.is_empty())) {
        let card = r.top.first().filter(|t| t.kind != "song").map(|t| t.title.as_str());
        let b = crate::search::blend_lyrics(&bq.q, r_ours_top(&bq.q, v, c), l.clone(), false, card);
        if b.lead_top {
            let s = &b.songs[0];
            return Some(Card { kind: "song".into(), id: s.video_id.clone(), title: s.title.clone(), subtitle: Some(format!("Song • {}", s.artists)), ..Card::default() }.item());
        }
    }
    t_ours_pod(bq, c)
}

pub const TOP_PICKERS: &[(&str, TopPicker)] = &[
    ("YouTube Music top card", t_ytm),
    ("ours before podtop (rerank_search_all)", t_ours),
    ("ours (rerank_search_all + podcast top_show)", t_ours_pod),
    ("ours + lyric lead (search2)", t_ours_lyrics),
];

/// Songs lists on the queries ytmusicapi answered: YouTube's order (first capture), ours over it,
/// and ytmusicapi's own song search.
pub fn eval_ytmusicapi(fx: &Fixture) -> Vec<(String, Agg)> {
    let mut out = vec![
        ("YouTube Music raw order".to_string(), Agg::default()),
        ("ours (rerank_search_songs)".to_string(), Agg::default()),
        ("ytmusicapi search(filter=\"songs\")".to_string(), Agg::default()),
    ];
    for bq in fx.queries.iter().filter(|b| !b.songs.is_empty()) {
        let (Some(y), Some(v)) =
            (bq.ytmusicapi.as_ref(), bq.ytm_songs.iter().find(|v| !v.is_empty()))
        else {
            continue;
        };
        if y.songs.is_empty() {
            continue;
        }
        let m = matches(bq);
        for (i, list) in
            [v.clone(), identity::rerank_search_songs(&bq.q, v.clone()), y.songs.clone()]
                .iter()
                .enumerate()
        {
            let (a, b, c) = score_songs(&m, list);
            out[i].1.add(a, b, c);
        }
    }
    out
}

pub fn eval_top(
    fx: &Fixture,
    pickers: &[(&str, TopPicker)],
    cat: Option<&str>,
) -> Vec<(String, Agg)> {
    let c = Corpus::build(fx);
    pickers
        .iter()
        .map(|(name, p)| {
            let mut agg = Agg::default();
            for bq in fx.queries.iter().filter(|b| cat.map_or(true, |c| b.cat == c)) {
                let Some(sp) = &bq.top else { continue };
                // Spotify answers mood words with a genre page ("chill"), which has no
                // counterpart here.
                if sp.kind == "other" {
                    continue;
                }
                if bq.all.top.is_empty() && bq.all.songs.is_empty() && bq.all.artists.is_empty() {
                    continue;
                }
                let ok = p(bq, &c).is_some_and(|t| same_entity(bq, sp, &t));
                agg.add(ok as u8 as f64, ok as u8 as f64, ok as u8 as f64);
            }
            (name.to_string(), agg)
        })
        .collect()
}

/// Floors from the run recorded in docs/bench/search.md (raised by stream podtop: the Songs list
/// with the artist page's Top songs blended in, the Top result with shows).
const BENCH_SONGS_MRR: f64 = 0.75;
const BENCH_SONGS_NDCG: f64 = 0.80;
const BENCH_TOP1: f64 = 0.87;
/// Top result on the podcast queries ("joe rogan", "the daily", …): 8/8 in the podtop run.
const BENCH_TOP1_PODCAST: f64 = 0.85;

#[test]
fn fixture_bench() {
    let fx = load();
    let verbose = std::env::var("SEARCH_BENCH_VERBOSE").is_ok();
    println!("| songs ranker | queries | top-1 | MRR | NDCG@5 | same first row in both captures |");
    let songs = eval_songs(&fx, RANKERS, None);
    for (name, a, st) in &songs {
        println!("{} {:.2} |", a.line(name), st);
    }
    let (_, raw_mrr, raw_ndcg) = songs[0].1.get();
    // What the `search` command ships: rerank_search_songs + the artist page's Top songs (podtop).
    let shipped = songs.iter().find(|(n, _, _)| n.contains("(podtop)")).unwrap();
    let (_, mrr, ndcg) = shipped.1.get();
    let (_, pre_mrr, pre_ndcg) = songs[1].1.get();
    assert!(
        mrr >= BENCH_SONGS_MRR && ndcg >= BENCH_SONGS_NDCG,
        "songs MRR {mrr:.3} NDCG@5 {ndcg:.3}"
    );
    assert!(mrr > raw_mrr && ndcg > raw_ndcg, "the re-rank must beat YouTube's own order");
    assert!(
        mrr >= pre_mrr && ndcg >= pre_ndcg,
        "the Top songs blend must not lose to the re-rank alone"
    );
    println!("| songs, queries ytmusicapi answered | queries | top-1 | MRR | NDCG@5 |");
    for (name, a) in eval_ytmusicapi(&fx) {
        println!("{}", a.line(&name));
    }
    println!("| top result, queries ytmusicapi answered | queries | top-1 | | |");
    let sub = Fixture {
        queries: fx.queries.iter().filter(|b| b.ytmusicapi.is_some()).cloned().collect(),
    };
    for (name, a) in eval_top(
        &sub,
        &[
            ("YouTube Music top card", t_ytm),
            ("ytmusicapi top result", t_ytmusicapi),
            ("ours", t_ours_pod),
        ],
        None,
    ) {
        println!("{}", a.line(&name));
    }
    println!("| top result | queries | top-1 | | |");
    let top = eval_top(&fx, TOP_PICKERS, None);
    for (name, a) in &top {
        println!("{}", a.line(name));
    }
    let ours = top.iter().find(|(n, _)| n.contains("top_show")).unwrap().1.get().0;
    assert!(ours >= BENCH_TOP1, "Top result top-1 {ours:.3}");
    // stream: search2 — with lyrics on, nothing may get worse.
    let lyr = top.iter().find(|(n, _)| n.contains("lyric lead")).unwrap().1.get().0;
    assert!(lyr >= ours, "lyric lead lost Top result: {lyr:.3} < {ours:.3}");
    let lyr_songs = songs.iter().find(|(n, _, _)| n.contains("lyrics blend")).unwrap().1.get();
    assert!(lyr_songs.1 >= mrr && lyr_songs.2 >= ndcg, "lyrics blend lost on songs: {lyr_songs:?}");
    let pods = eval_top(&fx, &[("ours", t_ours_pod)], Some("podcast"));
    let (n, p) = (pods[0].1.n, pods[0].1.get().0);
    println!("| top result, podcast queries | {n} | {p:.3} | | |");
    assert!(n >= 8 && p >= BENCH_TOP1_PODCAST, "podcast Top result {p:.3} over {n}");
    if verbose {
        let cats: Vec<String> = {
            let mut c: Vec<String> = fx.queries.iter().map(|b| b.cat.clone()).collect();
            c.dedup();
            c
        };
        for cat in &cats {
            println!("## {cat}");
            for (name, a, _) in eval_songs(&fx, RANKERS, Some(cat)) {
                println!("{}", a.line(&format!("songs {name}")));
            }
            for (name, a) in eval_top(&fx, TOP_PICKERS, Some(cat)) {
                println!("{}", a.line(&format!("top {name}")));
            }
        }
    }
}

/// `SEARCH_BENCH_DIFF=<ranker index a>,<b>`: queries where ranker a's top-1 differs from b's.
#[test]
fn diff_rankers() {
    let Ok(spec) = std::env::var("SEARCH_BENCH_DIFF") else { return };
    let (a, b) = spec.split_once(',').unwrap();
    let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
    let fx = load();
    let c = Corpus::build(&fx);
    for bq in fx.queries.iter().filter(|b| !b.songs.is_empty()) {
        for v in bq.ytm_songs.iter().filter(|v| !v.is_empty()) {
            let ra = (RANKERS[a].1)(&bq.q, v, &c);
            let rb = (RANKERS[b].1)(&bq.q, v, &c);
            let m = matches(bq);
            let (ta, _, _) = score_songs(&m, &ra);
            let (tb, _, _) = score_songs(&m, &rb);
            if ta != tb {
                let show = |r: &[SongItem]| {
                    r.iter()
                        .take(3)
                        .map(|s| {
                            format!(
                                "{} / {} [{}]",
                                s.title,
                                s.artists,
                                s.play_count.clone().unwrap_or_default()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" | ")
                };
                println!(
                    "[{}] {} sp1={} / {}\n   A{ta}: {}\n   B{tb}: {}",
                    bq.cat,
                    bq.q,
                    bq.songs[0].title,
                    bq.songs[0].artist,
                    show(&ra),
                    show(&rb)
                );
            }
        }
    }
}

/// `SEARCH_BENCH_GRID=1`: the plays weights of `rerank_search_songs` over a grid.
#[test]
fn grid_weights() {
    if std::env::var("SEARCH_BENCH_GRID").is_err() {
        return;
    }
    let fx = load();
    for aw in [0.0, 0.2, 0.3, 0.5, 0.7, 1.0, 2.0] {
        for ow in [0.0, 0.04, 0.08, 0.15, 0.3] {
            let mut agg = Agg::default();
            for bq in fx.queries.iter().filter(|b| !b.songs.is_empty()) {
                let m = matches(bq);
                let caps: Vec<&Vec<SongItem>> =
                    bq.ytm_songs.iter().filter(|v| !v.is_empty()).collect();
                if caps.is_empty() {
                    continue;
                }
                let (mut t, mut r, mut n) = (0.0, 0.0, 0.0);
                for v in &caps {
                    let ranked = identity::rerank_search_songs_with(&bq.q, v.to_vec(), aw, ow);
                    let (a, b, c) = score_songs(&m, &ranked);
                    t += a / caps.len() as f64;
                    r += b / caps.len() as f64;
                    n += c / caps.len() as f64;
                }
                agg.add(t, r, n);
            }
            println!("{}", agg.line(&format!("artist {aw} other {ow}")));
        }
    }
}

/// `SEARCH_BENCH_TOP_MISSES=1`: Top-result misses of ours.
#[test]
fn top_misses() {
    if std::env::var("SEARCH_BENCH_TOP_MISSES").is_err() {
        return;
    }
    let fx = load();
    let c = Corpus::build(&fx);
    for bq in &fx.queries {
        let Some(sp) = &bq.top else { continue };
        if bq.all.top.is_empty() && bq.all.songs.is_empty() && bq.all.artists.is_empty() {
            continue;
        }
        let t = t_ours_pod(bq, &c);
        if t.as_ref().is_some_and(|t| t.kind == "podcast") {
            println!("podcast top: {} -> {:?}", bq.q, t.as_ref().map(|t| &t.title));
        }
        if !t.as_ref().is_some_and(|t| same_entity(bq, sp, t)) {
            let r = bq.all.results();
            println!(
                "[{}] {} | sp {} {} / {} | ours {:?} | top {:?} | artists {:?}",
                bq.cat,
                bq.q,
                sp.kind,
                sp.title,
                sp.artist,
                t.map(|t| format!("{} {} / {}", t.kind, t.title, t.subtitle.unwrap_or_default())),
                r.top.iter().take(3).map(|t| format!("{} {}", t.kind, t.title)).collect::<Vec<_>>(),
                r.artists.iter().take(2).map(|t| t.title.clone()).collect::<Vec<_>>()
            );
        }
    }
}
