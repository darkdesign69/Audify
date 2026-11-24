//! Search pipeline (stream: search2, docs/streams/search2.md), shared by the desktop and iOS.
//!
//! - [`search_overview`]: the "All" view in one call. YouTube Music's mixed search plus the first
//!   page of the Songs chip, the podcast directories blended in (a person's name finds their show),
//!   the Top result picked across types, and a fallback chain so the page is never empty.
//! - [`search_page`]: one chip (Songs, Videos, Artists, Albums, Playlists, Profiles, Podcasts &
//!   Shows, Episodes) as an endless list. Each call answers one page plus the token for the next
//!   (ytmusicapi's filter params and continuations, `innertube::search`).
//!
//! **Never "nothing found".** When the normal search comes back empty, these steps run in order
//! and the first one with results wins:
//! 1. YouTube Music's own spelling correction (`showingResultsForRenderer` /
//!    `didYouMeanRenderer`). A "Showing results for" line names it.
//! 2. Prefix completion through the typeahead (`music/get_search_suggestions`, as ytmusicapi's
//!    `get_search_suggestions`); its top completion. Then the query with its least informative
//!    word dropped, one at a time.
//! 3. Songs by their words (`lyricsearch`), when the query reads like lyrics. Then nearby matches
//!    in the user's own library, history and downloads (strsim Jaro-Winkler).
//! 4. "Try these": the user's most played songs and artists, else YouTube's trending chart.
//!
//! The normal path is unchanged: the fallbacks only run when it is empty. Offline, or when
//! YouTube cannot be reached at all, the search runs over the library and the downloads instead.
//!
//! The network steps go through [`Backend`], so the benchmark (`search_bench`) replays recorded
//! answers through exactly this code.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use innertube::search::{Page, SpellFix};
use innertube::{BrowseItem, SearchFilter, SearchResults, SongItem};
use serde::{Deserialize, Serialize};

use crate::podcasts::{self, Episode, Show};
use crate::state::AppState;

// ─── what the UI receives ─────────────────────────────────────────────────────────────────────────

/// Why the results answer something other than what was typed. The UI shows a small line:
/// "Showing results for <query>" (and "Search instead for <original>" when YouTube corrected it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectionKind {
    /// YouTube Music's own correction ("Showing results for" / an applied "Did you mean").
    Spelling,
    /// The typeahead's top completion of what was typed.
    Completion,
    /// The query with a word left out.
    Relaxed,
    /// Matches from the user's library, history and downloads.
    Library,
    /// No network: the library and downloads only.
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub kind: CorrectionKind,
    /// What the results answer (the corrected query; the typed one for Library / Offline).
    pub query: String,
    /// What was typed.
    pub original: String,
}

/// The last-resort shelf, labelled softly ("Try these"), never as an error.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TryThese {
    pub songs: Vec<SongItem>,
    pub artists: Vec<BrowseItem>,
}

impl TryThese {
    fn is_empty(&self) -> bool {
        self.songs.is_empty() && self.artists.is_empty()
    }
}

/// The All view. `results` is YouTube Music's mixed search (re-ranked), `songs` the first page of
/// the Songs chip (re-ranked, with the artist page's Top songs blended in); the Songs chip goes on
/// from `songsContinuation` with [`search_page`] on `query`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOverview {
    /// The query the results answer (after any correction). Pass it to [`search_page`].
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction: Option<Correction>,
    pub results: SearchResults,
    pub songs: Vec<SongItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub songs_continuation: Option<String>,
    /// Podcast shows: YouTube Music + the directories, merged, the host's own show first.
    pub shows: Vec<Show>,
    pub episodes: Vec<Episode>,
    /// The directories were still answering: the UI asks `podcast_search` itself.
    pub pods_pending: bool,
    /// Index into `shows` of the show that is the Top result (`podcasts::search::top_show`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_show: Option<usize>,
    /// The Podcasts and Episodes shelves go right under the top rows, before the music shelves:
    /// a show is the Top result, or the query names a show's host or title (see
    /// [`all_view_order`]).
    pub podcasts_first: bool,
    /// Only when nothing else answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub try_these: Option<TryThese>,
    /// The top bar's dropdown: up to [`DROPDOWN_ROWS`] mixed rows in Spotify's order
    /// ([`dropdown_order`]), drawn from the lists above.
    pub dropdown: Vec<DropRow>,
}

/// One row of the top bar's search dropdown (stream: search-dropdown).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DropRow {
    /// An artist, album or playlist (or the Top result card).
    Card { item: BrowseItem },
    Song { song: SongItem },
    Show { show: Show },
}

/// One page of one chip. Exactly one of the row lists is filled, by the chip's kind.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub filter: String,
    /// The query the rows answer; send it back with `continuation` for the next page.
    pub query: String,
    pub songs: Vec<SongItem>,
    pub items: Vec<BrowseItem>,
    pub shows: Vec<Show>,
    pub episodes: Vec<Episode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction: Option<Correction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub try_these: Option<TryThese>,
}

impl SearchPage {
    fn len(&self) -> usize {
        self.songs.len() + self.items.len() + self.shows.len() + self.episodes.len()
    }
}

// ─── the network steps ────────────────────────────────────────────────────────────────────────────

/// `search_all` with the artist page's Top songs of its artist card (stream: podtop).
#[derive(Debug, Clone, Default)]
pub struct AllAnswer {
    pub results: SearchResults,
    pub fix: Option<SpellFix>,
    /// (artist name, Top songs) when the card may be the query's artist.
    pub artist_top: Option<(String, Vec<SongItem>)>,
}

/// The YouTube Music requests the pipeline makes. [`Live`] is the app; the benchmark replays.
pub trait Backend: Sync {
    fn all(&self, q: &str) -> impl Future<Output = Result<AllAnswer, String>> + Send;
    fn songs(&self, q: &str) -> impl Future<Output = Result<Page<SongItem>, String>> + Send;
    /// Typeahead completions, best first.
    fn suggest(&self, q: &str) -> impl Future<Output = Result<Vec<String>, String>> + Send;
}

/// The app's backend: the shared InnerTube client.
pub struct Live<'a> {
    pub state: &'a Arc<AppState>,
    pub record_history: bool,
}

/// Cap on the artist-page read for the Top songs blend.
const ARTIST_TOP_WAIT: Duration = Duration::from_millis(2500);

impl Backend for Live<'_> {
    async fn all(&self, q: &str) -> Result<AllAnswer, String> {
        let c = crate::podcasts::api::client(self.state)?;
        let (r, fix) =
            self.state.it.search_all_fixed(c, q, self.record_history).await.map_err(|e| e.to_string())?;
        let results = crate::smart::recs::rerank_search_all(&self.state.db, q, r);
        // stream: podtop, moved from the search page: the artist card's page "Top songs".
        let artist_top = match artist_card(q, &results) {
            Some(id) => tokio::time::timeout(ARTIST_TOP_WAIT, crate::api::get_artist(self.state, id))
                .await
                .ok()
                .and_then(Result::ok)
                .filter(|p| !p.top_songs.is_empty())
                .map(|p| (p.name.clone().unwrap_or_default(), p.top_songs)),
            None => None,
        };
        Ok(AllAnswer { results, fix, artist_top })
    }

    async fn songs(&self, q: &str) -> Result<Page<SongItem>, String> {
        let c = crate::podcasts::api::client(self.state)?;
        let mut p = self
            .state
            .it
            .search_tracks_page(c, q, SearchFilter::Songs, None)
            .await
            .map_err(|e| e.to_string())?;
        p.items = crate::smart::recs::rerank_search_songs(&self.state.db, q, p.items);
        Ok(p)
    }

    async fn suggest(&self, q: &str) -> Result<Vec<String>, String> {
        let c = crate::podcasts::api::client(self.state)?;
        let s = self.state.it.search_suggestions(c, q).await.map_err(|e| e.to_string())?;
        Ok(s.queries.into_iter().filter(|x| !x.history).map(|x| x.text).collect())
    }
}

/// The artist card whose page may hold the query's Top songs: YouTube's own artist Top card, or
/// the first artist row when its name is the query or starts with it (as the search page did).
pub fn artist_card(q: &str, r: &SearchResults) -> Option<String> {
    let fold = |x: &str| crate::podcasts::search::norm(x);
    let qn = fold(q);
    let card = match r.top.first() {
        Some(t) if t.kind == "artist" => Some(t),
        _ => r.artists.first().filter(|a| {
            let t = fold(&a.title);
            t == qn || t.starts_with(&qn)
        }),
    }?;
    card.id.starts_with("UC").then(|| card.id.clone())
}

// ─── the chain ───────────────────────────────────────────────────────────────────────────────────

/// What the network steps settled on.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub query: String,
    pub correction: Option<Correction>,
    pub results: SearchResults,
    pub songs: Vec<SongItem>,
    pub songs_continuation: Option<String>,
    /// Each query tried, for the logs and the benchmark.
    pub tried: Vec<String>,
}

