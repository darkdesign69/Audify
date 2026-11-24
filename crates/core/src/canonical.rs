//! Canonical tracks for every list the app generates (stream: canonical).
//!
//! Spotify's radio, autoplay and Made for you only ever play the studio recording. YouTube's
//! radio pages are full of music videos ("(Official Video)", "(Visualizer)", lyric videos), live
//! cuts and remaster re-uploads, and a radio seeded *on* a music video is nothing but videos. This
//! module turns a generated candidate list into the songs Spotify would play:
//!
//! 1. **Seed.** A radio on a music video runs on its audio track (ATV) instead
//!    ([`audio_seed`]); YouTube builds an ATV's radio out of songs.
//! 2. **Version filter.** Live, acoustic, sped-up, cover, karaoke … versions go unless the seed
//!    itself is that version ([`allowed`]). Studio covers remasters, edits and video dressing;
//!    re-recordings ("Taylor's Version") are the artist's current canonical cut and stay. Remixes
//!    stay only as released audio tracks ([`admits`]).
//! 3. **Map.** Each music video becomes its audio track: the row's own `counterpart` when
//!    YouTube paired it (signed in), else a cached Songs search scored by `identity`
//!    ([`best_audio`]). A video with no audio track is dropped. The video stays reachable as the
//!    row's `counterpart`, so the player's Song/Video switch still offers it.
//! 4. **Dedup** by id and by `identity` work key, the audio track winning.
//!
//! Lookups are cached in `smart_cache` (found: 30 days, none: 7 days) and run a few at a time.
//! Playback never waits on them: queue paths run the cache-only [`prepare`] synchronously and map
//! what is left in the background (`AppState::canonical_apply`); background generators (Made For
//! You, playlist suggestions) await [`canonicalize`].
//!
//! `identity` is used read-only (another stream tunes its internals).

use std::collections::{HashMap, HashSet};

use innertube::{InnerTube, SongItem, YouTubeClient};

use crate::db::Db;
use crate::identity::{self, Gate, Track, Version, WorkKey};

/// A found audio track is a fact about the release; keep it a month.
const FOUND_TTL: i64 = 30 * 24 * 3600;
/// "No audio track" can change when a release lands; ask again after a week.
const MISSING_TTL: i64 = 7 * 24 * 3600;
/// Songs searches in flight at once.
const PARALLEL: usize = 4;
/// Searches per batch: a radio page holds 50 rows, and anything past this stays unmapped (and
/// a generator drops it).
pub const LOOKUP_MAX: usize = 40;
/// The least `identity::score` an audio track needs to stand in for a video. Durations rarely
/// agree (a video has its intro and outro), so a perfect title + artist match without the
/// duration lands at ~0.79.
const ACCEPT: f64 = 0.70;

// --- what a row is ---------------------------------------------------------------------------

/// The recording a row names (`identity` classification).
pub fn version(s: &SongItem) -> Version {
    Track::from_song(s).parsed().version
}

/// Whether a candidate of version `cand` belongs in a list generated from a seed of version
/// `seed` (`None`: no seed song, e.g. an artist radio or a mix).
pub fn allowed(cand: Version, seed: Option<Version>) -> bool {
    match cand {
        Version::Studio | Version::ReRecorded => true,
        v => seed == Some(v),
    }
}

/// [`allowed`] for a row. One exception: a remix that YouTube Music carries as a release (an audio
/// track, or a video that maps to one, since [`best_audio`] keeps the version) is a catalogue
/// track Spotify plays too ("Cold Heart (PNAU Remix)", "Save Your Tears (Remix)"); fan remixes
/// have no audio track and go.
pub fn admits(s: &SongItem, seed: Option<Version>) -> bool {
    let v = version(s);
    allowed(v, seed) || (v == Version::Remix && !s.is_upload)
}

/// Rows the mapping never touches: podcast episodes and local files have no audio track to find.
fn exempt(s: &SongItem) -> bool {
    s.is_episode || crate::local::is_local_song(&s.video_id)
}

