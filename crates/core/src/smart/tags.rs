//! Liked Songs filter chips (lane magic-d3, MAGIC-GAPS #7): genre and mood tags per track, from
//! open data, and the handful of chips worth showing for a library.
//!
//! Spotify's desktop and mobile apps put genre and mood chips over Liked Songs ("Hip-hop",
//! "Chill", ...), built from your own likes; one tap filters the list and Play plays what's left.
//! YouTube Music has no genres per track, so the tags come from open data:
//!
//! 1. **MusicBrainz** (CC0, no key): the artist's community tags, one search per artist
//!    (`/ws/2/artist?query=artist:"…"`, whose answer already carries the tags with vote counts).
//!    MusicBrainz asks for at most one call a second and a User-Agent naming the app.
//! 2. **Last.fm** `track.getTopTags` (a free key) when a key is configured: per-track tags, the
//!    only source with moods. No key ships with the app, so this is off unless the build sets
//!    `SPOTLESS_LASTFM_API_KEY` or the `lastfm_api_key` setting holds one.
//! 3. **Deezer** album genres (no key): coarse, but every track has them. Used when nothing above
//!    gave the track a genre.
//!
//! The order and the thresholds are the ones that won the benchmark on a hand-labelled sample
//! (`docs/bench/tags/`, `docs/streams/magic-d3.md`): see [`Config::BEST`]. Essentia's mood models
//! were ruled out (AGPL; the app is MIT).
//!
//! Raw answers are cached in SQLite (`smart_tags`), so a change to the vocabulary or thresholds
//! re-derives the chips without asking the network again. Everything is local and anonymous: only
//! artist names and titles leave the machine.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use innertube::SongItem;
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::identity::norm;

// ── Vocabulary ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Genre,
    Mood,
}

/// The chips a track can get. Labels are plain genre and mood names.
pub const CHIPS: &[(&str, Kind)] = &[
    ("Pop", Kind::Genre),
    ("Hip-hop", Kind::Genre),
    ("R&B", Kind::Genre),
    ("Rock", Kind::Genre),
    ("Indie", Kind::Genre),
    ("Alternative", Kind::Genre),
    ("Metal", Kind::Genre),
    ("Punk", Kind::Genre),
    ("Electronic", Kind::Genre),
    ("Dance", Kind::Genre),
    ("House", Kind::Genre),
    ("Techno", Kind::Genre),
    ("Jazz", Kind::Genre),
    ("Soul", Kind::Genre),
    ("Funk", Kind::Genre),
    ("Disco", Kind::Genre),
    ("Blues", Kind::Genre),
    ("Folk", Kind::Genre),
    ("Country", Kind::Genre),
    ("Classical", Kind::Genre),
    ("Soundtrack", Kind::Genre),
    ("Latin", Kind::Genre),
    ("Reggae", Kind::Genre),
    ("K-pop", Kind::Genre),
    ("Afrobeats", Kind::Genre),
    ("Ambient", Kind::Genre),
    ("Lo-fi", Kind::Genre),
    ("Chill", Kind::Mood),
    ("Sad", Kind::Mood),
    ("Happy", Kind::Mood),
    ("Energetic", Kind::Mood),
    ("Romantic", Kind::Mood),
    ("Party", Kind::Mood),
];

pub fn kind_of(chip: &str) -> Kind {
    CHIPS.iter().find(|(c, _)| *c == chip).map_or(Kind::Genre, |(_, k)| *k)
}