impl Resolved {
    pub fn is_empty(&self) -> bool {
        let r = &self.results;
        self.songs.is_empty()
            && r.top.is_empty()
            && r.songs.is_empty()
            && r.artists.is_empty()
            && r.albums.is_empty()
            && r.playlists.is_empty()
    }
}

/// Retries after the first search, at most (each costs two requests).
const MAX_RETRIES: usize = 3;

/// Words that say least about what is wanted, dropped first.
const FILLER: [&str; 22] = [
    "the", "a", "an", "of", "and", "by", "feat", "ft", "song", "songs", "music", "lyrics", "lyric",
    "official", "video", "audio", "full", "album", "remix", "live", "version", "hd",
];

fn fold(s: &str) -> String {
    crate::podcasts::search::norm(s)
}

/// How much a word says, lowest first: filler; then words that read like a keyboard mash (no
/// vowel, a run of 4+ consonants, one letter repeated); then by length. Ties: the later word
/// goes first (it is the one most likely still being typed).
fn informativeness(w: &str) -> (u8, usize) {
    let lw = w.to_lowercase();
    if FILLER.contains(&lw.as_str()) || lw.chars().count() <= 1 {
        return (0, 0);
    }
    let latin = lw.chars().all(|c| c.is_ascii_alphanumeric());
    if latin {
        let vowel = |c: char| "aeiouy".contains(c);
        let mut run = 0;
        let mut max_run = 0;
        for c in lw.chars() {
            if c.is_ascii_alphabetic() && !vowel(c) {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 0;
            }
        }
        let distinct: std::collections::HashSet<char> = lw.chars().collect();
        let digits = lw.chars().all(|c| c.is_ascii_digit());
        if !digits && (!lw.chars().any(vowel) || max_run >= 4 || (distinct.len() <= 2 && lw.len() >= 4)) {
            return (1, lw.len());
        }
    }
    (2, lw.chars().count())
}

/// The query with its least informative word left out, then the next one, … (never empty).
pub fn relaxations(q: &str) -> Vec<String> {
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.len() < 2 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| (informativeness(words[i]), std::cmp::Reverse(i)));
    let mut out = Vec::new();
    let mut dropped = std::collections::HashSet::new();
    for &i in order.iter().take(words.len() - 1) {
        dropped.insert(i);
        let rest: Vec<&str> =
            words.iter().enumerate().filter(|(j, _)| !dropped.contains(j)).map(|(_, w)| *w).collect();
        out.push(rest.join(" "));
    }
    out
}

/// The typeahead's best completion that isn't the query itself.
fn completion(q: &str, suggestions: &[String]) -> Option<String> {
    let qn = fold(q);
    suggestions.iter().find(|s| !s.trim().is_empty() && fold(s) != qn).cloned()
}

async fn search_once<B: Backend>(b: &B, q: &str) -> (Result<AllAnswer, String>, Result<Page<SongItem>, String>) {
    tokio::join!(b.all(q), b.songs(q))
}

/// The Songs list for the All view: the re-ranked first page, the query's artist decided with
/// YouTube's Top card as evidence (stream: search-pop, `identity::search_artist_intent_with`),
/// then the artist page's Top songs blended in for that artist (stream: podtop). Pure; the
/// benchmarks call it too.
pub fn songs_for(q: &str, ranked: Vec<SongItem>, results: &SearchResults, artist_top: Option<&(String, Vec<SongItem>)>) -> Vec<SongItem> {
    use crate::identity;
    let mut list = ranked;
    if list.is_empty() {
        if let Some((name, top)) = artist_top {
            if !top.is_empty() && identity::norm(name) == identity::norm(q) {
                return top.clone();
            }
        }
        return list;
    }
    let Some(card) = results.top.first().and_then(|c| identity::card_artist_intent(q, c)) else {
        // No artist card to go by: the row rule alone, as before.
        return match artist_top {
            Some((name, top)) => identity::apply_artist_top_songs(q, list, name, top),
            None => list,
        };
    };
    let base = identity::search_artist_intent(q, &list);
    let who = identity::search_artist_intent_with(q, &list, Some(&card));
    if who != base {
        if let Some(w) = &who {
            list = identity::rerank_search_songs_for(q, list, w);
        }
    }
    match (who, artist_top) {
        (Some(w), Some((name, top))) => identity::apply_artist_top_songs_as(&w, list, name, top),
        _ => list,
    }
}

fn settle(q: &str, all: AllAnswer, songs: Page<SongItem>) -> Resolved {
    let list = songs_for(q, songs.items, &all.results, all.artist_top.as_ref());
    Resolved {
        query: q.to_owned(),
        correction: None,
        results: all.results,
        songs: list,
        songs_continuation: songs.continuation,
        tried: vec![q.to_owned()],
    }
}

/// The network part of the chain (steps 1 and 2). `Err` only when YouTube can't be reached at
/// all; an empty `Ok` means every step came back empty and the local steps take over.
pub async fn resolve<B: Backend>(b: &B, typed: &str) -> Result<Resolved, String> {
    let q = typed.trim();
    let (all, songs) = search_once(b, q).await;
    if let (Err(a), Err(s)) = (&all, &songs) {
        return Err(format!("{a}; {s}"));
    }
    let fix = all.as_ref().ok().and_then(|a| a.fix.clone()).or_else(|| songs.as_ref().ok().and_then(|s| s.spell_fix.clone()));
    let mut out = settle(q, all.unwrap_or_default(), songs.unwrap_or_default());
    if let Some(f) = fix.as_ref().filter(|f| f.applied) {
        // YouTube already answered the corrected query.
        out.query = f.query.clone();
        out.correction =
            Some(Correction { kind: CorrectionKind::Spelling, query: f.query.clone(), original: q.to_owned() });
    }
    if !out.is_empty() {
        return Ok(out);
    }
    // ── fallbacks: only when the normal search is empty ──
    let mut tried = out.tried.clone();
    let mut queue: Vec<(CorrectionKind, String)> = Vec::new();
    if let Some(f) = fix.filter(|f| !f.applied) {
        queue.push((CorrectionKind::Spelling, f.query));
    }
    let mut suggested = false;
    let mut retries = 0;
    loop {
        if queue.is_empty() && !suggested {
            suggested = true;
            let mut s = b.suggest(q).await.unwrap_or_default();
            // A single long mash: complete its first half instead.
            if completion(q, &s).is_none() && !q.contains(' ') && q.chars().count() >= 6 {
                let half: String = q.chars().take((q.chars().count() / 2).max(3)).collect();
                s = b.suggest(&half).await.unwrap_or_default();
            }
            if let Some(c) = completion(q, &s) {
                queue.push((CorrectionKind::Completion, c));
            }
            queue.extend(relaxations(q).into_iter().map(|r| (CorrectionKind::Relaxed, r)));
        }
        let Some((kind, next)) = (!queue.is_empty()).then(|| queue.remove(0)) else { break };
        if retries >= MAX_RETRIES {
            break;
        }
        if tried.iter().any(|t| fold(t) == fold(&next)) {
            continue;
        }
        retries += 1;
        tried.push(next.clone());
        let (all, songs) = search_once(b, &next).await;
        let mut r = settle(&next, all.unwrap_or_default(), songs.unwrap_or_default());
        if !r.is_empty() {
            r.correction = Some(Correction { kind, query: next.clone(), original: q.to_owned() });
            r.tried = tried;
            return Ok(r);
        }
    }
    out.tried = tried;
    Ok(out)
}

// ─── the local steps ─────────────────────────────────────────────────────────────────────────────

/// What the user has: liked songs, recent plays, downloads, saved artists / albums / playlists.
#[derive(Debug, Clone, Default)]
pub struct LocalPool {
    pub songs: Vec<SongItem>,
    pub cards: Vec<BrowseItem>,
}

pub fn local_pool(state: &AppState) -> LocalPool {
    let mut pool = LocalPool::default();
    let mut seen = std::collections::HashSet::new();
    {
        let conn = state.db.conn();
        let liked = crate::library::liked_songs(&conn).into_iter().map(|e| e.song);
        let recent = crate::library::recent_tracks(&conn, 300).into_iter().map(|t| t.song);
        let downloaded = crate::downloads::store::done_songs(&conn);
        for s in liked.chain(recent).chain(downloaded) {
            if seen.insert(s.video_id.clone()) {
                pool.songs.push(s);
            }
        }
        for it in crate::library::saved(&conn, None) {
            let kind: &'static str = match it.kind.as_str() {
                "artist" => "artist",
                "album" => "album",
                _ => "playlist",
            };
            pool.cards.push(BrowseItem {
                kind,
                id: it.id,
                title: it.title,
                subtitle: it.subtitle,
                thumbnail: it.thumbnail,
                duration: None,
                album_id: None,
                artist_runs: Vec::new(),
                play_count: None,
                is_video: false,
                is_upload: false,
                explicit: false,
            });
        }
    }
    pool
}

