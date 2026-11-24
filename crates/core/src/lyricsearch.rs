//! Search by lyrics: a fragment of a song's words finds the song, as on Spotify.
//!
//! Benchmarked in `docs/bench/lyricsearch.md` (stream: lyricsearch). No lyric text is stored,
//! shipped or logged anywhere here: the query goes to services that index lyrics and only titles
//! and artists come back.
//!
//! How it works:
//! 1. **Sources, in parallel** under one hard deadline ([`DEADLINE`]): Apple's (iTunes Search API)
//!    and Deezer's public catalogue search, which match lyric text as well as titles, and YouTube
//!    Music's own song search on the raw words. None needs an account or a key. Genius scored best
//!    alone but answers this client with 403 (bot protection) and its terms forbid scraping, so it
//!    is not used; see the benchmark.
//! 2. **Fusion**: each source's rows vote for a work (title + primary artist) by reciprocal rank
//!    ([`fuse`]); two sources agreeing beat one source's first row. Any source may fail or be
//!    rate-limited (iTunes allows about 20 searches a minute); the others carry on.
//! 3. **Resolution** to a playable YouTube Music song: a work that YouTube Music's own answer
//!    already holds is taken from it for free; the best remaining works are looked up with a
//!    title + artist song search and scored by [`identity::score`], the importer's matcher (same
//!    gates: version, artist, title, length).
//!
//! The search page calls [`looks_like_lyrics`] first: it is cheap and keeps the extra requests off
//! ordinary title and artist searches. The lyric search never blocks the normal one; see
//! `docs/streams/lyricsearch.md` for how its rows are blended in.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use innertube::{InnerTube, SongItem, YouTubeClient};
use serde::Serialize;
use serde_json::Value;

use crate::identity::{self, norm, Gate, Track};

/// Everything, the YouTube Music resolution included, is over by then.
pub const DEADLINE: Duration = Duration::from_millis(1200);
/// Title + artist searches per lyric search, at most.
const MAX_LOOKUPS: usize = 4;
/// How many of each source's rows vote.
const DEPTH: usize = 10;
/// How many of the fused leaders are resolved.
const RESOLVE: usize = 3;
/// The reciprocal-rank constant: the first row votes 1/K, the second 1/(K+1).
const RRF_K: f64 = 2.0;

/// A lyric source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Deezer,
    Itunes,
    /// YouTube Music's own song search on the lyric words.
    Ytm,
}

impl Source {
    /// Fusion weight. Grid-searched in the benchmark: equal weights are on the plateau.
    fn weight(self) -> f64 {
        1.0
    }
}

/// A song a source says the words belong to.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Seconds, when the source knows.
    pub duration: Option<f64>,
}

/// A fused work: one song, whatever source named it.
#[derive(Debug, Clone)]
pub struct Work {
    pub hit: Hit,
    pub score: f64,
    pub sources: Vec<Source>,
}

/// A lyric match, as a playable YouTube Music song.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub song: SongItem,
    /// The fused vote, 0..1.5 (three sources' first rows): from [`STRONG`] up, at least two
    /// sources put the song at or near the top.
    pub score: f64,
    pub sources: Vec<Source>,
}

/// A fused score at which a lyric match should lead the results when the title match is weak.
/// Benchmark: a fused leader at 1.0 or more is right 87% of the time (56% of lyric queries get
/// one); below it, 60%.
pub const STRONG: f64 = 1.0;

// --- the gate ----------------------------------------------------------------------------------

/// Personal pronouns: a lyric line is nearly always someone talking to someone. Titles and
/// artist names mostly aren't.
const PRONOUNS: &[&str] = &[
    // en
    "i", "you", "me", "my", "your", "we", "our", "us", "he", "him", "his", "she", "her", "they",
    "them", "im", "youre", "ill", "ive", "id", "ya", "u",
    // es / pt / it
    "yo", "tu", "te", "mi", "nos", "ti", "eu", "voce", "io", "noi", "lui", "lei", "ele", "ela",
    // fr / de
    "je", "j", "moi", "toi", "nous", "vous", "elle", "ich", "du", "mich", "dich", "mir", "dir",
    "wir", "ihr",
];