/// Phrase → chips. The first chip is the phrase's own genre; the rest are what it implies
/// ("grunge" is rock, and alternative). Matched longest phrase first, on whole words, and a matched
/// phrase is used up, so "trip hop" never also reads as "hip hop", nor "rhythm and blues" as blues.
const PHRASES: &[(&str, &[&str])] = &[
    // Hip-hop and its neighbours
    ("hip hop", &["Hip-hop"]),
    ("hiphop", &["Hip-hop"]),
    ("rap", &["Hip-hop"]),
    ("trap", &["Hip-hop"]),
    ("drill", &["Hip-hop"]),
    ("grime", &["Hip-hop", "Electronic"]),
    ("boom bap", &["Hip-hop"]),
    ("g funk", &["Hip-hop", "Funk"]),
    ("gangsta", &["Hip-hop"]),
    ("trip hop", &["Electronic"]),
    ("lo fi", &["Lo-fi"]),
    ("lofi", &["Lo-fi"]),
    ("chillhop", &["Lo-fi", "Hip-hop"]),
    // R&B and soul
    ("rhythm and blues", &["R&B"]),
    ("r and b", &["R&B"]),
    ("rnb", &["R&B"]),
    ("new jack swing", &["R&B"]),
    ("neo soul", &["Soul", "R&B"]),
    ("soul", &["Soul"]),
    ("motown", &["Soul"]),
    ("gospel", &["Soul"]),
    ("funk", &["Funk"]),
    ("disco", &["Disco"]),
    // Rock family
    ("rock", &["Rock"]),
    ("rock and roll", &["Rock"]),
    ("grunge", &["Rock", "Alternative"]),
    ("shoegaze", &["Indie", "Rock"]),
    ("britpop", &["Indie", "Rock"]),
    ("emo", &["Punk", "Rock"]),
    ("indie", &["Indie"]),
    ("alternative", &["Alternative"]),
    ("new wave", &["Alternative", "Pop"]),
    ("post punk", &["Punk", "Alternative"]),
    ("punk", &["Punk"]),
    ("hardcore", &["Punk"]),
    ("metal", &["Metal"]),
    ("metalcore", &["Metal"]),
    ("deathcore", &["Metal"]),
    ("djent", &["Metal"]),
    // Pop
    ("pop", &["Pop"]),
    ("synthpop", &["Pop", "Electronic"]),
    ("electropop", &["Pop", "Electronic"]),
    ("dance pop", &["Pop", "Dance"]),
    ("k pop", &["K-pop", "Pop"]),
    ("kpop", &["K-pop", "Pop"]),
    ("korean", &["K-pop"]),
    ("teen pop", &["Pop"]),
    // Electronic
    ("electronic", &["Electronic"]),
    ("electronica", &["Electronic"]),
    ("electro", &["Electronic"]),
    ("edm", &["Electronic", "Dance"]),
    ("idm", &["Electronic"]),
    ("downtempo", &["Electronic"]),
    ("dubstep", &["Electronic"]),
    ("brostep", &["Electronic", "Dance"]),
    ("drum and bass", &["Electronic"]),
    ("breakbeat", &["Electronic"]),
    ("big beat", &["Electronic", "Dance"]),
    ("synthwave", &["Electronic"]),
    ("industrial", &["Electronic"]),
    ("trance", &["Electronic", "Dance"]),
    ("uk garage", &["Electronic", "Dance"]),
    ("dance", &["Dance"]),
    ("eurodance", &["Dance", "Pop"]),
    ("house", &["House", "Electronic"]),
    ("deep house", &["House", "Electronic"]),
    ("techno", &["Techno", "Electronic"]),
    ("ambient", &["Ambient"]),
    ("new age", &["Ambient"]),
    // Jazz, blues, folk, country
    ("jazz", &["Jazz"]),
    ("bebop", &["Jazz"]),
    ("swing", &["Jazz"]),
    ("bossa nova", &["Latin", "Jazz"]),
    ("blues", &["Blues"]),
    ("folk", &["Folk"]),
    ("americana", &["Country", "Folk"]),
    ("bluegrass", &["Country", "Folk"]),
    ("country", &["Country"]),
    // Classical and scores
    ("classical", &["Classical"]),
    ("baroque", &["Classical"]),
    ("romantic era", &["Classical"]),
    ("opera", &["Classical"]),
    ("orchestral", &["Classical"]),
    ("chamber music", &["Classical"]),
    ("neoclassical", &["Classical"]),
    ("modern classical", &["Classical"]),
    ("contemporary classical", &["Classical"]),
    ("minimalism", &["Classical"]),
    ("soundtrack", &["Soundtrack"]),
    ("film score", &["Soundtrack"]),
    ("film soundtrack", &["Soundtrack"]),
    ("score", &["Soundtrack"]),
    ("video game music", &["Soundtrack"]),
    ("films games", &["Soundtrack"]),
    // Latin, Caribbean, African
    ("latin", &["Latin"]),
    ("reggaeton", &["Latin"]),
    ("urbano latino", &["Latin"]),
    ("salsa", &["Latin"]),
    ("bachata", &["Latin"]),
    ("merengue", &["Latin"]),
    ("cumbia", &["Latin"]),
    ("son cubano", &["Latin"]),
    ("flamenco", &["Latin"]),
    ("tango", &["Latin"]),
    ("samba", &["Latin"]),
    ("mpb", &["Latin"]),
    ("reggae", &["Reggae"]),
    ("roots reggae", &["Reggae"]),
    ("dancehall", &["Reggae"]),
    ("ska", &["Reggae"]),
    ("dub", &["Reggae"]),
    ("afrobeats", &["Afrobeats"]),
    ("afrobeat", &["Afrobeats"]),
    ("afropop", &["Afrobeats"]),
    ("afro pop", &["Afrobeats"]),
    ("afroswing", &["Afrobeats"]),
    ("amapiano", &["Afrobeats", "House"]),
    ("highlife", &["Afrobeats"]),
    ("african music", &["Afrobeats"]),
    // Moods
    ("chill", &["Chill"]),
    ("chillout", &["Chill"]),
    ("chill out", &["Chill"]),
    ("mellow", &["Chill"]),
    ("relaxing", &["Chill"]),
    ("relax", &["Chill"]),
    ("calm", &["Chill"]),
    ("sad", &["Sad"]),
    ("melancholy", &["Sad"]),
    ("melancholic", &["Sad"]),
    ("depressing", &["Sad"]),
    ("heartbreak", &["Sad"]),
    ("happy", &["Happy"]),
    ("feel good", &["Happy"]),
    ("uplifting", &["Happy"]),
    ("cheerful", &["Happy"]),
    ("energetic", &["Energetic"]),
    ("upbeat", &["Energetic"]),
    ("workout", &["Energetic"]),
    ("aggressive", &["Energetic"]),
    ("romantic", &["Romantic"]),
    ("love songs", &["Romantic"]),
    ("love song", &["Romantic"]),
    ("sensual", &["Romantic"]),
    ("party", &["Party"]),
];