/// The music video `video` as its audio track `audio`: the audio's id, title, art and length,
/// the video kept as the counterpart, and the queue flags of the row it replaces.
pub fn as_audio(video: &SongItem, audio: &SongItem) -> SongItem {
    let mut out = audio.clone();
    out.is_video = false;
    out.counterpart = Some(video.video_id.clone());
    out.counterpart_thumbnail = video.thumbnail.clone();
    out.counterpart_segments = Vec::new();
    out.autoplay = video.autoplay;
    out.queued = video.queued;
    out.queued_end = video.queued_end;
    out.queued_by = video.queued_by.clone();
    out.queued_from = video.queued_from.clone();
    out.album = out.album.or_else(|| video.album.clone());
    out.album_id = out.album_id.or_else(|| video.album_id.clone());
    out.explicit |= video.explicit;
    out
}

/// A video row YouTube already paired with its song (signed-in wrapper rows): trade over.
fn swap_to_audio(item: &mut SongItem) -> bool {
    item.is_video && item.counterpart.is_some() && crate::state::swap_version(item)
}

// --- the cache -------------------------------------------------------------------------------

fn key_found(id: &str) -> String {
    format!("atv:{id}")
}
fn key_missing(id: &str) -> String {
    format!("atv-none:{id}")
}

/// What the cache knows about a video: `Some(Some(audio))` found, `Some(None)` it has none,
/// `None` never asked (or expired).
pub fn cached(db: &Db, video_id: &str) -> Option<Option<SongItem>> {
    if let Some(a) = crate::smart::cache_get::<SongItem>(db, &key_found(video_id), FOUND_TTL) {
        return Some(Some(a));
    }
    crate::smart::cache_get::<bool>(db, &key_missing(video_id), MISSING_TTL).map(|_| None)
}

fn remember(db: &Db, video_id: &str, audio: Option<&SongItem>) {
    match audio {
        Some(a) => {
            let mut a = a.clone();
            // Only the song itself: no queue flags or video twin ride along in the cache.
            a.counterpart = None;
            a.counterpart_thumbnail = None;
            a.counterpart_segments = Vec::new();
            a.autoplay = false;
            crate::smart::cache_put(db, &key_found(video_id), &a)
        }
        None => crate::smart::cache_put(db, &key_missing(video_id), &true),
    }
}

// --- the synchronous pass ----------------------------------------------------------------------

/// The cache-only pass, for paths that must not wait (radio, autoplay). In place: drops versions
/// the seed didn't ask for, swaps paired videos, maps the cached ones, drops videos known to have
/// no audio track, and dedups. Returns the videos still to look up (left in place, in order).
///
/// `seed` is the song the list was generated from; its own row (by id) is never touched and the
/// rest are deduped against it, so a radio on a playing video gets no copy of that same song.
pub fn prepare(
    items: &mut Vec<SongItem>,
    seed: Option<&SongItem>,
    lookup: impl Fn(&str) -> Option<Option<SongItem>>,
) -> Vec<SongItem> {
    prepare_with(items, seed, true, lookup)
}

/// [`prepare`] without the version filter: for lists of what the user played themselves (Repeat
/// Rewind, a Your Mix's favourites, New Release Mix's albums), where a live cut they love is
/// theirs to keep but a music video still becomes its song.
pub fn prepare_songs(
    items: &mut Vec<SongItem>,
    lookup: impl Fn(&str) -> Option<Option<SongItem>>,
) -> Vec<SongItem> {
    prepare_with(items, None, false, lookup)
}

fn prepare_with(
    items: &mut Vec<SongItem>,
    seed: Option<&SongItem>,
    versions: bool,
    lookup: impl Fn(&str) -> Option<Option<SongItem>>,
) -> Vec<SongItem> {
    let seed_id = seed.map(|s| s.video_id.as_str());
    let seed_v = seed.map(version);
    let mut out: Vec<SongItem> = Vec::with_capacity(items.len());
    for mut s in items.drain(..) {
        if Some(s.video_id.as_str()) == seed_id || exempt(&s) {
            out.push(s);
            continue;
        }
        if versions && !admits(&s, seed_v) {
            continue;
        }
        if s.is_video && !swap_to_audio(&mut s) {
            match lookup(&s.video_id) {
                Some(Some(a)) => s = as_audio(&s, &a),
                Some(None) => continue,
                None => {}
            }
        }
        out.push(s);
    }
    *items = dedup(out, seed);
    items
        .iter()
        .filter(|s| s.is_video && !exempt(s) && Some(s.video_id.as_str()) != seed_id)
        .cloned()
        .collect()
}