/// Other words that carry the grammar of a sentence.
const FUNCTION_WORDS: &[&str] = &[
    // en
    "it", "the", "a", "an", "and", "but", "or", "so", "to", "of", "in", "on", "at", "for", "with",
    "from", "is", "are", "was", "be", "been", "am", "do", "dont", "did", "have", "had", "will",
    "would", "can", "cant", "could", "if", "when", "that", "this", "what", "all", "no", "not",
    "just", "like", "up", "down", "out", "s", "t", "m", "re", "ll", "ve",
    // es / pt / it
    "que", "el", "la", "los", "las", "un", "una", "y", "de", "en", "con", "por", "para", "se",
    "es", "lo", "le", "o", "os", "e", "do", "da", "na", "no", "com", "pra", "nao", "il", "di",
    "che", "non", "per", "sono",
    // fr / de
    "il", "on", "ne", "pas", "et", "le", "les", "des", "du", "au", "est", "suis", "sie", "und",
    "der", "die", "das", "nicht", "ist", "bin", "mit", "ein", "eine", "zu",
];

/// Search words that say "I am looking for a version / a kind of thing", never in a lyric query.
const CATALOGUE_WORDS: &[&str] = &[
    "remix", "remastered", "remaster", "live", "acoustic", "karaoke", "instrumental", "cover",
    "feat", "ft", "album", "playlist", "podcast", "episode", "ost", "soundtrack", "version",
    "slowed", "reverb", "sped", "nightcore", "edit", "mix", "official", "video", "audio",
];

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30ff     // kana
        | 0x3400..=0x4dbf   // CJK ext A
        | 0x4e00..=0x9fff   // CJK unified
        | 0xac00..=0xd7af)  // hangul
}

/// Whether `query` reads like a line of a song rather than a title or a name. Cheap (no I/O, no
/// allocation beyond one normalised copy): run it on every search, and only fire the lyric
/// sources when it says yes. Measured on generated lyric fragments against the search
/// benchmark's 164 labelled title / artist / album / podcast queries (docs/bench/lyricsearch.md).
pub fn looks_like_lyrics(query: &str) -> bool {
    let q = query.trim();
    if q.contains("://") || q.contains("spotify:") || q.chars().count() > 200 {
        return false;
    }
    // Chinese and Japanese run without spaces: count characters instead of words.
    let cjk = q.chars().filter(|c| is_cjk(*c)).count();
    let hangul = q.chars().any(|c| ('\u{ac00}'..='\u{d7af}').contains(&c));
    if cjk >= 6 && !hangul && q.split_whitespace().count() <= 3 {
        return true;
    }
    let n = norm(q);
    let words: Vec<&str> = n.split(' ').filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| CATALOGUE_WORDS.contains(w)) {
        return false;
    }
    let pronouns = words.iter().filter(|w| PRONOUNS.contains(w)).count();
    let grammar = pronouns + words.iter().filter(|w| FUNCTION_WORDS.contains(w)).count();
    match words.len() {
        0..=3 => false,
        4 => pronouns >= 1 || grammar >= 2,
        5 => grammar >= 1 || hangul,
        _ => true,
    }
}

// --- fusion ------------------------------------------------------------------------------------

fn work_key(h: &Hit) -> (String, String) {
    (identity::parse(&h.title, &h.artist, None).base, identity::primary_artist(&h.artist))
}

/// Weighted reciprocal-rank fusion of the sources' rows, best first.
pub fn fuse(lists: &[(Source, Vec<Hit>)]) -> Vec<Work> {
    let mut works: Vec<Work> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    for (src, hits) in lists {
        for (rank, h) in hits.iter().take(DEPTH).enumerate() {
            let key = work_key(h);
            if key.0.is_empty() {
                continue;
            }
            let vote = src.weight() / (RRF_K + rank as f64);
            match at.get(&key) {
                Some(&i) => {
                    let w = &mut works[i];
                    w.score += vote;
                    if !w.sources.contains(src) {
                        w.sources.push(*src);
                    }
                    // Keep the richest description (Deezer / iTunes know album and length).
                    if w.hit.duration.is_none() && h.duration.is_some() {
                        w.hit.duration = h.duration;
                        w.hit.album = h.album.clone();
                    }
                }
                None => {
                    at.insert(key, works.len());
                    works.push(Work { hit: h.clone(), score: vote, sources: vec![*src] });
                }
            }
        }
    }
    works.sort_by(|a, b| b.score.total_cmp(&a.score));
    works
}

// --- sources -----------------------------------------------------------------------------------