/// A raw tag as the vocabulary reads it: lowercase words, `&` as "and", punctuation as spaces.
fn tag_words(raw: &str) -> String {
    let s = raw.to_lowercase().replace("r&b", "rnb").replace("r & b", "rnb");
    norm(&s)
}

/// What one raw tag means: `(chip, head)` pairs, `head` false for implied genres. Unknown tags
/// ("british", "seen live", "female vocalists") mean nothing.
pub fn map_tag(raw: &str) -> Vec<(&'static str, bool)> {
    // Libraries repeat the same few hundred tags; read each once.
    static MEMO: std::sync::Mutex<Option<HashMap<String, Vec<(&'static str, bool)>>>> =
        std::sync::Mutex::new(None);
    if let Some(v) = MEMO.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(raw).cloned())) {
        return v;
    }
    let v = map_tag_uncached(raw);
    if let Ok(mut m) = MEMO.lock() {
        m.get_or_insert_with(HashMap::new).insert(raw.to_owned(), v.clone());
    }
    v
}

fn phrases_longest_first() -> &'static [&'static (&'static str, &'static [&'static str])] {
    static ORDER: std::sync::OnceLock<Vec<&'static (&'static str, &'static [&'static str])>> =
        std::sync::OnceLock::new();
    ORDER.get_or_init(|| {
        let mut order: Vec<_> = PHRASES.iter().collect();
        order.sort_by_key(|(p, _)| std::cmp::Reverse(p.split(' ').count()));
        order
    })
}

fn map_tag_uncached(raw: &str) -> Vec<(&'static str, bool)> {
    let s = tag_words(raw);
    if s.is_empty() {
        return Vec::new();
    }
    let mut words: Vec<Option<&str>> = s.split(' ').map(Some).collect();
    // (position of the phrase's last word, chips), longest phrases first.
    let mut hits: Vec<(usize, &'static [&'static str])> = Vec::new();
    for &&(phrase, chips) in phrases_longest_first() {
        let pw: Vec<&str> = phrase.split(' ').collect();
        let n = pw.len();
        if n > words.len() {
            continue;
        }
        let mut i = 0;
        while i + n <= words.len() {
            if (0..n).all(|k| words[i + k] == Some(pw[k])) {
                hits.push((i + n - 1, chips));
                for w in &mut words[i..i + n] {
                    *w = None;
                }
                i += n;
            } else {
                i += 1;
            }
        }
    }
    // The genre named last is the tag's own ("alternative rock" is rock, "indie pop" is pop).
    let head_pos = hits.iter().map(|(p, _)| *p).max();
    let mut out: Vec<(&'static str, bool)> = Vec::new();
    for (pos, chips) in &hits {
        for (k, c) in chips.iter().enumerate() {
            let head = Some(*pos) == head_pos && k == 0 || kind_of(c) == Kind::Mood;
            match out.iter_mut().find(|(o, _)| o == c) {
                Some(o) => o.1 |= head,
                None => out.push((c, head)),
            }
        }
    }
    out
}

// ── Scoring ────────────────────────────────────────────────────────────────────────────────────