/// How close `cand` is to the query, 0–1: strsim's Jaro-Winkler over the whole folded strings and
/// word by word (each query word against its best word of `cand`), the higher. A query held in
/// `cand` whole scores 1.
pub fn closeness(query: &str, cand: &str) -> f64 {
    let (q, c) = (fold(query), fold(cand));
    if q.is_empty() || c.is_empty() {
        return 0.0;
    }
    if q.chars().count() >= 3 && format!(" {c} ").contains(&format!(" {q} ")) {
        return 1.0;
    }
    let whole = strsim::jaro_winkler(&q, &c);
    let cw: Vec<&str> = c.split(' ').collect();
    let words: Vec<f64> = q
        .split(' ')
        .map(|w| cw.iter().map(|x| strsim::jaro_winkler(w, x)).fold(0.0, f64::max))
        .collect();
    let per_word = words.iter().sum::<f64>() / words.len() as f64;
    whole.max(per_word)
}

/// Close enough to show as "from your library".
pub const LOCAL_MATCH: f64 = 0.86;

/// Library matches for the query, best first.
pub fn local_matches(pool: &LocalPool, q: &str) -> (Vec<SongItem>, Vec<BrowseItem>) {
    let mut songs: Vec<(f64, SongItem)> = pool
        .songs
        .iter()
        .map(|s| {
            let score = closeness(q, &s.title)
                .max(closeness(q, &s.artists))
                .max(closeness(q, &format!("{} {}", s.title, s.artists)));
            (score, s.clone())
        })
        .filter(|(sc, _)| *sc >= LOCAL_MATCH)
        .collect();
    songs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut cards: Vec<(f64, BrowseItem)> = pool
        .cards
        .iter()
        .map(|c| (closeness(q, &c.title), c.clone()))
        .filter(|(sc, _)| *sc >= LOCAL_MATCH)
        .collect();
    cards.sort_by(|a, b| b.0.total_cmp(&a.0));
    (songs.into_iter().map(|x| x.1).take(50).collect(), cards.into_iter().map(|x| x.1).take(20).collect())
}

fn artist_card_of(t: &crate::stats::TopArtist) -> Option<BrowseItem> {
    Some(BrowseItem {
        kind: "artist",
        id: t.id.clone()?,
        title: t.name.clone(),
        subtitle: Some("Artist".into()),
        thumbnail: t.thumbnail.clone(),
        duration: None,
        album_id: None,
        artist_runs: Vec::new(),
        play_count: None,
        is_video: false,
        is_upload: false,
        explicit: false,
    })
}

/// Step 4: the user's most played songs and artists; with no history yet, YouTube's chart.
pub async fn try_these(state: &Arc<AppState>, offline: bool) -> TryThese {
    let mut t = TryThese {
        songs: crate::api::on_repeat_songs(state).into_iter().take(10).collect(),
        artists: crate::api::top_artists(state, 8).iter().filter_map(artist_card_of).collect(),
    };
    if t.songs.len() < 5 {
        let conn = state.db.conn();
        let have: std::collections::HashSet<String> = t.songs.iter().map(|s| s.video_id.clone()).collect();
        let more = crate::library::recent_tracks(&conn, 30)
            .into_iter()
            .map(|r| r.song)
            .chain(crate::library::liked_songs(&conn).into_iter().map(|e| e.song))
            .chain(if offline { crate::downloads::store::done_songs(&conn) } else { Vec::new() })
            .filter(|s| !have.contains(&s.video_id));
        t.songs.extend(more.take(10 - t.songs.len()));
    }
    if t.is_empty() && !offline {
        if let Ok(c) = crate::podcasts::api::client(state) {
            if let Ok(page) = state.it.charts(c, None).await {
                t = chart_try_these(&page);
                // The chart's songs are a playlist ("Top 100 Songs", else "Trending").
                let list = page.highlights.top_songs.as_ref().or(page.highlights.trending.as_ref());
                if let Some(pl) = list {
                    if let Ok(p) = state.it.playlist(c, &pl.id, None).await {
                        t.songs.extend(p.items.into_iter().take(10));
                    }
                }
            }
        }
    }
    t
}

/// "Try these" from YouTube's charts page: its top artists (and any song rows). The songs come
/// from the page's "Top songs" playlist, a request of its own.
pub fn chart_try_these(page: &innertube::explore::ChartsPage) -> TryThese {
    let mut t = TryThese::default();
    for s in &page.sections {
        for e in &s.items {
            match e.item.kind {
                "artist" if t.artists.len() < 8 => t.artists.push(e.item.clone()),
                "song" if t.songs.len() < 10 => t.songs.push(card_song(&e.item)),
                _ => {}
            }
        }
    }
    t
}

/// A song card (chart row) as a playable track.
fn card_song(c: &BrowseItem) -> SongItem {
    SongItem {
        video_id: c.id.clone(),
        title: c.title.clone(),
        artists: c.subtitle.clone().unwrap_or_default(),
        artist_runs: c.artist_runs.clone(),
        album_id: c.album_id.clone(),
        duration: c.duration.clone(),
        thumbnail: c.thumbnail.clone(),
        play_count: c.play_count.clone(),
        explicit: c.explicit,
        is_video: c.is_video,
        ..Default::default()
    }
}

// ─── songs by lyrics (stream: lyricsearch, blended per docs/streams/lyricsearch.md) ────────────────

/// A lyric match as the UI holds it between [`search_lyrics`] and [`blend_lyrics`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricMatch {
    pub song: SongItem,
    pub score: f64,
}

/// Songs whose words match the query (`lyricsearch::search_by_lyrics`, at most 1.2 s, never an
/// error). Empty at once when the query doesn't read like lyrics (`looks_like_lyrics`). The UI
/// calls it alongside [`search_overview`] on a settled or submitted query and never waits on it.
pub async fn search_lyrics(state: &Arc<AppState>, query: String) -> Vec<LyricMatch> {
    let q = query.trim();
    if q.is_empty() || !crate::lyricsearch::looks_like_lyrics(q) {
        return Vec::new();
    }
    let out: Vec<LyricMatch> = crate::lyricsearch::search_by_lyrics(state, q)
        .await
        .into_iter()
        .map(|c| LyricMatch { song: c.song, score: c.score })
        .collect();
    tracing::info!(query = %q, matches = out.len(), lead = ?out.first().map(|m| (m.song.title.clone(), m.score)), "search: lyrics");
    out
}

/// The Songs list with the lyric matches blended in.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricBlend {
    pub songs: Vec<SongItem>,
    /// `songs[0]` is a lyric match that becomes the Top result.
    pub lead_top: bool,
    /// The lyric rows that were added (video ids).
    pub added: Vec<String>,
}

/// At most this many lyric rows join the Songs list.
pub const MAX_LYRIC_ROWS: usize = 3;

/// The query names the first song's title: its base title is a whole phrase of the query and
/// has at least half as many words (lyricsearch.md step 3). "home" inside a lyric line is not.
pub fn title_strong(q: &str, top: &SongItem) -> bool {
    title_strong_str(q, &top.title, &top.artists)
}

fn title_strong_str(q: &str, title: &str, artists: &str) -> bool {
    let base = crate::identity::parse(title, artists, None).base;
    let nq = crate::identity::norm(q);
    let (bw, qw) = (base.split_whitespace().count(), nq.split_whitespace().count());
    !base.is_empty() && format!(" {nq} ").contains(&format!(" {base} ")) && bw * 2 >= qw
}

/// lyricsearch.md steps 3–5. `interacted`: the normal results are on screen and the user has
/// touched them, so the matches are only appended below. Each song once (video id, then the
/// same work): the earlier slot wins. `top_title`: the page's Top result when it isn't a song
/// (an album, artist or playlist card); one titled like the query is a strong title match too, so
/// "to pimp a butterfly" keeps its album rather than a lyric hit from it.
pub fn blend_lyrics(
    q: &str,
    songs: Vec<SongItem>,
    lyric: Vec<LyricMatch>,
    interacted: bool,
    top_title: Option<&str>,
) -> LyricBlend {
    let Some(lead) = lyric.first() else { return LyricBlend { songs, ..Default::default() } };
    let top = songs.first();
    if let Some(t) = top {
        if crate::identity::work_key_song(t) == crate::identity::work_key_song(&lead.song) {
            return LyricBlend { songs, ..Default::default() };
        }
    }
    let strong = top.is_some_and(|t| title_strong(q, t)) || top_title.is_some_and(|t| title_strong_str(q, t, ""));
    let picks: Vec<SongItem> = lyric.iter().take(MAX_LYRIC_ROWS).map(|m| m.song.clone()).collect();
    let (at, lead_top) = if interacted {
        (songs.len(), false)
    } else if !strong && lead.score >= crate::lyricsearch::STRONG {
        (0, true)
    } else if !strong {
        (songs.len().min(1), false)
    } else {
        (songs.len().min(3), false)
    };
    let mut merged = songs.clone();
    merged.splice(at..at, picks.iter().cloned());
    let mut ids = std::collections::HashSet::new();
    let mut works = std::collections::HashSet::new();
    merged.retain(|s| ids.insert(s.video_id.clone()) && works.insert(crate::identity::work_key_song(s)));
    let had: std::collections::HashSet<&str> = songs.iter().map(|s| s.video_id.as_str()).collect();
    let added: Vec<String> =
        picks.iter().filter(|p| !had.contains(p.video_id.as_str()) && merged.iter().any(|m| m.video_id == p.video_id)).map(|p| p.video_id.clone()).collect();
    let lead_top = lead_top && merged.first().is_some_and(|m| m.video_id == lead.song.video_id);
    LyricBlend { songs: merged, lead_top, added }
}