async fn get_json(url: &str, query: &[(&str, &str)]) -> Option<Value> {
    let resp = crate::http::client()
        .get(url)
        .query(query)
        .header("User-Agent", crate::http::WEB_UA)
        .timeout(DEADLINE)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        tracing::debug!(status = %resp.status(), url, "lyricsearch: source refused");
        return None;
    }
    resp.json().await.ok()
}

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or_default().to_owned()
}

pub fn parse_deezer(v: &Value) -> Vec<Hit> {
    v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let title = s(d, "/title");
            (!title.is_empty()).then(|| Hit {
                title,
                artist: s(d, "/artist/name"),
                album: Some(s(d, "/album/title")).filter(|a| !a.is_empty()),
                duration: d.get("duration").and_then(Value::as_f64),
            })
        })
        .collect()
}

pub fn parse_itunes(v: &Value) -> Vec<Hit> {
    v.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let title = s(d, "/trackName");
            (!title.is_empty()).then(|| Hit {
                title,
                artist: s(d, "/artistName"),
                album: Some(s(d, "/collectionName")).filter(|a| !a.is_empty()),
                duration: d.get("trackTimeMillis").and_then(Value::as_f64).map(|ms| ms / 1000.0),
            })
        })
        .collect()
}

async fn deezer(q: &str) -> Vec<Hit> {
    let v = get_json("https://api.deezer.com/search", &[("q", q), ("limit", "10")]).await;
    v.as_ref().map(parse_deezer).unwrap_or_default()
}

async fn itunes(q: &str) -> Vec<Hit> {
    let v = get_json(
        "https://itunes.apple.com/search",
        &[("term", q), ("entity", "song"), ("limit", "10")],
    )
    .await;
    v.as_ref().map(parse_itunes).unwrap_or_default()
}

// --- resolution --------------------------------------------------------------------------------

fn work_track(h: &Hit) -> Track {
    Track {
        title: h.title.clone(),
        artist_line: h.artist.clone(),
        artists: identity::artist_names(&h.artist),
        album: h.album.clone(),
        duration: h.duration,
        ..Track::default()
    }
}

/// The row of `rows` that is `hit`, by the importer's matcher: no artist or title gate failed and
/// the score clears "To check". A version gate (karaoke, live, cover) also rules a row out.
pub fn best_row(hit: &Hit, rows: &[SongItem]) -> Option<SongItem> {
    let src = work_track(hit);
    let mut ranked: Vec<(identity::Score, &SongItem)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (identity::score(&src, &Track::from_song(r), i), r))
        .collect();
    identity::sort_ranked(&mut ranked);
    ranked
        .into_iter()
        .find(|(sc, _)| {
            sc.value >= identity::CHECK
                && !sc.gates.iter().any(|g| matches!(g, Gate::Artist | Gate::Title | Gate::Version))
        })
        .map(|(_, r)| r.clone())
}

fn ytm_hits(rows: &[SongItem]) -> Vec<Hit> {
    rows.iter()
        .map(|r| Hit {
            title: r.title.clone(),
            artist: r.artists.clone(),
            album: r.album.clone(),
            duration: r.duration.as_deref().and_then(identity::secs),
        })
        .collect()
}

/// Where the YouTube Music lookups go. A trait so the tests can stand in for InnerTube.
pub trait SongSearch: Sync {
    fn songs<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Vec<SongItem>>;
}

/// The app's InnerTube, anonymous and without touching the search history.
pub struct Ytm<'a> {
    pub it: &'a InnerTube,
    pub client: &'a YouTubeClient,
}

impl SongSearch for Ytm<'_> {
    fn songs<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Vec<SongItem>> {
        async move {
            match self.it.search_songs(self.client, query, false).await {
                Ok(r) => r.items,
                Err(e) => {
                    tracing::debug!(error = %e, "lyricsearch: song search failed");
                    Vec::new()
                }
            }
        }
        .boxed()
    }
}

/// The lyric sources other than YouTube Music. A trait for the same reason.
pub trait LyricSources: Sync {
    fn sources<'a>(&'a self, query: &'a str) -> Vec<BoxFuture<'a, (Source, Vec<Hit>)>>;
}

/// iTunes and Deezer over the network.
pub struct Web;