/// One source's raw tags for a track or its artist, with their vote weights.
pub type RawTags = Vec<(String, f64)>;

/// Thresholds for turning raw tags into a track's chips. Picked by the benchmark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// A chip is kept when its score is at least this share of the best tag's weight.
    pub min_share: f64,
    /// How much an implied genre counts next to the tag's own genre.
    pub implied: f64,
    /// At most this many genre chips per track.
    pub max_genres: usize,
    /// Ignore raw tags with fewer votes than this (MusicBrainz counts; 1 = keep all).
    pub min_votes: f64,
}

impl Config {
    /// The benchmark winner (docs/streams/magic-d3.md).
    /// Two-fold CV on 223 labelled tracks: precision 0.88, recall 0.70 (genres).
    pub const BEST: Config = Config { min_share: 0.8, implied: 1.0, max_genres: 4, min_votes: 2.0 };
}

/// Chip scores (0..1) from one source's raw tags.
pub fn score(tags: &[(String, f64)], cfg: &Config) -> BTreeMap<&'static str, f64> {
    let max = tags.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    let mut out: BTreeMap<&'static str, f64> = BTreeMap::new();
    if max <= 0.0 {
        return out;
    }
    for (raw, w) in tags {
        if *w < cfg.min_votes {
            continue;
        }
        let r = w / max;
        for (chip, head) in map_tag(raw) {
            let v = if head { r } else { r * cfg.implied };
            let e = out.entry(chip).or_insert(0.0);
            *e = e.max(v);
        }
    }
    out
}