/// [`prepare`] against the database cache.
pub fn prepare_db(db: &Db, items: &mut Vec<SongItem>, seed: Option<&SongItem>) -> Vec<SongItem> {
    prepare(items, seed, |id| cached(db, id))
}

/// One row per id and per work (`identity` work key), the first slot kept and an audio track
/// taking it over from a video of the same work. The seed's id and work count as taken (its own
/// row, when present, stays).
pub fn dedup(items: Vec<SongItem>, seed: Option<&SongItem>) -> Vec<SongItem> {
    let seed_id = seed.map(|s| s.video_id.clone());
    let mut ids: HashSet<String> = HashSet::new();
    let mut works: HashMap<WorkKey, usize> = HashMap::new();
    let seed_work = seed.map(identity::work_key_song);
    let mut out: Vec<SongItem> = Vec::with_capacity(items.len());
    let mut seed_seen = false;
    for s in items {
        let is_seed = Some(&s.video_id) == seed_id.as_ref();
        if is_seed {
            if seed_seen {
                continue;
            }
            seed_seen = true;
            ids.insert(s.video_id.clone());
            out.push(s);
            continue;
        }
        if exempt(&s) {
            if ids.insert(s.video_id.clone()) {
                out.push(s);
            }
            continue;
        }
        if ids.contains(&s.video_id)
            || seed_id.as_ref().is_some_and(|id| s.counterpart.as_ref() == Some(id))
        {
            continue;
        }
        let w = identity::work_key_song(&s);
        if seed_work.as_ref() == Some(&w) {
            continue;
        }
        match works.get(&w) {
            Some(&at) if out[at].is_video && !s.is_video => {
                ids.insert(s.video_id.clone());
                out[at] = s;
            }
            Some(_) => {}
            None => {
                ids.insert(s.video_id.clone());
                works.insert(w, out.len());
                out.push(s);
            }
        }
    }
    out
}

/// Drop rows of `items` the list `have` already holds, by id or as the same song (work key),
/// another version included: autoplay never brings back a song the queue has.
pub fn drop_known(items: &mut Vec<SongItem>, have: &[SongItem]) {
    let ids: HashSet<&str> = have
        .iter()
        .flat_map(|s| std::iter::once(s.video_id.as_str()).chain(s.counterpart.as_deref()))
        .collect();
    let works: HashSet<WorkKey> =
        have.iter().filter(|s| !exempt(s)).map(identity::work_key_song).collect();
    items.retain(|s| {
        !ids.contains(s.video_id.as_str())
            && (exempt(s) || !works.contains(&identity::work_key_song(s)))
    });
}

// --- finding the audio track -----------------------------------------------------------------