// ─── the All view's order ───────────────────────────────────────────────────────────────────────

/// Podcasts before music: a show is the Top result, or the first show is the query's own popular
/// show (`podcasts::search::leads_popular`: its title or host is the query, or it charts and the
/// query is its title's words). `results` is YouTube's answer: a strong music card there (an
/// artist named like the query with an audience, a song or album titled like it) keeps the music
/// first against a show that only shares a word.
///
/// stream: search-pop. Before, any show folded from both sources with an exact title counted, so
/// "drake" put a one-episode "Drake Podcast" and twenty namesakes above Drake's songs.
pub fn podcasts_first(query: &str, top_show: Option<usize>, shows: &[Show], results: &SearchResults) -> bool {
    use podcasts::search::{leads_popular, strong_music_for_shelf};
    let strong = r_cards(results).any(|c| strong_music_for_shelf(query, c));
    top_show.is_some() || shows.first().is_some_and(|s| leads_popular(query, s, strong))
}

/// YouTube's Top card and first artist card.
fn r_cards(r: &SearchResults) -> impl Iterator<Item = &BrowseItem> {
    r.top.first().into_iter().chain(r.artists.first())
}

/// One row of the All view, as the page lays it out (for the benchmark: what a user sees, in
/// order).
#[derive(Debug, Clone)]
pub enum ViewRow<'a> {
    Card(&'a BrowseItem),
    Song(&'a SongItem),
    Show(&'a Show),
}

/// The All view top to bottom, as `routes/search/+page.svelte` (and the iOS Top list) lay it out:
/// the Top result; the shows when [`SearchOverview::podcasts_first`]; four songs; two albums, two
/// more songs and two artists interleaved; then the Artists, Albums and Podcasts shelves.
/// Each entity once.
pub fn all_view_order(ov: &SearchOverview) -> Vec<ViewRow<'_>> {
    let r = &ov.results;
    let mut out: Vec<ViewRow> = Vec::new();
    out.extend(top_row(ov));
    if ov.podcasts_first {
        out.extend(ov.shows.iter().map(ViewRow::Show));
    }
    out.extend(ov.songs.iter().take(4).map(ViewRow::Song));
    for i in 0..2 {
        out.extend(r.albums.get(i).map(ViewRow::Card));
        out.extend(ov.songs.get(4 + i).map(ViewRow::Song));
        out.extend(r.artists.get(i).map(ViewRow::Card));
    }
    out.extend(r.artists.iter().map(ViewRow::Card));
    out.extend(r.albums.iter().map(ViewRow::Card));
    out.extend(r.playlists.iter().map(ViewRow::Card));
    if !ov.podcasts_first {
        out.extend(ov.shows.iter().map(ViewRow::Show));
    }
    if let Some(t) = &ov.try_these {
        out.extend(t.songs.iter().map(ViewRow::Song));
        out.extend(t.artists.iter().map(ViewRow::Card));
    }
    once_each(out)
}

/// How many rows the dropdown lists (Spotify shows about 30).
pub const DROPDOWN_ROWS: usize = 30;

/// The top bar's dropdown, top to bottom: the Top result (the person's shows next, when
/// [`SearchOverview::podcasts_first`]), four songs, then rounds of one artist, one album, two songs,
/// one playlist and one show until [`DROPDOWN_ROWS`]. "Try these" stands in for an empty answer,
/// so the dropdown is never empty either. Each entity once.
pub fn dropdown_order(ov: &SearchOverview) -> Vec<ViewRow<'_>> {
    let r = &ov.results;
    let mut out: Vec<ViewRow> = Vec::new();
    out.extend(top_row(ov));
    if ov.podcasts_first {
        out.extend(ov.shows.iter().take(3).map(ViewRow::Show));
    }
    out.extend(ov.songs.iter().take(4).map(ViewRow::Song));
    let mut songs = ov.songs.iter().skip(4);
    let (mut artists, mut albums, mut lists, mut shows) = (r.artists.iter(), r.albums.iter(), r.playlists.iter(), ov.shows.iter());
    loop {
        let before = out.len();
        out.extend(artists.next().map(ViewRow::Card));
        out.extend(albums.next().map(ViewRow::Card));
        out.extend(songs.next().map(ViewRow::Song));
        out.extend(songs.next().map(ViewRow::Song));
        out.extend(lists.next().map(ViewRow::Card));
        out.extend(shows.next().map(ViewRow::Show));
        if out.len() == before || out.len() >= 2 * DROPDOWN_ROWS {
            break;
        }
    }
    if let Some(t) = &ov.try_these {
        out.extend(t.songs.iter().map(ViewRow::Song));
        out.extend(t.artists.iter().map(ViewRow::Card));
    }
    let mut out = once_each(out);
    out.truncate(DROPDOWN_ROWS);
    out
}

fn top_row(ov: &SearchOverview) -> Option<ViewRow<'_>> {
    let r = &ov.results;
    match ov.top_show {
        Some(i) => ov.shows.get(i).map(ViewRow::Show),
        None => r.top.first().or(r.artists.first()).map(ViewRow::Card).or(ov.songs.first().map(ViewRow::Song)),
    }
}

fn once_each(mut rows: Vec<ViewRow<'_>>) -> Vec<ViewRow<'_>> {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| {
        let id = match row {
            ViewRow::Card(c) => c.id.clone(),
            ViewRow::Song(s) => s.video_id.clone(),
            ViewRow::Show(s) => s.id.clone(),
        };
        seen.insert(id)
    });
    rows
}

impl From<ViewRow<'_>> for DropRow {
    fn from(row: ViewRow<'_>) -> Self {
        match row {
            ViewRow::Card(c) => DropRow::Card { item: c.clone() },
            ViewRow::Song(s) => DropRow::Song { song: s.clone() },
            ViewRow::Show(s) => DropRow::Show { show: s.clone() },
        }
    }
}

// ─── commands ────────────────────────────────────────────────────────────────────────────────────

/// After the music answer, how long the podcast directories may still take before the All view
/// paints without them (the UI then asks `podcast_search` on its own).
const PODS_GRACE: Duration = Duration::from_millis(400);

/// The All view. See the module docs.
pub async fn search_overview(
    state: &Arc<AppState>,
    query: String,
    record_history: bool,
) -> Result<SearchOverview, String> {
    let mut ov = overview_rows(state, query, record_history).await?;
    ov.dropdown = dropdown_order(&ov).into_iter().map(DropRow::from).collect();
    Ok(ov)
}

async fn overview_rows(
    state: &Arc<AppState>,
    query: String,
    record_history: bool,
) -> Result<SearchOverview, String> {
    let q = query.trim().to_owned();
    if q.is_empty() {
        return Ok(SearchOverview::default());
    }
    let offline = crate::downloads::offline_mode();
    if offline {
        return Ok(offline_overview(state, &q).await);
    }
    let backend = Live { state, record_history };
    let music = resolve(&backend, &q);
    let pods = podcasts::api::podcast_search(state, q.clone(), None);
    tokio::pin!(music, pods);
    let mut pods_out = None;
    let music_out = loop {
        tokio::select! {
            m = &mut music => break m,
            p = &mut pods, if pods_out.is_none() => pods_out = Some(p),
        }
    };
    if pods_out.is_none() {
        pods_out = tokio::time::timeout(PODS_GRACE, &mut pods).await.ok();
    }
    let r = match music_out {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "search: YouTube unreachable, searching the library");
            return Ok(offline_overview(state, &q).await);
        }
    };
    let pods_pending = pods_out.is_none();
    let (shows, episodes) = match pods_out {
        Some(Ok(p)) => (p.shows, p.episodes),
        _ => (Vec::new(), Vec::new()),
    };
    let mut songs = r.songs;
    // Lyrics are not awaited here: the UI runs `search_lyrics` alongside and blends late. Only
    // an otherwise empty page waits for them (the never-empty chain, below).
    if songs.is_empty() && r.results.top.is_empty() && crate::lyricsearch::looks_like_lyrics(&q) {
        songs = search_lyrics(state, q.clone()).await.into_iter().map(|m| m.song).collect();
    }
    crate::smart::explicit::remember_rows(&state.db, &songs, true);
    let top_show = podcasts::search::top_show(&r.query, r.results.top.first(), r.results.artists.first(), &shows);
    let mut ov = SearchOverview {
        query: r.query,
        correction: r.correction,
        results: r.results,
        songs,
        songs_continuation: r.songs_continuation,
        shows,
        episodes,
        pods_pending,
        top_show,
        podcasts_first: false,
        try_these: None,
        dropdown: Vec::new(),
    };
    ov.podcasts_first = podcasts_first(&ov.query, ov.top_show, &ov.shows, &ov.results);
    if ov.songs.is_empty() && ov.results.top.is_empty() && ov.results.artists.is_empty() && ov.results.albums.is_empty() && ov.results.playlists.is_empty() {
        // Step 3: the library; step 4: "Try these".
        let pool = local_pool(state);
        let (ls, lc) = local_matches(&pool, &q);
        if !ls.is_empty() || !lc.is_empty() {
            ov.correction = Some(Correction { kind: CorrectionKind::Library, query: q.clone(), original: q.clone() });
            ov.songs = ls;
            ov.results.artists = lc.iter().filter(|c| c.kind == "artist").cloned().collect();
            ov.results.albums = lc.iter().filter(|c| c.kind == "album").cloned().collect();
            ov.results.playlists = lc.into_iter().filter(|c| c.kind == "playlist").collect();
        }
        if ov.songs.is_empty() && ov.shows.is_empty() && ov.episodes.is_empty() && ov.results.artists.is_empty() {
            ov.try_these = Some(try_these(state, false).await);
        }
    }
    tracing::info!(query = %q, answered = %ov.query, songs = ov.songs.len(), shows = ov.shows.len(),
        top_show = ?ov.top_show.map(|i| ov.shows[i].title.clone()), correction = ?ov.correction.as_ref().map(|c| c.kind),
        "search: overview");
    Ok(ov)
}