/// A track's chips from its scores: genres over the threshold (best first, at most
/// `max_genres`), and moods over it.
pub fn chips_from(scores: &BTreeMap<&'static str, f64>, cfg: &Config) -> Vec<&'static str> {
    let mut genres: Vec<(&'static str, f64)> = scores
        .iter()
        .filter(|(c, s)| kind_of(c) == Kind::Genre && **s >= cfg.min_share)
        .map(|(c, s)| (*c, *s))
        .collect();
    genres.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    genres.truncate(cfg.max_genres);
    let mut out: Vec<&'static str> = genres.into_iter().map(|(c, _)| c).collect();
    out.extend(
        scores.iter().filter(|(c, s)| kind_of(c) == Kind::Mood && **s >= cfg.min_share).map(|(c, _)| *c),
    );
    out
}

/// What each source said about one track. Any can be missing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackSources {
    /// MusicBrainz artist tags (the main source).
    pub mb: Option<RawTags>,
    /// Last.fm track tags (only with a key).
    pub lastfm: Option<RawTags>,
    /// Deezer album genres.
    pub deezer: Option<Vec<String>>,
}

/// The shipped combination: MusicBrainz's genres; Last.fm's tags add moods and stand in for the
/// genres when MusicBrainz knew none; Deezer's genres stand in last.
pub fn track_chips(src: &TrackSources, cfg: &Config) -> Vec<&'static str> {
    let has_genre = |c: &[&str]| c.iter().any(|c| kind_of(c) == Kind::Genre);
    let mut out: Vec<&'static str> =
        src.mb.as_ref().map(|t| chips_from(&score(t, cfg), cfg)).unwrap_or_default();
    if let Some(lf) = &src.lastfm {
        let lc = chips_from(&score(lf, cfg), cfg);
        let fill = !has_genre(&out);
        for c in lc {
            if (fill || kind_of(c) == Kind::Mood) && !out.contains(&c) {
                out.push(c);
            }
        }
    }
    if !has_genre(&out) {
        if let Some(dz) = &src.deezer {
            // Deezer's genres carry no votes: every one counts.
            let t: RawTags = dz.iter().map(|g| (g.clone(), 1.0)).collect();
            let dcfg = Config { min_votes: 0.0, ..*cfg };
            for c in chips_from(&score(&t, &dcfg), &dcfg) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

// ── Library chips ──────────────────────────────────────────────────────────────────────────────

/// Spotify shows a handful of chips, each worth filtering by.
pub const MIN_TRACKS: usize = 5;
pub const MAX_CHIPS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chip {
    pub label: String,
    pub kind: Kind,
    /// The tracks under it, in library order.
    pub video_ids: Vec<String>,
}

/// The chips for a library: the most common tags with at least [`MIN_TRACKS`] tracks, at most
/// [`MAX_CHIPS`]. A chip that would hold (nearly) everything filters nothing and is left out, as
/// is one that holds almost the same tracks as a bigger chip.
pub fn library_chips(tracks: &[(String, Vec<&'static str>)]) -> Vec<Chip> {
    let tagged = tracks.iter().filter(|(_, c)| !c.is_empty()).count();
    let mut by: HashMap<&'static str, Vec<String>> = HashMap::new();
    for (id, chips) in tracks {
        for c in chips {
            by.entry(c).or_default().push(id.clone());
        }
    }
    let rank = |c: &str| CHIPS.iter().position(|(x, _)| *x == c).unwrap_or(usize::MAX);
    let mut all: Vec<(&'static str, Vec<String>)> =
        by.into_iter().filter(|(_, ids)| ids.len() >= MIN_TRACKS).collect();
    // Biggest first; on a tie the broader genre (earlier in CHIPS) wins.
    all.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(rank(a.0).cmp(&rank(b.0))));
    let mut out: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (label, ids) in all {
        if tagged >= 10 && ids.len() * 10 > tagged * 9 {
            continue; // covers over 90% of what's tagged: filters nothing
        }
        let set: HashSet<&String> = ids.iter().collect();
        let dup = out.iter().any(|(_, o)| {
            let inter = o.iter().filter(|x| set.contains(x)).count();
            let union = set.len() + o.len() - inter;
            union > 0 && inter * 10 >= union * 9
        });
        if dup {
            continue;
        }
        out.push((label, ids));
        if out.len() == MAX_CHIPS {
            break;
        }
    }
    out.into_iter()
        .map(|(label, ids)| Chip { label: label.to_owned(), kind: kind_of(label), video_ids: ids })
        .collect()
}

// ── Sources (network) ──────────────────────────────────────────────────────────────────────────

const UA: &str = concat!("Spotless/", env!("CARGO_PKG_VERSION"), " (desktop music player)");
/// Cached answers are kept this long (genres barely move).
const TTL: i64 = 90 * 24 * 3600;

#[derive(Deserialize)]
struct MbSearch {
    #[serde(default)]
    artists: Vec<MbArtist>,
}
#[derive(Deserialize)]
struct MbArtist {
    name: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    tags: Vec<MbTag>,
    #[serde(default)]
    aliases: Vec<MbAlias>,
}
#[derive(Deserialize)]
struct MbTag {
    name: String,
    count: i64,
}
#[derive(Deserialize)]
struct MbAlias {
    name: String,
}

/// From MusicBrainz's search candidates (best first): the first whose name (or an alias) is the
/// one asked for, else the first scoring 95+ (a spelling MusicBrainz knows, "Tupac" for "2Pac").
fn mb_pick(name: &str, cands: &[(String, i64, Vec<String>, RawTags)]) -> Option<RawTags> {
    let want = norm(name);
    cands
        .iter()
        .find(|(n, _, aliases, _)| norm(n) == want || aliases.iter().any(|a| norm(a) == want))
        .or_else(|| cands.first().filter(|c| c.1 >= 95))
        .map(|c| c.3.clone())
}

async fn mb_artist(name: &str) -> Result<Option<RawTags>, String> {
    let q = format!("artist:\"{}\"", name.replace(['"', '\\'], ""));
    let url = format!(
        "https://musicbrainz.org/ws/2/artist/?fmt=json&limit=5&query={}",
        urlencoding::encode(&q)
    );
    let r = crate::http::client()
        .get(url)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("musicbrainz {}", r.status()));
    }
    let s: MbSearch = r.json().await.map_err(|e| e.to_string())?;
    let cands: Vec<(String, i64, Vec<String>, RawTags)> = s
        .artists
        .into_iter()
        .map(|a| {
            (
                a.name,
                a.score,
                a.aliases.into_iter().map(|x| x.name).collect(),
                a.tags.into_iter().map(|t| (t.name, t.count as f64)).collect(),
            )
        })
        .collect();
    Ok(mb_pick(name, &cands))
}

#[derive(Deserialize)]
struct LfTop {
    toptags: Option<LfTags>,
}
#[derive(Deserialize)]
struct LfTags {
    #[serde(default)]
    tag: Vec<LfTag>,
}
#[derive(Deserialize)]
struct LfTag {
    name: String,
    count: f64,
}

async fn lastfm_track(key: &str, artist: &str, title: &str) -> Result<Option<RawTags>, String> {
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=track.gettoptags&autocorrect=1&format=json&api_key={}&artist={}&track={}",
        urlencoding::encode(key),
        urlencoding::encode(artist),
        urlencoding::encode(title)
    );
    let r = crate::http::client()
        .get(url)
        .header("User-Agent", UA)
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let t: LfTop = r.json().await.map_err(|e| e.to_string())?;
    Ok(t.toptags.map(|t| t.tag.into_iter().take(30).map(|x| (x.name, x.count)).collect()))
}

#[derive(Deserialize)]
struct DzSearch {
    #[serde(default)]
    data: Vec<DzTrack>,
}
#[derive(Deserialize)]
struct DzTrack {
    album: Option<DzAlbumRef>,
}
#[derive(Deserialize)]
struct DzAlbumRef {
    id: i64,
}
#[derive(Deserialize)]
struct DzAlbum {
    genres: Option<DzGenres>,
}
#[derive(Deserialize)]
struct DzGenres {
    #[serde(default)]
    data: Vec<DzGenre>,
}
#[derive(Deserialize)]
struct DzGenre {
    name: String,
}

async fn deezer_genres(artist: &str, title: &str) -> Result<Option<Vec<String>>, String> {
    let q = format!("artist:\"{}\" track:\"{}\"", artist.replace('"', ""), title.replace('"', ""));
    let url = format!("https://api.deezer.com/search?limit=1&q={}", urlencoding::encode(&q));
    let get = |u: String| {
        crate::http::client().get(u).header("User-Agent", UA).timeout(Duration::from_secs(6)).send()
    };
    let s: DzSearch =
        get(url).await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let Some(id) = s.data.first().and_then(|t| t.album.as_ref()).map(|a| a.id) else {
        return Ok(None);
    };
    let a: DzAlbum = get(format!("https://api.deezer.com/album/{id}"))
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(a.genres.map(|g| g.data.into_iter().map(|x| x.name).collect()))
}

// ── Cache ──────────────────────────────────────────────────────────────────────────────────────

pub fn migrate(db: &Db) {
    db.with_conn(|c| {
        if let Err(e) = c.execute_batch(
            "CREATE TABLE IF NOT EXISTS smart_tags (
                 key  TEXT PRIMARY KEY,
                 json TEXT NOT NULL,
                 at   INTEGER NOT NULL
             );",
        ) {
            tracing::warn!(error = %e, "tags: creating the table failed");
        }
    });
}

fn get<T: serde::de::DeserializeOwned>(db: &Db, key: &str) -> Option<Option<T>> {
    let row: Option<(String, i64)> = db.with_conn(|c| {
        c.query_row("SELECT json, at FROM smart_tags WHERE key = ?1", [key], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok()
    });
    let (json, at) = row?;
    if super::now_secs() - at > TTL {
        return None;
    }
    serde_json::from_str(&json).ok()
}

fn put<T: Serialize>(db: &Db, key: &str, v: &Option<T>) {
    let Ok(json) = serde_json::to_string(v) else { return };
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_tags(key, json, at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET json = excluded.json, at = excluded.at",
            rusqlite::params![key, json, super::now_secs()],
        );
    });
}