/// Text inside brackets removed: "Song (Official Video) [4K]" -> "Song".
fn strip_brackets(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The Songs search that finds a video's audio track: "<artist> <title>", the title undressed
/// ("(Official Video)" gone) but keeping a version word the row really is ("(Live)").
pub fn query_for(video: &SongItem) -> String {
    let t = Track::from_song(video);
    let p = t.parsed();
    let artist = t.artists.first().cloned().unwrap_or_else(|| video.artists.trim().to_owned());
    let title = if p.version == Version::Studio {
        p.base_raw.clone()
    } else {
        // "Hotel California (Live 1976)": the words that make it that version stay.
        let base = strip_brackets(&p.base_raw);
        let dressing = video.title.replacen(&p.base_raw, "", 1);
        format!("{base} {}", dressing.replace(['(', ')', '[', ']'], " "))
            .split_whitespace()
            .filter(|w| {
                !matches!(
                    identity::norm(w).as_str(),
                    "official"
                        | "video"
                        | "music"
                        | "visualizer"
                        | "lyric"
                        | "lyrics"
                        | "4k"
                        | "hd"
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let title = title.trim().to_owned();
    if artist.is_empty() || identity::norm(&title).contains(&identity::norm(&artist)) {
        title
    } else {
        format!("{artist} {title}")
    }
}

/// The audio track among `cands` (a Songs search, YouTube's order) that is `video`'s song: same
/// title and artist and the same version; the length may differ (a video's intro and outro).
pub fn best_audio(video: &SongItem, cands: &[SongItem]) -> Option<SongItem> {
    let src = Track::from_song(video);
    let mut best: Option<(identity::Score, &SongItem)> = None;
    for (i, c) in cands.iter().enumerate() {
        if c.is_video || c.is_episode || c.video_id == video.video_id {
            continue;
        }
        let s = identity::score(&src, &Track::from_song(c), i);
        let hard = s.gates.iter().any(|g| matches!(g, Gate::Version | Gate::Artist | Gate::Title));
        if hard || s.value < ACCEPT {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| s.rank > b.rank) {
            best = Some((s, c));
        }
    }
    best.map(|(_, c)| c.clone())
}

/// Network access for lookups. A plain struct (not `AppState`) so the live test can build one.
pub struct Resolver<'a> {
    pub it: &'a InnerTube,
    pub client: &'a YouTubeClient,
    pub db: &'a Db,
}

impl<'a> Resolver<'a> {
    pub fn of(state: &'a crate::state::AppState) -> Option<Resolver<'a>> {
        Some(Resolver {
            it: &state.it,
            client: state.clients.get(innertube::METADATA_CLIENT)?,
            db: &state.db,
        })
    }

    /// `video`'s audio track: `Ok(Some)` found, `Ok(None)` it has none, `Err` couldn't ask.
    /// Cached either way it is answered.
    pub async fn lookup(&self, video: &SongItem) -> Result<Option<SongItem>, String> {
        if let Some(c) = cached(self.db, &video.video_id) {
            return Ok(c);
        }
        if let Some(twin) = video.counterpart.as_ref().filter(|_| video.is_video) {
            let mut a = video.clone();
            a.video_id = twin.clone();
            a.is_video = false;
            a.thumbnail = video.counterpart_thumbnail.clone().or(a.thumbnail.take());
            a.counterpart = None;
            return Ok(Some(a));
        }
        let q = query_for(video);
        // ── stream: reliability ── the transport retries the anonymous 403 bursts itself.
        let r = self.it.search_songs(self.client, &q, false).await.map_err(|e| e.to_string())?;
        let found = best_audio(video, &r.items);
        remember(self.db, &video.video_id, found.as_ref());
        Ok(found)
    }

    /// [`Self::lookup`] for many videos, a few at a time, at most [`LOOKUP_MAX`]. Only the
    /// answered ones are in the map (a failed search is simply absent).
    pub async fn lookup_many(&self, videos: &[SongItem]) -> HashMap<String, Option<SongItem>> {
        let mut seen = HashSet::new();
        let todo: Vec<&SongItem> =
            videos.iter().filter(|v| seen.insert(v.video_id.clone())).take(LOOKUP_MAX).collect();
        // Chunks rather than a buffered stream: a stream of borrowing futures trips the
        // `Send` bound of the background task (higher-ranked lifetime inference).
        let mut out = HashMap::new();
        for chunk in todo.chunks(PARALLEL) {
            // ── stream: reliability ── a batch of up to 40 searches enriches a list; nobody
            // clicked on any of them, so they are paced and give way to user actions.
            let answers = innertube::background(futures_util::future::join_all(
                chunk.iter().map(|v| self.lookup(v)),
            ))
            .await;
            for (v, r) in chunk.iter().zip(answers) {
                match r {
                    Ok(a) => {
                        out.insert(v.video_id.clone(), a);
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, video = %v.video_id, "canonical: lookup failed")
                    }
                }
            }
        }
        out
    }

    /// The id a radio on `seed` should run on: its audio track when it is a music video, else
    /// `None` (keep the seed). A radio on a video is a radio of videos.
    pub async fn audio_seed(&self, seed: &SongItem) -> Option<String> {
        if !seed.is_video || exempt(seed) {
            return None;
        }
        match self.lookup(seed).await {
            Ok(Some(a)) => Some(a.video_id),
            _ => None,
        }
    }

    /// The whole pass, waiting for the lookups: for lists built in the background (Made For
    /// You, suggestions, mixes). A video still unmapped after it (no answer) is dropped: a
    /// generated list holds songs only.
    pub async fn canonicalize(
        &self,
        items: Vec<SongItem>,
        seed: Option<&SongItem>,
    ) -> Vec<SongItem> {
        self.run(items, seed, true).await
    }

    /// [`Self::canonicalize`] without the version filter ([`prepare_songs`]).
    pub async fn to_songs(&self, items: Vec<SongItem>) -> Vec<SongItem> {
        self.run(items, None, false).await
    }

    async fn run(
        &self,
        mut items: Vec<SongItem>,
        seed: Option<&SongItem>,
        versions: bool,
    ) -> Vec<SongItem> {
        let pending = prepare_with(&mut items, seed, versions, |id| cached(self.db, id));
        if !pending.is_empty() {
            let found = self.lookup_many(&pending).await;
            apply(&mut items, 0, seed.map(|s| s.video_id.as_str()), &found);
        }
        let seed_id = seed.map(|s| s.video_id.as_str());
        items.retain(|s| !s.is_video || exempt(s) || Some(s.video_id.as_str()) == seed_id);
        items
    }
}

/// See [`Resolver::audio_seed`].
pub async fn audio_seed(state: &crate::state::AppState, seed: &SongItem) -> Option<String> {
    Resolver::of(state)?.audio_seed(seed).await
}

// ── stream: autoplay ──
/// Versions an autoplay seed is taken as: the studio song and its own catalogue cuts.
fn studio_like(v: Version) -> bool {
    matches!(v, Version::Studio | Version::ReRecorded | Version::Remix)
}

/// The studio recording a song stands for, as a bare query row: the title without its version
/// words ("Bohemian Rhapsody (Live Aid 1985)" → "Bohemian Rhapsody"), no length, marked a video
/// so [`best_audio`] looks for an audio track of it.
pub fn studio_probe(seed: &SongItem) -> SongItem {
    let p = Track::from_song(seed).parsed();
    let mut probe = seed.clone();
    probe.title = strip_brackets(&p.base_raw);
    probe.duration = None;
    probe.is_video = true;
    probe.counterpart = None;
    probe
}

fn key_studio(id: &str) -> String {
    format!("studio:{id}")
}

impl Resolver<'_> {
    /// What autoplay should seed its radio on for `seed`: the studio audio track. A music video
    /// becomes its song; a live, acoustic, sped-up … cut becomes the studio recording, so a live
    /// clip doesn't fill autoplay with live clips. `None`: the seed already is one (or nothing
    /// better was found), seed on it as it is. Cached either way it is answered.
    pub async fn studio_seed(&self, seed: &SongItem) -> Option<SongItem> {
        if exempt(seed) {
            return None;
        }
        let audio = if seed.is_video { self.lookup(seed).await.ok().flatten() } else { None };
        let base = audio.clone().unwrap_or_else(|| seed.clone());
        if studio_like(version(&base)) {
            return audio;
        }
        let key = key_studio(&base.video_id);
        if let Some(hit) = crate::smart::cache_get::<Option<SongItem>>(self.db, &key, FOUND_TTL) {
            return hit.or(audio);
        }
        let probe = studio_probe(&base);
        let q = query_for(&probe);
        let found = match self.it.search_songs(self.client, &q, false).await {
            Ok(r) => best_audio(&probe, &r.items),
            Err(_) => return audio,
        };
        crate::smart::cache_put(self.db, &key, &found);
        found.or(audio)
    }
}

/// See [`Resolver::studio_seed`]. Without a metadata client the seed stays as it is.
pub async fn studio_seed(state: &crate::state::AppState, seed: &SongItem) -> Option<SongItem> {
    Resolver::of(state)?.studio_seed(seed).await
}
// ── end stream: autoplay ──

/// See [`Resolver::to_songs`].
pub async fn to_songs(state: &crate::state::AppState, mut items: Vec<SongItem>) -> Vec<SongItem> {
    match Resolver::of(state) {
        Some(r) => r.to_songs(items).await,
        None => {
            prepare_songs(&mut items, |id| cached(&state.db, id));
            items.retain(|s| !s.is_video || exempt(s));
            items
        }
    }
}

/// See [`Resolver::canonicalize`]. Without a metadata client the cache-only pass still runs.
pub async fn canonicalize(
    state: &crate::state::AppState,
    mut items: Vec<SongItem>,
    seed: Option<&SongItem>,
) -> Vec<SongItem> {
    match Resolver::of(state) {
        Some(r) => r.canonicalize(items, seed).await,
        None => {
            prepare_db(&state.db, &mut items, seed);
            items.retain(|s| !s.is_video || exempt(s));
            items
        }
    }
}

/// Put lookup answers into a list, from row `from` on, never touching `keep` (the playing row)
/// or a row the user queued by hand. A mapped video becomes its audio track, or goes when that
/// song is already in the list (anywhere) or when it has no audio track. Returns the first index
/// that changed (rows at or after it may have moved).
pub fn apply(
    items: &mut Vec<SongItem>,
    from: usize,
    keep: Option<&str>,
    found: &HashMap<String, Option<SongItem>>,
) -> Option<usize> {
    if found.is_empty() {
        return None;
    }
    let mut ids: HashSet<String> = items.iter().map(|s| s.video_id.clone()).collect();
    let mut works: HashSet<WorkKey> =
        items.iter().filter(|s| !s.is_video).map(identity::work_key_song).collect();
    let mut first = None;
    let mut i = from;
    while i < items.len() {
        let s = &items[i];
        let touch = s.is_video
            && !s.queued
            && !s.queued_end
            && Some(s.video_id.as_str()) != keep
            && !exempt(s);
        let answer = if touch { found.get(&s.video_id) } else { None };
        match answer {
            Some(Some(a)) => {
                let row = as_audio(s, a);
                let w = identity::work_key_song(&row);
                first.get_or_insert(i);
                if ids.contains(&row.video_id) || works.contains(&w) {
                    items.remove(i);
                    continue;
                }
                ids.insert(row.video_id.clone());
                works.insert(w);
                items[i] = row;
            }
            Some(None) => {
                first.get_or_insert(i);
                items.remove(i);
                continue;
            }
            None => {}
        }
        i += 1;
    }
    first
}

// ── stream: reliability ── post-play lookups, deferred and coalesced.
//
// Every play hydrates a radio, and every radio queued ~40 searches at once. Fired the moment the
// track started they competed with what the user does next (a hover hint, a page) and, with the
// bot wall tarpitting, clogged the background lane for seconds. Nobody needs them until the
// upcoming rows are about to play, so they now wait a few seconds, and everything queued in the
// meantime (rapid skips, a radio and its re-seed) goes out as one batch of what is still upcoming.

/// How long post-play lookups wait before going out.
pub const POST_PLAY_DEFER: std::time::Duration = std::time::Duration::from_secs(4);

#[derive(Default)]
pub struct Deferred {
    items: Vec<SongItem>,
    running: bool,
}

impl Deferred {
    /// Queue `items` (deduped by video id). True when no worker is draining: start one.
    pub fn push(&mut self, items: Vec<SongItem>) -> bool {
        for it in items {
            if !self.items.iter().any(|i| i.video_id == it.video_id) {
                self.items.push(it);
            }
        }
        !std::mem::replace(&mut self.running, true)
    }

    /// Everything queued so far. Empty means the worker should stop (and it is marked stopped
    /// under the same lock, so a push right after starts a new one).
    pub fn take(&mut self) -> Vec<SongItem> {
        let v = std::mem::take(&mut self.items);
        if v.is_empty() {
            self.running = false;
        }
        v
    }
}

pub static DEFERRED: std::sync::Mutex<Deferred> =
    std::sync::Mutex::new(Deferred { items: Vec::new(), running: false });
// ── end stream: reliability ──

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod tests;