impl LyricSources for Web {
    fn sources<'a>(&'a self, q: &'a str) -> Vec<BoxFuture<'a, (Source, Vec<Hit>)>> {
        vec![
            deezer(q).map(|h| (Source::Deezer, h)).boxed(),
            itunes(q).map(|h| (Source::Itunes, h)).boxed(),
        ]
    }
}

enum Event {
    Source(Source, Vec<Hit>, Vec<SongItem>),
    Resolved((String, String), Option<SongItem>),
}

/// [`search_by_lyrics`] with the network parts passed in. Never takes longer than `deadline`;
/// what is not in by then is left out.
///
/// Resolution is eager: whenever a source answers, the fused leaders that YouTube Music's own
/// rows don't already hold get their title + artist search at once, so iTunes's and Deezer's
/// answers (about 0.4 s) are resolved while YouTube Music's own (about 0.7 s) is on its way.
pub async fn search_with(
    query: &str,
    web: &dyn LyricSources,
    ytm: &dyn SongSearch,
    deadline: Duration,
) -> Vec<Candidate> {
    let end = tokio::time::Instant::now() + deadline;
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }

    let mut tasks: FuturesUnordered<BoxFuture<'_, Event>> = web
        .sources(q)
        .into_iter()
        .map(|f| f.map(|(s, h)| Event::Source(s, h, Vec::new())).boxed())
        .collect();
    tasks.push(ytm.songs(q).map(|rows| Event::Source(Source::Ytm, ytm_hits(&rows), rows)).boxed());
    let mut sources_left = tasks.len();

    let mut lists: Vec<(Source, Vec<Hit>)> = Vec::new();
    let mut ytm_rows: Vec<SongItem> = Vec::new();
    let mut resolved: HashMap<(String, String), Option<SongItem>> = HashMap::new();
    let mut launched: HashSet<(String, String)> = HashSet::new();

    loop {
        let event = match tokio::time::timeout_at(end, tasks.next()).await {
            Ok(Some(e)) => e,
            Ok(None) | Err(_) => break,
        };
        match event {
            Event::Resolved(key, row) => {
                // A row the lyric query's own YouTube answer gave stays: it is the same song.
                resolved.entry(key).or_insert(row);
            }
            Event::Source(src, hits, rows) => {
                sources_left -= 1;
                if src == Source::Ytm {
                    ytm_rows = rows;
                }
                lists.push((src, hits));
                for w in fuse(&lists).into_iter().take(RESOLVE) {
                    let key = work_key(&w.hit);
                    if matches!(resolved.get(&key), Some(Some(_))) {
                        continue;
                    }
                    if let Some(row) = best_row(&w.hit, &ytm_rows) {
                        resolved.insert(key, Some(row));
                    } else if launched.len() < MAX_LOOKUPS && launched.insert(key.clone()) {
                        let hit = w.hit.clone();
                        tasks.push(
                            async move {
                                let artist =
                                    hit.artist.split([',', '&']).next().unwrap_or_default().trim();
                                let rows = ytm.songs(&format!("{} {artist}", hit.title)).await;
                                Event::Resolved(key, best_row(&hit, &rows))
                            }
                            .boxed(),
                        );
                    }
                }
            }
        }
        // Done early: every source answered and every leader is settled.
        if sources_left == 0 {
            let settled = fuse(&lists)
                .iter()
                .take(RESOLVE)
                .all(|w| resolved.contains_key(&work_key(&w.hit)));
            if settled {
                break;
            }
        }
    }

    let mut out: Vec<Candidate> = Vec::new();
    for w in fuse(&lists) {
        let Some(Some(song)) = resolved.get(&work_key(&w.hit)) else { continue };
        if out.iter().any(|c| c.song.video_id == song.video_id) {
            continue;
        }
        out.push(Candidate { song: song.clone(), score: w.score, sources: w.sources });
    }
    out
}

/// Songs whose lyrics contain `query`, best first, as playable YouTube Music songs. At most
/// [`DEADLINE`]; empty when nothing matched or nothing answered in time.
pub async fn search_by_lyrics(
    state: &crate::state::AppState,
    query: &str,
) -> Vec<Candidate> {
    let Some(client) = state.clients.get(innertube::METADATA_CLIENT) else { return Vec::new() };
    let ytm = Ytm { it: &state.it, client };
    search_with(query, &Web, &ytm, DEADLINE).await
}

#[cfg(test)]
#[path = "lyricsearch_tests.rs"]
mod tests;