/// The artist a track is tagged by: the first linked artist, else the artist line up to the first
/// separator ("Drake, Rihanna" is Drake; "Earth, Wind & Fire" has no link, so it's searched whole
/// first, see [`fill`]).
pub fn artist_of(s: &SongItem) -> String {
    if let Some(r) = s.artist_runs.iter().find(|r| r.id.is_some()) {
        return r.text.trim().to_owned();
    }
    let line = s.artists.trim();
    line.split(" feat").next().unwrap_or(line).trim().to_owned()
}

fn first_part(artist: &str) -> String {
    artist.split([',', '&', '\u{2022}']).map(str::trim).find(|p| !p.is_empty()).unwrap_or("").to_owned()
}

fn mb_key(artist: &str) -> String {
    format!("mb:{}", norm(artist))
}
fn lf_key(artist: &str, title: &str) -> String {
    format!("lf:{}|{}", norm(artist), norm(title))
}
fn dz_key(video_id: &str) -> String {
    format!("dz:{video_id}")
}

fn title_of(s: &SongItem) -> String {
    crate::identity::parse(&s.title, &s.artists, s.album.as_deref()).base_raw
}

/// What the cache knows about a track (no network).
pub fn cached_sources(db: &Db, s: &SongItem) -> (TrackSources, bool) {
    let artist = artist_of(s);
    let mb: Option<Option<RawTags>> = get(db, &mb_key(&artist));
    let lastfm: Option<Option<RawTags>> = get(db, &lf_key(&artist, &title_of(s)));
    let deezer: Option<Option<Vec<String>>> = get(db, &dz_key(&s.video_id));
    let complete = mb.is_some();
    (
        TrackSources { mb: mb.flatten(), lastfm: lastfm.flatten(), deezer: deezer.flatten() },
        complete,
    )
}