/// No network: the library and the downloads, fuzzy-matched; "Try these" from them otherwise.
async fn offline_overview(state: &Arc<AppState>, q: &str) -> SearchOverview {
    let pool = local_pool(state);
    let (songs, cards) = local_matches(&pool, q);
    let mut ov = SearchOverview {
        query: q.to_owned(),
        correction: Some(Correction { kind: CorrectionKind::Offline, query: q.to_owned(), original: q.to_owned() }),
        songs,
        ..Default::default()
    };
    ov.results.artists = cards.iter().filter(|c| c.kind == "artist").cloned().collect();
    ov.results.albums = cards.iter().filter(|c| c.kind == "album").cloned().collect();
    ov.results.playlists = cards.into_iter().filter(|c| c.kind == "playlist").collect();
    if ov.songs.is_empty() && ov.results.artists.is_empty() && ov.results.albums.is_empty() && ov.results.playlists.is_empty() {
        ov.try_these = Some(try_these(state, true).await);
    }
    ov
}

/// One page of a chip. `filter` is the chip (`songs`, `videos`, `albums`, `artists`, `playlists`,
/// `profiles`, `podcasts`, `episodes`); `continuation` the previous page's token (with the same
/// `query` that page returned). A first page that comes back empty runs the same fallbacks as the
/// All view, so a chip is never empty either.
pub async fn search_page(
    state: &Arc<AppState>,
    query: String,
    filter: String,
    continuation: Option<String>,
) -> Result<SearchPage, String> {
    let f = SearchFilter::from_name(&filter).ok_or_else(|| format!("unknown search filter: {filter}"))?;
    let q = query.trim().to_owned();
    if q.is_empty() {
        return Ok(SearchPage { filter, ..Default::default() });
    }
    if crate::downloads::offline_mode() {
        return Ok(offline_page(state, &q, filter, f).await);
    }
    let first = continuation.is_none();
    let page = match page_once(state, &q, f, continuation.as_deref(), first).await {
        Ok(p) => p,
        Err(e) if first => {
            tracing::warn!(error = %e, "search: chip unreachable, searching the library");
            return Ok(offline_page(state, &q, filter, f).await);
        }
        Err(e) => return Err(e),
    };
    let mut page = SearchPage { filter: filter.clone(), query: q.clone(), ..page };
    tracing::info!(query = %q, filter = %filter, rows = page.len(), first, more = page.continuation.is_some(), "search: page");
    if !first || page.len() > 0 {
        return Ok(page);
    }
    // Fallbacks for an empty first page: YouTube's correction, the typeahead, relaxed words.
    let mut tries: Vec<(CorrectionKind, String)> = Vec::new();
    if let Some(c) = page.correction.take() {
        tries.push((c.kind, c.query));
    }
    let backend = Live { state, record_history: false };
    if let Some(c) = completion(&q, &backend.suggest(&q).await.unwrap_or_default()) {
        tries.push((CorrectionKind::Completion, c));
    }
    tries.extend(relaxations(&q).into_iter().map(|r| (CorrectionKind::Relaxed, r)));
    for (kind, next) in tries.into_iter().take(MAX_RETRIES) {
        if let Ok(p) = page_once(state, &next, f, None, true).await {
            if p.len() > 0 {
                return Ok(SearchPage {
                    filter,
                    query: next.clone(),
                    correction: Some(Correction { kind, query: next, original: q }),
                    ..p
                });
            }
        }
    }
    // Library, then "Try these".
    let pool = local_pool(state);
    let (songs, cards) = local_matches(&pool, &q);
    if f.is_tracks() && !songs.is_empty() {
        page.songs = songs;
    } else if !f.is_tracks() && !cards.is_empty() {
        page.items = cards;
    }
    if page.len() > 0 {
        page.correction = Some(Correction { kind: CorrectionKind::Library, query: q.clone(), original: q });
    } else {
        page.try_these = Some(try_these(state, false).await);
    }
    Ok(page)
}

async fn offline_page(state: &Arc<AppState>, q: &str, filter: String, f: SearchFilter) -> SearchPage {
    let pool = local_pool(state);
    let (songs, cards) = local_matches(&pool, q);
    let mut page = SearchPage {
        filter,
        query: q.to_owned(),
        correction: Some(Correction { kind: CorrectionKind::Offline, query: q.to_owned(), original: q.to_owned() }),
        ..Default::default()
    };
    match f {
        SearchFilter::Songs | SearchFilter::Videos => page.songs = songs,
        SearchFilter::Artists => page.items = cards.into_iter().filter(|c| c.kind == "artist").collect(),
        SearchFilter::Albums => page.items = cards.into_iter().filter(|c| c.kind == "album").collect(),
        SearchFilter::Playlists | SearchFilter::CommunityPlaylists | SearchFilter::FeaturedPlaylists => {
            page.items = cards.into_iter().filter(|c| c.kind == "playlist").collect()
        }
        _ => {}
    }
    if page.len() == 0 {
        page.try_these = Some(try_these(state, true).await);
    }
    page
}