/// The Last.fm key, when the build or the settings have one.
pub fn lastfm_key(db: &Db) -> Option<String> {
    db.get_setting("lastfm_api_key")
        .filter(|k| !k.trim().is_empty())
        .or_else(|| option_env!("SPOTLESS_LASTFM_API_KEY").map(str::to_owned))
}

// ── The Liked Songs answer ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LikedChips {
    pub chips: Vec<Chip>,
    /// Liked tracks whose tags are known (looked up, found or not).
    pub done: usize,
    pub total: usize,
}

static FILLING: AtomicBool = AtomicBool::new(false);

/// The chips for Liked Songs from what's cached. Starts the background lookup for the rest.
pub fn liked_chips(state: &Arc<crate::state::AppState>) -> LikedChips {
    let songs: Vec<SongItem> =
        state.db.with_conn(|c| crate::library::liked_songs(c)).into_iter().map(|e| e.song).collect();
    let out = chips_for(&state.db, &songs);
    if out.done < out.total {
        spawn_fill(state.clone(), songs);
    }
    out
}

pub fn chips_for(db: &Db, songs: &[SongItem]) -> LikedChips {
    let mut done = 0;
    let tracks: Vec<(String, Vec<&'static str>)> = songs
        .iter()
        .map(|s| {
            let (src, complete) = cached_sources(db, s);
            done += complete as usize;
            (s.video_id.clone(), track_chips(&src, &Config::BEST))
        })
        .collect();
    LikedChips { chips: library_chips(&tracks), done, total: songs.len() }
}

fn spawn_fill(state: Arc<crate::state::AppState>, songs: Vec<SongItem>) {
    if FILLING.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::rt::spawn(async move {
        fill(&state, &songs).await;
        FILLING.store(false, Ordering::SeqCst);
        use crate::host::HostExt;
        let _ = state.host.emit("smart-tags-changed", chips_for(&state.db, &songs));
    });
}

/// Look up what the cache lacks: MusicBrainz per artist (one call a second), then the fallbacks
/// per track that still has no genre. Emits `smart-tags-changed` as chips fill in.
pub async fn fill(state: &crate::state::AppState, songs: &[SongItem]) {
    use crate::host::HostExt;
    let db = &state.db;
    let mut seen = HashSet::new();
    let mut n = 0usize;
    for s in songs {
        if crate::local::is_local_song(&s.video_id) {
            continue;
        }
        let artist = artist_of(s);
        let key = mb_key(&artist);
        if artist.is_empty() || !seen.insert(key.clone()) || get::<RawTags>(db, &key).is_some() {
            continue;
        }
        let mut found = mb_artist(&artist).await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        if matches!(found, Ok(None)) {
            let first = first_part(&artist);
            if !first.is_empty() && first != artist {
                found = mb_artist(&first).await;
                tokio::time::sleep(Duration::from_millis(1100)).await;
            }
        }
        match found {
            Ok(tags) => put(db, &key, &tags),
            Err(e) => {
                // Rate limit or no network: stop; the next visit picks up where this left off.
                tracing::info!(error = %e, "tags: musicbrainz lookup stopped");
                return;
            }
        }
        n += 1;
        if n % 15 == 0 {
            let _ = state.host.emit("smart-tags-changed", chips_for(db, songs));
        }
    }
    let lf = lastfm_key(db);
    for s in songs {
        let (src, complete) = cached_sources(db, s);
        if !complete || crate::local::is_local_song(&s.video_id) {
            continue;
        }
        let chips = track_chips(&src, &Config::BEST);
        let artist = artist_of(s);
        let title = title_of(s);
        if let Some(key) = lf.as_deref() {
            let k = lf_key(&artist, &title);
            if get::<RawTags>(db, &k).is_none() {
                match lastfm_track(key, &artist, &title).await {
                    Ok(t) => put(db, &k, &t),
                    Err(_) => {}
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        if !chips.iter().any(|c| kind_of(c) == Kind::Genre) {
            let k = dz_key(&s.video_id);
            if get::<Vec<String>>(db, &k).is_none() {
                if let Ok(g) = deezer_genres(&artist, &title).await {
                    put(db, &k, &g);
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heads(raw: &str) -> Vec<&'static str> {
        map_tag(raw).into_iter().filter(|(_, h)| *h).map(|(c, _)| c).collect()
    }
    fn all(raw: &str) -> Vec<&'static str> {
        let mut v: Vec<_> = map_tag(raw).into_iter().map(|(c, _)| c).collect();
        v.sort();
        v
    }

    #[test]
    fn vocabulary_reads_compound_genres() {
        assert_eq!(heads("alternative rock"), ["Rock"]);
        assert_eq!(all("alternative rock"), ["Alternative", "Rock"]);
        assert_eq!(heads("indie pop"), ["Pop"]);
        assert_eq!(all("Hip-Hop"), ["Hip-hop"]);
        assert_eq!(all("trip hop"), ["Electronic"]);
        assert_eq!(all("rhythm and blues"), ["R&B"]);
        assert_eq!(all("contemporary R&B"), ["R&B"]);
        assert_eq!(all("dancehall"), ["Reggae"]);
        assert_eq!(all("K-Pop"), ["K-pop", "Pop"]);
        assert_eq!(all("deep house"), ["Electronic", "House"]);
        assert_eq!(all("british"), Vec::<&str>::new());
        assert_eq!(all("seen live"), Vec::<&str>::new());
        assert_eq!(all("Rap/Hip Hop"), ["Hip-hop"]);
        assert_eq!(all("Films/Games"), ["Soundtrack"]);
        assert_eq!(all("mellow"), ["Chill"]);
    }

    fn raw(v: &[(&str, f64)]) -> RawTags {
        v.iter().map(|(n, w)| (n.to_string(), *w)).collect()
    }

    #[test]
    fn scores_follow_the_votes() {
        let cfg = Config::BEST;
        let nirvana = raw(&[("grunge", 69.0), ("alternative rock", 30.0), ("rock", 26.0), ("american", 8.0)]);
        let c = chips_from(&score(&nirvana, &cfg), &cfg);
        assert!(c.contains(&"Rock") && c.contains(&"Alternative"), "{c:?}");
        // A lone vote for a genre next to a well-voted one doesn't make the cut.
        let one = raw(&[("hip hop", 40.0), ("jazz", 2.0)]);
        assert_eq!(chips_from(&score(&one, &cfg), &cfg), ["Hip-hop"]);
    }

    #[test]
    fn fallbacks_fill_only_what_is_missing() {
        let cfg = Config::BEST;
        let src = TrackSources {
            mb: Some(raw(&[("hip hop", 10.0)])),
            lastfm: Some(raw(&[("rnb", 100.0), ("chill", 90.0)])),
            deezer: Some(vec!["Pop".into()]),
        };
        // Last.fm adds its mood but not a second opinion on the genre; Deezer isn't needed.
        assert_eq!(track_chips(&src, &cfg), ["Hip-hop", "Chill"]);
        let none = TrackSources { mb: Some(vec![]), lastfm: None, deezer: Some(vec!["Rap/Hip Hop".into()]) };
        assert_eq!(track_chips(&none, &cfg), ["Hip-hop"]);
    }

    #[test]
    fn library_chips_pick_useful_ones() {
        let mut tracks = Vec::new();
        for i in 0..20 {
            let mut c = vec!["Pop"];
            if i < 8 {
                c.push("Hip-hop");
            }
            if i < 4 {
                c.push("Jazz"); // too few
            }
            if (8..16).contains(&i) {
                c.push("Rock");
                c.push("Alternative"); // the same tracks as Rock: dropped
            }
            tracks.push((format!("v{i}"), c));
        }
        let chips = library_chips(&tracks);
        let labels: Vec<&str> = chips.iter().map(|c| c.label.as_str()).collect();
        // Pop holds every tagged track: no use as a filter.
        assert_eq!(labels, ["Hip-hop", "Rock"]);
        assert_eq!(chips[1].video_ids.len(), 8);
    }

    #[test]
    fn picks_the_named_artist() {
        let c = vec![
            ("Nirvana".to_string(), 100, vec![], raw(&[("grunge", 69.0)])),
            ("Nirvana".to_string(), 75, vec![], raw(&[("progressive rock", 3.0)])),
        ];
        assert_eq!(mb_pick("Nirvana", &c).unwrap()[0].0, "grunge");
        let alias = vec![("2Pac".to_string(), 100, vec!["Tupac".to_string()], raw(&[("hip hop", 9.0)]))];
        assert!(mb_pick("Tupac", &alias).is_some());
        let other = vec![("Somebody Else".to_string(), 60, vec![], raw(&[("pop", 1.0)]))];
        assert!(mb_pick("Nobody", &other).is_none());
    }
}

// ── Benchmark ── `TAGS_BENCH_RAW=/path/raw.json cargo test -p encore-core --lib tags_bench -- --ignored --nocapture`
#[cfg(test)]
#[path = "tags_bench.rs"]
mod tags_bench;