/// One request (or, for a first podcasts / episodes page, YouTube plus the directories).
async fn page_once(
    state: &Arc<AppState>,
    q: &str,
    f: SearchFilter,
    continuation: Option<&str>,
    first: bool,
) -> Result<SearchPage, String> {
    let c = crate::podcasts::api::client(state)?;
    let it = &state.it;
    let fix_of = |s: Option<SpellFix>| {
        s.filter(|f| f.applied).map(|f| Correction { kind: CorrectionKind::Spelling, query: f.query, original: q.to_owned() })
    };
    let err = |e: innertube::Error| e.to_string();
    match f {
        SearchFilter::Songs | SearchFilter::Videos => {
            let p = it.search_tracks_page(c, q, f, continuation).await.map_err(err)?;
            let songs = if first && f == SearchFilter::Songs {
                crate::smart::recs::rerank_search_songs(&state.db, q, p.items)
            } else {
                crate::smart::explicit::remember_rows(&state.db, &p.items, true);
                p.items
            };
            Ok(SearchPage { songs, continuation: p.continuation, correction: fix_of(p.spell_fix), ..Default::default() })
        }
        SearchFilter::Podcasts => {
            let yt = async {
                let v = it.search_page_json(c, q, f, continuation).await.map_err(err)?;
                Ok::<_, String>((podcasts::youtube::parse_show_search(innertube::search::page_rows(&v)), innertube::search::next_token(&v), innertube::search::spell_fix(&v)))
            };
            let dir = async {
                if !first {
                    return Vec::new();
                }
                let (a, b) = tokio::join!(podcasts::itunes::search_shows(&state.db, q), podcasts::itunes::search_podcastindex(&state.db, q));
                let mut d = a.unwrap_or_default();
                d.extend(b.unwrap_or_default());
                d
            };
            let chart = async { if first { podcasts::charts::top(&state.db).await } else { Default::default() } };
            let (yt, dir, chart) = tokio::join!(yt, dir, chart);
            let (mut shows, next, fix) = match yt {
                Ok(x) => x,
                Err(e) if dir.is_empty() => return Err(e),
                Err(_) => (Vec::new(), None, None),
            };
            if first {
                let mut merged = podcasts::search::merge_shows(shows, dir);
                podcasts::charts::apply(&mut merged, &chart);
                shows = podcasts::search::rank_shows(q, merged);
            }
            podcasts::api::mark_followed(state, &mut shows);
            Ok(SearchPage { shows, continuation: next, correction: fix_of(fix), ..Default::default() })
        }
        SearchFilter::Episodes => {
            let now = podcasts::now_secs();
            let yt = async {
                let v = it.search_page_json(c, q, f, continuation).await.map_err(err)?;
                Ok::<_, String>((podcasts::youtube::parse_episode_search(innertube::search::page_rows(&v), now), innertube::search::next_token(&v), innertube::search::spell_fix(&v)))
            };
            let dir = async {
                if first { podcasts::itunes::search_episodes(&state.db, q).await.unwrap_or_default() } else { Vec::new() }
            };
            let (yt, dir) = tokio::join!(yt, dir);
            let (mut eps, next, fix) = match yt {
                Ok(x) => x,
                Err(e) if dir.is_empty() => return Err(e),
                Err(_) => (Vec::new(), None, None),
            };
            if first {
                eps = podcasts::search::merge_episodes(eps, dir);
            }
            podcasts::api::decorate(state, &mut eps);
            Ok(SearchPage { episodes: eps, continuation: next, correction: fix_of(fix), ..Default::default() })
        }
        _ => {
            let p = it.search_cards_page(c, q, f, continuation).await.map_err(err)?;
            let mut items = p.items;
            if f == SearchFilter::Playlists {
                // Shows and episodes have their own chips.
                items.retain(|i| i.kind == "playlist");
            }
            Ok(SearchPage { items, continuation: p.continuation, correction: fix_of(p.spell_fix), ..Default::default() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn song(id: &str, title: &str, artists: &str) -> SongItem {
        serde_json::from_value(serde_json::json!({ "video_id": id, "title": title, "artists": artists })).unwrap()
    }

    #[test]
    fn dropdown_mixes_thirty_rows_in_spotify_order() {
        let card = |kind: &'static str, id: &str| BrowseItem {
            kind,
            id: id.into(),
            title: id.into(),
            subtitle: None,
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: vec![],
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
        };
        let mut ov = SearchOverview::default();
        ov.results.top = vec![card("artist", "ar0")];
        ov.results.artists = (0..5).map(|i| card("artist", &format!("ar{i}"))).collect();
        ov.results.albums = (0..5).map(|i| card("album", &format!("al{i}"))).collect();
        ov.results.playlists = (0..5).map(|i| card("playlist", &format!("pl{i}"))).collect();
        ov.songs = (0..20).map(|i| song(&format!("s{i}"), "t", "a")).collect();
        ov.shows = (0..5).map(|i| Show { id: format!("sh{i}"), ..Default::default() }).collect();
        let ids: Vec<String> = dropdown_order(&ov)
            .into_iter()
            .map(|r| match r {
                ViewRow::Card(c) => c.id.clone(),
                ViewRow::Song(s) => s.video_id.clone(),
                ViewRow::Show(s) => s.id.clone(),
            })
            .collect();
        assert_eq!(ids.len(), DROPDOWN_ROWS);
        // Top result, four songs, then rounds (the top artist is not repeated).
        assert_eq!(&ids[..12], ["ar0", "s0", "s1", "s2", "s3", "al0", "s4", "s5", "pl0", "sh0", "ar1", "al1"]);
        let uniq: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(uniq.len(), ids.len());

        // A person's show leads, their shows follow, music after.
        ov.top_show = Some(2);
        ov.podcasts_first = true;
        let first: Vec<String> = dropdown_order(&ov).into_iter().take(4).map(|r| match r {
            ViewRow::Show(s) => s.id.clone(),
            ViewRow::Song(s) => s.video_id.clone(),
            ViewRow::Card(c) => c.id.clone(),
        }).collect();
        assert_eq!(first, ["sh2", "sh0", "sh1", "s0"]);

        // Never empty: "Try these" stands in.
        let empty = SearchOverview {
            try_these: Some(TryThese { songs: vec![song("t1", "x", "y")], artists: vec![card("artist", "a1")] }),
            ..Default::default()
        };
        assert_eq!(dropdown_order(&empty).len(), 2);
        let json = serde_json::to_value(DropRow::from(dropdown_order(&empty).remove(0))).unwrap();
        assert_eq!(json["type"], "song");
        assert_eq!(json["song"]["video_id"], "t1");
    }

    fn dd_card(kind: &'static str, id: &str, title: &str) -> BrowseItem {
        BrowseItem {
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: None,
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: vec![],
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
        }
    }

    /// stream: search-pop. A hit song that answers the typed text better than YouTube's Top card
    /// takes the Top result ("blinding": Blinding Lights over a new album "BLINDED").
    #[test]
    fn a_hit_song_takes_the_place_of_a_weaker_top_card() {
        let album = BrowseItem { subtitle: Some("Album • Sbhekzin • 2026".into()), ..dd_card("album", "MPREx", "BLINDED") };
        let hit = BrowseItem {
            subtitle: Some("Song • The Weeknd • 3.7B plays".into()),
            play_count: Some("3.7B".into()),
            ..dd_card("song", "v1", "Blinding Lights")
        };
        let r = SearchResults { top: vec![album.clone()], songs: vec![hit.clone()], ..Default::default() };
        assert_eq!(crate::identity::rerank_search_all("blinding", r.clone()).top[0].id, "v1");
        // Not over a card titled like the query, nor for a song that isn't a hit.
        let single = BrowseItem { subtitle: Some("Single • X • 2025".into()), ..dd_card("album", "MPREy", "Blinding Lights") };
        let r2 = SearchResults { top: vec![single], songs: vec![hit.clone()], ..Default::default() };
        assert_eq!(crate::identity::rerank_search_all("blinding", r2).top[0].id, "MPREy");
        let small = BrowseItem { play_count: Some("2M".into()), ..hit };
        let r3 = SearchResults { top: vec![album], songs: vec![small], ..Default::default() };
        assert_eq!(crate::identity::rerank_search_all("blinding", r3).top[0].id, "MPREx");
    }

    /// stream: search-pop. YouTube's Top card names the artist a partial query means: "billie" is
    /// Billie Eilish (331M audience), not the K-pop group Billlie one typo away.
    #[test]
    fn the_top_card_s_artist_leads_the_songs() {
        let mut billlie = song("b1", "GingaMingaYo", "Billlie");
        billlie.play_count = Some("89M".into());
        let mut bad_guy = song("e1", "bad guy", "Billie Eilish");
        bad_guy.play_count = Some("2.4B".into());
        let mut thirty = song("e2", "The 30th", "Billie Eilish");
        thirty.play_count = Some("91M".into());
        let ranked = vec![billlie, thirty, bad_guy];
        let card = BrowseItem { subtitle: Some("Artist • 331M monthly audience".into()), ..dd_card("artist", "UCx", "Billie Eilish") };
        let results = SearchResults { top: vec![card.clone()], ..Default::default() };
        let out = songs_for("billie", ranked.clone(), &results, None);
        assert_eq!(out.iter().map(|s| s.video_id.as_str()).collect::<Vec<_>>(), ["e1", "e2", "b1"]);
        // A small artist card is no evidence: the list is the row rule's, as before.
        let small = BrowseItem { subtitle: Some("Artist • 1 subscriber".into()), ..card };
        let before = songs_for("billie", ranked.clone(), &SearchResults::default(), None);
        assert_eq!(songs_for("billie", ranked, &SearchResults { top: vec![small], ..Default::default() }, None)[0].video_id, before[0].video_id);
    }

    #[test]
    fn relaxes_the_least_informative_word_first() {
        assert_eq!(relaxations("hello adele"), vec!["hello"]);
        assert_eq!(relaxations("bohemian rhapsody lyrics")[0], "bohemian rhapsody");
        // A keyboard mash goes before real words.
        assert_eq!(relaxations("taylor xqzvwpl swift")[0], "taylor swift");
        assert_eq!(relaxations("adele"), Vec::<String>::new());
        let r = relaxations("the a b");
        assert!(!r.iter().any(|x| x.is_empty()));
    }

    #[test]
    fn local_matching_is_fuzzy() {
        assert!(closeness("bohemain rapsody", "Bohemian Rhapsody") >= LOCAL_MATCH);
        assert!(closeness("beyonse", "Beyoncé") >= LOCAL_MATCH);
        assert!(closeness("daft", "Daft Punk") >= LOCAL_MATCH);
        assert!(closeness("xqzvwpl", "Bohemian Rhapsody") < LOCAL_MATCH);
        let pool = LocalPool {
            songs: vec![song("a", "Bohemian Rhapsody", "Queen"), song("b", "Halo", "Beyoncé")],
            cards: vec![],
        };
        let (s, _) = local_matches(&pool, "beyonse");
        assert_eq!(s.iter().map(|x| x.video_id.as_str()).collect::<Vec<_>>(), ["b"]);
    }

    fn lm(id: &str, title: &str, artists: &str, score: f64) -> LyricMatch {
        LyricMatch { song: song(id, title, artists), score }
    }

    #[test]
    fn lyrics_blend_by_the_recipe() {
        let normal = || vec![song("a", "Home", "X"), song("b", "B", "x"), song("c", "C", "x"), song("d", "D", "x")];
        let ids = |b: &LyricBlend| b.songs.iter().map(|s| s.video_id.as_str()).collect::<Vec<_>>().join("");
        let q = "i walked along the empty road back home"; // invented
        // Weak title match + strong lead: the lead is the Top result, at most 3 lyric rows.
        let lyric = vec![lm("l1", "Road", "Y", 1.2), lm("l2", "Way", "Z", 0.5), lm("l3", "P", "W", 0.4), lm("l4", "Q", "V", 0.3)];
        let b = blend_lyrics(q, normal(), lyric.clone(), false, None);
        assert!(b.lead_top);
        assert_eq!(ids(&b), "l1l2l3abcd");
        // Weak lead: row 2.
        let mut weak = lyric.clone();
        weak[0].score = 0.6;
        assert_eq!(ids(&blend_lyrics(q, normal(), weak, false, None)), "al1l2l3bcd");
        // The title matched: a whole phrase of the query with at least half its words.
        let titled = vec![song("t", "Empty Road", "X"), song("b", "B", "x"), song("c", "C", "x"), song("d", "D", "x")];
        assert_eq!(ids(&blend_lyrics("empty road walking", titled, lyric.clone(), false, None)), "tbcl1l2l3d");
        // The user already touched the list: appended quietly.
        let b = blend_lyrics(q, normal(), lyric.clone(), true, None);
        assert!(!b.lead_top);
        assert_eq!(ids(&b), "abcdl1l2l3");
        // Agreement (same song on top): unchanged. Duplicates keep the earlier slot.
        assert_eq!(ids(&blend_lyrics(q, normal(), vec![lm("a2", "Home", "X", 1.3)], false, None)), "abcd");
        assert_eq!(ids(&blend_lyrics(q, normal(), vec![lm("c", "C", "x", 1.3), lm("l2", "Way", "Z", 0.5)], false, None)), "cl2abd");
        assert_eq!(blend_lyrics(q, normal(), vec![], false, None).songs.len(), 4);
        assert!(!title_strong(q, &song("h", "Home", "X")));
        assert!(title_strong("hello adele", &song("h", "Hello", "Adele")));
        // An album card titled like the query is a strong title match: no lyric lead over it.
        let b = blend_lyrics("empty road walking", normal(), lyric.clone(), false, Some("Empty Road Walking"));
        assert!(!b.lead_top);
        assert_eq!(ids(&b), "abcl1l2l3d");
    }

    /// A scripted backend: per query, an answer (or an empty one), and a record of every call.
    #[derive(Default)]
    struct Script {
        all: HashMap<String, SearchResults>,
        songs: HashMap<String, Page<SongItem>>,
        fix: HashMap<String, SpellFix>,
        suggest: HashMap<String, Vec<String>>,
        down: bool,
        calls: Mutex<Vec<String>>,
    }
    impl Backend for Script {
        async fn all(&self, q: &str) -> Result<AllAnswer, String> {
            self.calls.lock().unwrap().push(format!("all:{q}"));
            if self.down {
                return Err("offline".into());
            }
            Ok(AllAnswer { results: self.all.get(q).cloned().unwrap_or_default(), fix: self.fix.get(q).cloned(), artist_top: None })
        }
        async fn songs(&self, q: &str) -> Result<Page<SongItem>, String> {
            if self.down {
                return Err("offline".into());
            }
            Ok(self.songs.get(q).cloned().unwrap_or_default())
        }
        async fn suggest(&self, q: &str) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push(format!("suggest:{q}"));
            Ok(self.suggest.get(q).cloned().unwrap_or_default())
        }
    }
    fn page(songs: Vec<SongItem>) -> Page<SongItem> {
        Page { items: songs, continuation: Some("t".into()), spell_fix: None }
    }

    #[tokio::test]
    async fn the_normal_path_makes_no_extra_requests() {
        let mut s = Script::default();
        s.songs.insert("queen".into(), page(vec![song("a", "Bohemian Rhapsody", "Queen")]));
        let r = resolve(&s, "queen").await.unwrap();
        assert_eq!(r.songs.len(), 1);
        assert!(r.correction.is_none());
        assert_eq!(*s.calls.lock().unwrap(), vec!["all:queen"]);
    }

    #[tokio::test]
    async fn youtube_s_applied_correction_is_reported() {
        let mut s = Script::default();
        s.songs.insert("bohemain".into(), page(vec![song("a", "Bohemian Rhapsody", "Queen")]));
        s.fix.insert("bohemain".into(), SpellFix { query: "bohemian".into(), applied: true });
        let r = resolve(&s, "bohemain").await.unwrap();
        assert_eq!(r.correction.unwrap(), Correction { kind: CorrectionKind::Spelling, query: "bohemian".into(), original: "bohemain".into() });
        assert_eq!(r.query, "bohemian");
    }

    #[tokio::test]
    async fn the_chain_runs_in_order_and_stops_at_the_first_answer() {
        // Did-you-mean (not applied, empty) → its query answers.
        let mut s = Script::default();
        s.fix.insert("tylr swft".into(), SpellFix { query: "taylor swift".into(), applied: false });
        s.songs.insert("taylor swift".into(), page(vec![song("a", "Love Story", "Taylor Swift")]));
        let r = resolve(&s, "tylr swft").await.unwrap();
        assert_eq!(r.correction.as_ref().unwrap().kind, CorrectionKind::Spelling);
        assert!(!s.calls.lock().unwrap().iter().any(|c| c.starts_with("suggest")), "stops before the typeahead");

        // No correction: the typeahead's completion.
        let mut s = Script::default();
        s.suggest.insert("metalli".into(), vec!["metallica".into()]);
        s.songs.insert("metallica".into(), page(vec![song("a", "One", "Metallica")]));
        let r = resolve(&s, "metalli").await.unwrap();
        assert_eq!(r.correction.as_ref().unwrap().kind, CorrectionKind::Completion);
        assert_eq!(r.query, "metallica");

        // Then a word dropped.
        let mut s = Script::default();
        s.songs.insert("daft punk".into(), page(vec![song("a", "One More Time", "Daft Punk")]));
        let r = resolve(&s, "daft punk qqqqzzz").await.unwrap();
        assert_eq!(r.correction.as_ref().unwrap().kind, CorrectionKind::Relaxed);
        assert_eq!(r.query, "daft punk");

        // Nothing anywhere: empty, bounded retries, no error.
        let s = Script::default();
        let r = resolve(&s, "a b c d e f").await.unwrap();
        assert!(r.is_empty());
        assert!(r.tried.len() <= 1 + MAX_RETRIES);
    }

    #[tokio::test]
    async fn unreachable_is_an_error_for_the_caller_to_answer_locally() {
        let s = Script { down: true, ..Default::default() };
        assert!(resolve(&s, "queen").await.is_err());
    }
}

/// Live check of the shipped pipeline on a throwaway core (no window, no account):
/// `cargo test -p encore-core --lib search::live -- --ignored --nocapture`.
#[cfg(test)]
mod live {
    use super::*;

    /// stream: search-dropdown. "queen": the dropdown's ~30 mixed rows, and the Songs chip endless.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "hits live YouTube Music and iTunes"]
    async fn queen_dropdown_and_songs() {
        let dir = std::env::temp_dir().join(format!("spotless-search-dropdown-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let host: Arc<dyn crate::host::Host> = Arc::new(crate::host::NullHost(dir.clone()));
        let st = crate::boot::start_core(host).map_err(|e| e.to_string()).unwrap().state.clone();
        let t = std::time::Instant::now();
        let ov = search_overview(&st, "queen".into(), false).await.unwrap();
        let rows: Vec<String> = ov.dropdown.iter().map(|d| match d {
            DropRow::Card { item } => format!("{}: {}", item.kind, item.title),
            DropRow::Song { song } => format!("song: {}", song.title),
            DropRow::Show { show } => format!("podcast: {}", show.title),
        }).collect();
        println!("queen ({} ms): {} dropdown rows\n  {}", t.elapsed().as_millis(), rows.len(), rows.join("\n  "));
        assert!(rows.len() >= 20, "dropdown too short");
        for kind in ["song: ", "artist: ", "album: "] {
            assert!(rows.iter().any(|r| r.starts_with(kind)), "no {kind}");
        }
        let mut ids: std::collections::HashSet<String> = ov.songs.iter().map(|s| s.video_id.clone()).collect();
        let (mut next, mut pages) = (ov.songs_continuation.clone(), 1);
        while let Some(tok) = next.take() {
            let p = search_page(&st, ov.query.clone(), "songs".into(), Some(tok)).await.unwrap();
            ids.extend(p.songs.iter().map(|s| s.video_id.clone()));
            pages += 1;
            next = p.continuation;
            if ids.len() >= 150 || pages >= 10 {
                break;
            }
        }
        println!("queen Songs chip: {} unique rows over {pages} pages", ids.len());
        assert!(ids.len() >= 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// stream: search-pop. Where the obviously meant popular result lands, live:
    /// `POP_Q="tucker,rogan" cargo test -p encore-core --lib search::live::pop_diagnosis -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "hits live YouTube Music and iTunes"]
    async fn pop_diagnosis() {
        let dir = std::env::temp_dir().join(format!("spotless-search-pop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let host: Arc<dyn crate::host::Host> = Arc::new(crate::host::NullHost(dir.clone()));
        let st = crate::boot::start_core(host).map_err(|e| e.to_string()).unwrap().state.clone();
        let qs = std::env::var("POP_Q").unwrap_or_else(|_| "tucker,rogan,huberman,lex,diary of a,mel robbins,smartless,blinding,taylor,drake,billie,shape of".into());
        let label = |r: &ViewRow| match r {
            ViewRow::Card(c) => format!("{}: {} [{}]", c.kind, c.title, c.subtitle.clone().unwrap_or_default()),
            ViewRow::Song(s) => format!("song: {} - {} [{}]", s.title, s.artists, s.play_count.clone().unwrap_or_default()),
            ViewRow::Show(s) => format!("podcast: {} - {} (eps {:?}, chart {:?})", s.title, s.author, s.episode_count, s.chart_rank),
        };
        for q in qs.split(',') {
            let t = std::time::Instant::now();
            let ov = search_overview(&st, q.into(), false).await.unwrap();
            let ms = t.elapsed().as_millis();
            println!("\n=== {q:?} ({ms} ms) top_show {:?} podcasts_first {} pending {}", ov.top_show, ov.podcasts_first, ov.pods_pending);
            for (i, r) in all_view_order(&ov).iter().take(20).enumerate() {
                println!("  all {:>2}. {}", i + 1, label(r));
            }
            for (i, r) in dropdown_order(&ov).iter().take(15).enumerate() {
                println!("  drop {:>2}. {}", i + 1, label(r));
            }
            // The owner's report: "tucker" must open on The Tucker Carlson Show.
            if q == "tucker" {
                let first = all_view_order(&ov).into_iter().take(3).any(|r| matches!(r, ViewRow::Show(s) if s.title.contains("Tucker Carlson")));
                assert!(first, "tucker: The Tucker Carlson Show is not in the first three rows");
            }
            if let Ok(p) = search_page(&st, q.into(), "podcasts".into(), None).await {
                for (i, s) in p.shows.iter().take(8).enumerate() {
                    println!("  chip {:>2}. {} - {} (eps {:?}, chart {:?})", i + 1, s.title, s.author, s.episode_count, s.chart_rank);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the UIs do with a search card of this kind: (play, open). Mirrors `playBrowse` /
    /// `exploreHref` (Mac) and `SearchView.swift` (iPhone).
    fn ui_action(kind: &str, id: &str) -> String {
        match kind {
            "song" => format!("play video {id}; open = play"),
            "album" => format!("play album {id}; open /album/{id}"),
            "artist" => format!("play top songs; open /artist/{id}"),
            "playlist" => format!("play getPlaylist({id}); open /playlist/{id}"),
            "podcast" => format!("play newest episode of {id}; open /show/{id}"),
            "episode" => format!("play episode {id}; open /episode/{id}"),
            _ => "nothing".into(),
        }
    }

    /// The owner's report (1.0.4-rc.3): an episode from "jensen huang jre" opened as an empty
    /// playlist and wouldn't play. Prints every result's kind, id and what the UIs do with it:
    /// `cargo test -p encore-core --lib search::live::episode_results_route_as_episodes -- --ignored --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "hits live YouTube Music and iTunes"]
    async fn episode_results_route_as_episodes() {
        let dir = std::env::temp_dir().join(format!("spotless-search-ep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let host: Arc<dyn crate::host::Host> = Arc::new(crate::host::NullHost(dir.clone()));
        let st = crate::boot::start_core(host).map_err(|e| e.to_string()).unwrap().state.clone();
        let ov = search_overview(&st, "jensen huang jre".into(), false).await.unwrap();
        let r = &ov.results;
        let mut cards = Vec::new();
        for (shelf, list) in [("top", &r.top), ("songs", &r.songs), ("albums", &r.albums), ("artists", &r.artists), ("playlists", &r.playlists)] {
            for c in list {
                println!("{shelf:>9}: {:<8} {:<40} {:?} -> {}", c.kind, c.id, c.title, ui_action(c.kind, &c.id));
                cards.push(c);
            }
        }
        for s in &ov.shows {
            println!("{:>9}: show     {:<40} {:?} -> open /show/{}", "shows", s.id, s.title, s.id);
        }
        for e in &ov.episodes {
            println!("{:>9}: episode  {:<40} {:?} (video {:?})", "episodes", e.id, e.title, e.video_id);
        }
        for c in &cards {
            if c.id.starts_with("MPSP") {
                assert_eq!(c.kind, "podcast", "{} is a show", c.id);
            }
            assert!(c.kind != "playlist" || !c.id.starts_with("MP"), "{} ({}) routed as a playlist", c.id, c.title);
        }
        // The owner's episode is the Top result, as an episode.
        let top = r.top.first().expect("a Top result");
        assert_eq!((top.kind, top.id.as_str()), ("episode", "3hptKYix4X8"), "{}", top.title);
        // The episode page and its play button both load the episode by the card's id.
        for c in cards.iter().filter(|c| c.kind == "episode") {
            let e = podcasts::api::podcast_episode(&st, c.id.clone()).await.unwrap();
            println!("episode page for {}: {:?} of {:?}, video {:?}", c.id, e.title, e.show_title, e.video_id);
            assert_eq!(e.id, c.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "hits live YouTube Music and iTunes"]
    async fn overview_pages_and_fallbacks() {
        let dir = std::env::temp_dir().join(format!("spotless-search2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let host: Arc<dyn crate::host::Host> = Arc::new(crate::host::NullHost(dir.clone()));
        let booted = crate::boot::start_core(host).map_err(|e| e.to_string()).unwrap();
        let st = booted.state.clone();

        let t = std::time::Instant::now();
        let ov = search_overview(&st, "chris williamson".into(), false).await.unwrap();
        let top = ov.top_show.map(|i| ov.shows[i].title.clone());
        println!("chris williamson ({} ms): top show {top:?}, podcasts first {}, shows {:?}", t.elapsed().as_millis(),
            ov.podcasts_first, ov.shows.iter().take(3).map(|s| &s.title).collect::<Vec<_>>());
        assert_eq!(top.as_deref(), Some("Modern Wisdom"));

        let t = std::time::Instant::now();
        let ov = search_overview(&st, "daft punk".into(), false).await.unwrap();
        println!("daft punk overview {} ms, {} songs", t.elapsed().as_millis(), ov.songs.len());
        let mut ids: std::collections::HashSet<String> = ov.songs.iter().map(|s| s.video_id.clone()).collect();
        let mut next = ov.songs_continuation.clone();
        let mut pages = 1;
        while let Some(tok) = next.take() {
            let p = search_page(&st, ov.query.clone(), "songs".into(), Some(tok)).await.unwrap();
            ids.extend(p.songs.iter().map(|s| s.video_id.clone()));
            pages += 1;
            next = p.continuation;
            if ids.len() >= 150 || pages >= 10 {
                break;
            }
        }
        println!("daft punk Songs chip: {} unique rows over {pages} pages", ids.len());
        assert!(ids.len() >= 100);

        for q in ["!!!???", "bohemain rapsodyy", "xqzvwpl kjh"] {
            let ov = search_overview(&st, q.into(), false).await.unwrap();
            let n = all_view_order(&ov).len();
            println!("{q:?}: {n} rows, correction {:?}, try these {:?}", ov.correction,
                ov.try_these.as_ref().map(|t| (t.songs.len(), t.artists.len())));
            assert!(n > 0, "{q}: nothing");
        }
        for f in ["podcasts", "profiles", "episodes", "albums"] {
            let p = search_page(&st, "chris williamson".into(), f.into(), None).await.unwrap();
            println!("chip {f}: {} rows, more {}, first {:?}", p.len(), p.continuation.is_some(),
                p.shows.first().map(|s| s.title.clone()).or(p.items.first().map(|i| i.title.clone())).or(p.episodes.first().map(|e| e.title.clone())));
        }

        // Lyrics alongside: the overview's latency must not change (it never waits on them).
        let mut alone = Vec::new();
        let mut with = Vec::new();
        for q in ["dua lipa", "blinding lights", "die with a smile", "metallica", "birds of a feather", "the weeknd"] {
            // Two spellings of the same query, so neither answer comes from the other's cache;
            // which one runs with lyrics alternates.
            let (a, b) = (q.to_owned(), q.to_uppercase());
            let flip = alone.len() % 2 == 1;
            let (plain, lyr_q) = if flip { (b.clone(), a.clone()) } else { (a.clone(), b.clone()) };
            let t = std::time::Instant::now();
            let _ = search_overview(&st, plain, false).await;
            alone.push(t.elapsed().as_millis());
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let t = std::time::Instant::now();
            let lyr = tokio::spawn({
                let st = st.clone();
                let q = lyr_q.clone();
                async move { search_lyrics(&st, q).await.len() }
            });
            let _ = search_overview(&st, lyr_q, false).await;
            with.push(t.elapsed().as_millis());
            let n = lyr.await.unwrap();
            println!("{q:?}: overview alone {} ms, with lyrics alongside {} ms (gate {}, {n} lyric matches)",
                alone.last().unwrap(), with.last().unwrap(), crate::lyricsearch::looks_like_lyrics(q));
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        alone.sort();
        with.sort();
        println!("median overview: alone {} ms, with lyrics {} ms", alone[alone.len() / 2], with[with.len() / 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
