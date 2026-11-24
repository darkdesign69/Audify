//! Spotify import (#375): match a Spotify library to YouTube Music and build the playlists.
//!
//! [`crate::spotify`] reads the source; this decides what each track is on YouTube Music and does
//! the writing. One import at a time, in the background, in three phases:
//!
//! 1. **Matching.** One `FILTER_SONG` search per distinct track (a song in five playlists is one
//!    search), scored by [`score`], plus a video search when no song is even a plausible guess.
//!    The searches are unrecorded, so a 2,000-track import doesn't bury the user's search
//!    history. Every answer lands in `import_matches`, so a second import, a retry and "Update
//!    from Spotify" only search for what is new.
//! 2. **Review.** The job waits while the user changes any pick. Nothing has been written yet.
//! 3. **Creating.** One playlist per list, on the account or on this machine.
//!
//! Every request to YouTube goes through [`before_youtube`] first, which is what keeps an import
//! from getting the user's IP or account flagged. See "staying welcome on YouTube" below.
//!
//! The UI follows along through `import-progress` events, each carrying a [`Snapshot`].
//!
//! Thanks to @Tomjerri1, who asked for this in #375 and wrote the first version of it in #378.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use innertube::{BrowseItem, SongItem};
use serde::{Deserialize, Serialize};
use crate::host::HostExt;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

use crate::db::{now_secs, ImportMatch};
use crate::spotify::{self, LinkKind, ListKind, SavedAlbum, SourceList, SourceTrack};
use crate::state::{is_local_playlist, AppState, LOCAL_PLAYLIST_PREFIX};

// --- matching ------------------------------------------------------------------------------------

// stream: smart: tiers and scoring moved to `crate::identity` (research 22). `CHECK` is kept for
// the "is any song even a plausible guess" test before the video search.
const CHECK: f64 = crate::identity::CHECK;
/// YouTube Music's playlist cap. A longer list is split into "Name", "Name (2)", ...
const YTM_PLAYLIST_MAX: usize = 5000;

/// Words in a title's dressing that make it a different recording, so both sides have to agree on
/// them. "Remastered", "radio edit", "mono" and the like are the same song and stay out.
const VERSIONS: &[&str] = &[
    "live",
    "acoustic",
    "remix",
    "instrumental",
    "karaoke",
    "cover",
    "demo",
    "sped",
    "slowed",
    "reverb",
    "nightcore",
    "8d",
    "acapella",
    "cappella",
    "unplugged",
    "orchestral",
    "piano",
    "lofi",
    "extended",
    "reprise",
    "taylors",
];

/// Lowercase, accents off, apostrophes gone ("don't" is "dont"), `&` as "and", anything else that
/// isn't a letter or digit as a space. Letters of every script survive.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd() {
        match c {
            _ if is_combining_mark(c) => {}
            '\'' | '\u{2019}' | '`' => {}
            '&' => out.push_str(" and "),
            _ if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title without its dressing, and the dressing: bracketed parts, a Spotify-style
/// " - 2011 Remaster" tail and a bare "feat. X" tail. Both as written, not normalized.
fn strip_dressing(title: &str) -> (String, String) {
    let mut base = String::new();
    let mut dressing = String::new();
    let mut depth = 0u32;
    for c in title.chars() {
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                dressing.push(' ');
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                dressing.push(' ');
            }
            _ if depth > 0 => dressing.push(c),
            _ => base.push(c),
        }
    }
    for sep in [" - ", " \u{2013} ", " \u{2014} "] {
        if let Some(i) = base.find(sep) {
            dressing.push(' ');
            dressing.push_str(&base[i + sep.len()..]);
            base.truncate(i);
        }
    }
    let feat = base.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).find(|&i| {
        let word = base[i + 1..].split(' ').next().unwrap_or("").to_lowercase();
        matches!(word.trim_end_matches('.'), "feat" | "ft" | "featuring")
    });
    if let Some(i) = feat {
        dressing.push_str(&base[i..]);
        base.truncate(i);
    }
    let base = base.trim();
    // All dressing ("(Intro)"): the dressing is the title.
    if base.is_empty() {
        return (title.trim().to_owned(), String::new());
    }
    (base.to_owned(), dressing)
}

fn versions(dressing: &str) -> Vec<&'static str> {
    let words = norm(dressing);
    let words: HashSet<&str> = words.split(' ').collect();
    VERSIONS.iter().copied().filter(|v| words.contains(v)).collect()
}

fn title_score(a: &str, b: &str) -> f64 {
    strsim::sorensen_dice(&norm(a), &norm(b))
}

// ── stream: smart ── scoring is `identity::score` (the karaoke fix: the artist is matched
// against the artist line only, never the title, and non-original markers gate the candidate).

/// A source track in `identity`'s comparable view.
fn src_track(src: &SourceTrack) -> crate::identity::Track {
    crate::identity::Track {
        title: src.title.clone(),
        artist_line: src.artists.join(", "),
        artists: src.artists.clone(),
        artist_ids: Vec::new(),
        album: src.album.clone(),
        duration: src.duration_ms.map(|ms| ms as f64 / 1000.0),
        is_video: false,
        explicit: src.explicit,
    }
}

/// How sure we are that `cand` (at `i` in YouTube's answer) is `src`.
pub fn score(src: &SourceTrack, cand: &SongItem, i: usize) -> crate::identity::Score {
    crate::identity::score(&src_track(src), &crate::identity::Track::from_song(cand), i)
}

type Ranked = (crate::identity::Score, SongItem);
// ── end stream: smart ──

/// What to type into the search box: the bare title, any version word (searching "Song" for
/// "Song (Live)" finds the studio cut), and the first artist.
pub fn query(t: &SourceTrack) -> String {
    let (base, dressing) = strip_dressing(&t.title);
    let mut q = base;
    for v in versions(&dressing) {
        q.push(' ');
        q.push_str(v);
    }
    if let Some(a) = t.artists.first() {
        q.push(' ');
        q.push_str(a);
    }
    q
}

fn rank(src: &SourceTrack, items: Vec<SongItem>, out: &mut Vec<Ranked>) {
    for (i, c) in items.into_iter().enumerate() {
        if c.video_id.is_empty() || out.iter().any(|(_, o)| o.video_id == c.video_id) {
            continue;
        }
        out.push((score(src, &c, i), c));
    }
}

fn metadata_client(state: &AppState) -> Result<&innertube::YouTubeClient, String> {
    state.clients.get(innertube::METADATA_CLIENT).ok_or_else(|| "metadata client missing".into())
}

// --- staying welcome on YouTube ------------------------------------------------------------------
//
// Every request an import sends leaves from the user's own IP and, for the writes, from their own
// Google account. YouTube flags both for volume, and a flagged IP is "Sign in to confirm you're
// not a bot" on every song the app plays afterwards, not only on the import. Nobody controls when
// that lifts. So an import goes slower than it could, on purpose, and stops at the first sign that
// YouTube minds:
//
// - one request at a time: a search every 1.2 to 2 s, a write every 3 to 5 s, never in lockstep;
// - a 15 to 25 s break after every 40 requests;
// - at most 600 searches in any hour, counted across restarts; past that the import waits and
//   says until when;
// - a 429 or 403 from YouTube, or a bot check anywhere in the app (`note_playability`), stops the
//   import and keeps every import off YouTube for an hour, also across restarts;
// - no bulk likes, follows or album saves. Mass engagement from one account is what YouTube's spam
//   filters look for, and the playlists are the migration. Liked Songs, saved albums and followed
//   artists land in the local library instead (library.rs), which YouTube never sees.
//
// ponytail: fixed numbers, chosen conservative with no published limit to aim at. If users report
// pushback below them, lower them here; there is no setting on purpose.

const SEARCH_GAP_MS: (u64, u64) = (1_200, 2_000);
const WRITE_GAP_MS: (u64, u64) = (3_000, 5_000);
const BREAK_EVERY: u32 = 40;
const BREAK_MS: (u64, u64) = (15_000, 25_000);
const SEARCHES_PER_HOUR: i64 = 600;
const HOUR: i64 = 3_600;
const COOLDOWN_SECS: i64 = HOUR;
/// `settings` rows: "<window start> <searches in it>", and the unix second the cooldown ends.
const BUDGET_KEY: &str = "import_search_budget";
const COOLDOWN_KEY: &str = "import_cooldown_until";

fn between((lo, hi): (u64, u64)) -> Duration {
    Duration::from_millis(lo + rand::random::<u64>() % (hi - lo))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    Search,
    Write,
}

struct Pacer {
    last: Option<Instant>,
    since_break: u32,
}

static PACER: Mutex<Pacer> = Mutex::new(Pacer { last: None, since_break: 0 });

/// Take the next request slot: `gap` after the last one, or `now` if that has passed. Taken under
/// the lock, so two callers at once (an import and a pasted link) queue one behind the other
/// instead of firing together; time already spent since the last request counts toward the gap.
fn reserve(p: &mut Pacer, now: Instant, gap: Duration) -> Instant {
    let at = p.last.map_or(now, |t| (t + gap).max(now));
    p.last = Some(at);
    at
}

/// Unix second of the last bot check the player saw, 0 for none.
static BOT_CHECK_AT: AtomicI64 = AtomicI64::new(0);

/// The orchestrator's hook: every playback YouTube refused passes its reason through here. A bot
/// check means the IP is already on thin ice, whatever the import is doing.
///
/// Matched on words because the status is a plain `LOGIN_REQUIRED`, which a region lock answers
/// too. The reason comes in the app's language; these words cover English, German, Spanish,
/// Italian, Dutch, French, Portuguese and Russian. The import's own 429/403 check is the guard
/// that doesn't depend on wording.
pub fn note_playability(reason: Option<&str>) {
    if reason.is_some_and(is_bot_check) {
        BOT_CHECK_AT.store(now_secs(), Ordering::Relaxed);
    }
}

pub(crate) fn is_bot_check(reason: &str) -> bool {
    reason
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| matches!(w, "bot" | "bots" | "robot" | "robô" | "робот"))
}

/// Why a step stopped short.
enum Halt {
    /// Stopped by the user, or replaced by a newer import: nothing to say.
    Cancelled,
    /// YouTube pushed back, now or within the cooldown. The unix second it ends.
    Cooldown(i64),
    Failed(String),
}

impl Halt {
    /// What the UI is told: a code it words (`cooldown:<until>`), or the message as it is.
    fn code(self) -> String {
        match self {
            Halt::Cancelled => "gone".into(),
            Halt::Cooldown(until) => format!("cooldown:{until}"),
            Halt::Failed(m) => m,
        }
    }
}

/// YouTube telling us to slow down: a 429, or the 403 it answers an anonymous search with.
fn pushed_back(e: &innertube::Error) -> bool {
    matches!(e, innertube::Error::Http(h) if matches!(h.status().map(|s| s.as_u16()), Some(429 | 403)))
}

/// What an error from YouTube means for the import. Pushback starts the cooldown.
fn halt_for(state: &AppState, e: innertube::Error) -> Halt {
    if pushed_back(&e) {
        tracing::warn!(error = %e, "import: YouTube pushed back, cooling down");
        Halt::Cooldown(start_cooldown(state, now_secs()))
    } else {
        Halt::Failed(e.to_string())
    }
}

fn cooldown_setting(state: &AppState) -> i64 {
    state.db.get_setting(COOLDOWN_KEY).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Start the cooldown, or stretch one already running, from `from`. Answers when it ends.
fn start_cooldown(state: &AppState, from: i64) -> i64 {
    let until = from + COOLDOWN_SECS;
    let current = cooldown_setting(state);
    if until > current {
        state.db.set_setting(COOLDOWN_KEY, &until.to_string());
    }
    until.max(current)
}

/// When the cooldown ends, if one is running.
fn cooldown(state: &AppState) -> Option<i64> {
    let seen = BOT_CHECK_AT.load(Ordering::Relaxed);
    let until = if seen > 0 { start_cooldown(state, seen) } else { cooldown_setting(state) };
    (until > now_secs()).then_some(until)
}

/// When the hourly search budget frees up again, if it is spent. `(window start, searches)`.
fn over_budget((start, count): (i64, i64), now: i64) -> Option<i64> {
    (now - start < HOUR && count >= SEARCHES_PER_HOUR).then_some(start + HOUR)
}

/// The budget after one more search.
fn spend((start, count): (i64, i64), now: i64) -> (i64, i64) {
    if now - start >= HOUR {
        (now, 1)
    } else {
        (start, count + 1)
    }
}

fn budget(state: &AppState) -> (i64, i64) {
    state
        .db
        .get_setting(BUDGET_KEY)
        .and_then(|v| {
            let (a, b) = v.split_once(' ')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, 0))
}

/// Wait until the next request to YouTube is welcome, or say why there won't be one. Every
/// import request goes through here: the matching searches, the playlist writes, an update, a
/// pasted Spotify link. `gen` ties the wait to a job, so a stopped import stops waiting.
async fn before_youtube(state: &AppState, gen: Option<u64>, ask: Ask) -> Result<(), Halt> {
    let check = || -> Result<(), Halt> {
        if let Some(until) = cooldown(state) {
            return Err(Halt::Cooldown(until));
        }
        if gen.is_some_and(|g| !alive(g)) {
            return Err(Halt::Cancelled);
        }
        Ok(())
    };
    check()?;
    if ask == Ask::Search {
        while let Some(at) = over_budget(budget(state), now_secs()) {
            if let Some(g) = gen {
                set_waiting(state, g, Some(at));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            check()?;
        }
        if let Some(g) = gen {
            set_waiting(state, g, None);
        }
    }
    let at = {
        let mut p = PACER.lock().unwrap();
        let mut gap = between(if ask == Ask::Search { SEARCH_GAP_MS } else { WRITE_GAP_MS });
        if p.since_break >= BREAK_EVERY {
            p.since_break = 0;
            gap += between(BREAK_MS);
        }
        p.since_break += 1;
        reserve(&mut p, Instant::now(), gap)
    };
    tokio::time::sleep_until(at.into()).await;
    check()?;
    if ask == Ask::Search {
        let (start, count) = spend(budget(state), now_secs());
        state.db.set_setting(BUDGET_KEY, &format!("{start} {count}"));
    }
    Ok(())
}

/// Search for one track: songs first, then videos when no song is even a plausible guess
/// (covers, live sets and releases that never got an official upload only exist as videos; the
/// video search answers nothing when the user hides music videos). Best first, at most six.
async fn search(
    state: &AppState,
    gen: Option<u64>,
    src: &SourceTrack,
) -> Result<Vec<Ranked>, Halt> {
    let client = metadata_client(state).map_err(Halt::Failed)?;
    search_with(&state.it, client, src, || before_youtube(state, gen, Ask::Search), |e| {
        halt_for(state, e)
    })
    .await
}

/// The search itself, with the pacing and the error mapping handed in, so the live end-to-end
/// check (stream import2) runs the very same steps without an app around it.
async fn search_with<P, F>(
    it: &innertube::InnerTube,
    client: &innertube::YouTubeClient,
    src: &SourceTrack,
    mut pace: P,
    on_err: impl Fn(innertube::Error) -> Halt,
) -> Result<Vec<Ranked>, Halt>
where
    P: FnMut() -> F,
    F: std::future::Future<Output = Result<(), Halt>>,
{
    let q = query(src);
    let mut out = Vec::new();
    pace().await?;
    let songs = it.search_songs(client, &q, false).await.map_err(&on_err)?;
    rank(src, songs.items, &mut out);
    if out.iter().all(|(s, _)| s.value < CHECK) {
        pace().await?;
        match it.search_videos(client, &q).await {
            Ok(r) => rank(src, r.items, &mut out),
            Err(e) if pushed_back(&e) => return Err(on_err(e)),
            Err(_) => {}
        }
    }
    crate::identity::sort_ranked(&mut out);
    out.truncate(6);
    Ok(out)
}

// --- rows and the cache --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Pending,
    Matched,
    Check,
    Missing,
}

impl Tier {
    fn as_str(self) -> &'static str {
        match self {
            Tier::Pending => "pending",
            Tier::Matched => "matched",
            Tier::Check => "check",
            Tier::Missing => "missing",
        }
    }

    fn parse(s: &str) -> Option<Tier> {
        Some(match s {
            "matched" => Tier::Matched,
            "check" => Tier::Check,
            "missing" => Tier::Missing,
            _ => return None,
        })
    }
}

/// The cache key: Spotify's track id, or for a track with none (a local file, a CSV row) its
/// normalized title and first artist.
pub fn key(t: &SourceTrack) -> String {
    match &t.id {
        Some(id) => id.clone(),
        None => format!(
            "~{}|{}",
            norm(&t.title),
            t.artists.first().map(|a| norm(a)).unwrap_or_default()
        ),
    }
}

fn classify(ranked: Vec<Ranked>) -> (Tier, Option<SongItem>, Vec<SongItem>) {
    let tier = match crate::identity::tier(&ranked.iter().map(|(s, _)| s).collect::<Vec<_>>()) {
        crate::identity::MatchTier::Matched => Tier::Matched,
        crate::identity::MatchTier::Check => Tier::Check,
        crate::identity::MatchTier::Missing => Tier::Missing,
    };
    let candidates: Vec<SongItem> = ranked.into_iter().map(|(_, c)| c).collect();
    let pick = (tier != Tier::Missing).then(|| candidates[0].clone());
    (tier, pick, candidates)
}

/// A song YouTube Music didn't have a week ago may be there now.
const MISSING_TTL: i64 = 7 * 86_400;

type Answer = (Tier, Option<SongItem>, Vec<SongItem>);

fn cached(state: &AppState, key: &str) -> Option<Answer> {
    let m = state.db.get_import_match(key)?;
    let tier = Tier::parse(&m.tier)?;
    if tier == Tier::Missing && !m.manual && now_secs() - m.updated_at > MISSING_TTL {
        return None;
    }
    let pick: Option<SongItem> = m.song_json.and_then(|j| serde_json::from_str(&j).ok());
    // A row whose song no longer parses (a `SongItem` shape change) is asked again.
    if tier != Tier::Missing && pick.is_none() {
        return None;
    }
    let candidates =
        m.candidates_json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    Some((tier, pick, candidates))
}

fn remember(state: &AppState, key: &str, (tier, pick, candidates): &Answer, manual: bool) {
    let song_json = pick.as_ref().and_then(|p| serde_json::to_string(p).ok());
    let candidates_json = (*tier != Tier::Matched && !candidates.is_empty())
        .then(|| serde_json::to_string(candidates).ok())
        .flatten();
    state.db.put_import_match(
        key,
        &ImportMatch {
            video_id: pick.as_ref().map(|p| p.video_id.clone()),
            song_json,
            tier: tier.as_str().to_owned(),
            candidates_json,
            manual,
            updated_at: now_secs(),
        },
    );
}

// --- the job -------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Matching,
    Review,
    Creating,
    Done,
    Failed,
    Cancelled,
}

struct Row {
    key: String,
    track: SourceTrack,
    tier: Tier,
    pick: Option<SongItem>,
    candidates: Vec<SongItem>,
    /// stream: ios-import-review: the user left it out in review (not just found nothing).
    left_out: bool,
}

/// One list being imported, its tracks as row keys in the source's order.
struct Picked {
    kind: ListKind,
    name: String,
    cover: Option<String>,
    url: Option<String>,
    keys: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResult {
    kind: ListKind,
    name: String,
    /// The browse id the UI opens: `VL…` on the account, `LOCALPLAYLIST:<n>` on this machine.
    id: String,
    local: bool,
    added: usize,
    missing: usize,
    removed: usize,
    // ── stream: ios-import-review ── an earlier import's playlist topped up, not a new one.
    updated: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Recent {
    title: String,
    artists: String,
    tier: Tier,
    thumbnail: Option<String>,
}

struct Job {
    gen: u64,
    phase: Phase,
    lists: Vec<Picked>,
    rows: Vec<Row>,
    index: HashMap<String, usize>,
    /// An "Update from Spotify" of this playlist: no review, and the writes are a diff.
    update_of: Option<String>,
    step: (usize, usize),
    /// Waiting out the hourly search budget: the unix second it frees up.
    waiting_until: Option<i64>,
    message: Option<String>,
    results: Vec<ListResult>,
    recent: VecDeque<Recent>,
    last_emit: Option<Instant>,
    // ── stream: import2 ── the streaming history to write once matched (history.rs).
    plays: Vec<crate::history::Play>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListBrief {
    kind: ListKind,
    name: String,
    count: usize,
    cover: Option<String>,
}

/// Everything the UI draws from, small enough to send four times a second.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    phase: Phase,
    total: usize,
    done: usize,
    matched: usize,
    check: usize,
    missing: usize,
    lists: Vec<ListBrief>,
    recent: Vec<Recent>,
    step: [usize; 2],
    waiting_until: Option<i64>,
    message: Option<String>,
    results: Vec<ListResult>,
    update: Option<String>,
}

impl Job {
    fn new(gen: u64, lists: Vec<SourceList>) -> Job {
        let mut rows: Vec<Row> = Vec::new();
        let mut index = HashMap::new();
        let lists = lists
            .into_iter()
            .map(|l| {
                let mut keys = Vec::new();
                let mut seen = HashSet::new();
                for track in l.tracks {
                    let k = key(&track);
                    // Twice in one playlist goes in once: YouTube refuses the second copy anyway.
                    if !seen.insert(k.clone()) {
                        continue;
                    }
                    index.entry(k.clone()).or_insert_with(|| {
                        rows.push(Row {
                            key: k.clone(),
                            track,
                            tier: Tier::Pending,
                            pick: None,
                            candidates: Vec::new(),
                            left_out: false,
                        });
                        rows.len() - 1
                    });
                    keys.push(k);
                }
                Picked { kind: l.kind, name: l.name, cover: l.cover, url: l.url, keys }
            })
            .collect();
        Job {
            gen,
            phase: Phase::Matching,
            lists,
            rows,
            index,
            update_of: None,
            step: (0, 0),
            waiting_until: None,
            message: None,
            results: Vec::new(),
            recent: VecDeque::new(),
            last_emit: None,
            plays: Vec::new(),
        }
    }

    fn resolve(&mut self, i: usize, (tier, pick, candidates): Answer) {
        let row = &mut self.rows[i];
        self.recent.push_front(Recent {
            title: row.track.title.clone(),
            artists: row.track.artists.join(", "),
            tier,
            thumbnail: pick.as_ref().and_then(|p| p.thumbnail.clone()),
        });
        self.recent.truncate(5);
        row.tier = tier;
        row.pick = pick;
        row.candidates = candidates;
    }

    fn snapshot(&self) -> Snapshot {
        let count = |t: Tier| self.rows.iter().filter(|r| r.tier == t).count();
        let pending = count(Tier::Pending);
        Snapshot {
            phase: self.phase,
            total: self.rows.len(),
            done: self.rows.len() - pending,
            matched: count(Tier::Matched),
            check: count(Tier::Check),
            missing: count(Tier::Missing),
            lists: self
                .lists
                .iter()
                .map(|l| ListBrief {
                    kind: l.kind,
                    name: l.name.clone(),
                    count: l.keys.len(),
                    cover: l.cover.clone(),
                })
                .collect(),
            recent: self.recent.iter().cloned().collect(),
            step: [self.step.0, self.step.1],
            waiting_until: self.waiting_until,
            message: self.message.clone(),
            results: self.results.clone(),
            update: self.update_of.clone(),
        }
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Matching | Phase::Creating)
    }
}

struct Importer {
    /// The last source read, kept until the next one so a failed import can be started again.
    pending: Option<spotify::Library>,
    job: Option<Job>,
    gen: u64,
}

// ponytail: one import per process, in a static rather than an AppState field. There is one
// window and one dialog; a second concurrent import is refused (`busy`), not queued.
static IMPORT: Mutex<Importer> = Mutex::new(Importer { pending: None, job: None, gen: 0 });

/// Run `f` on the job if it is still generation `gen` and still running (not cancelled, not
/// replaced by a newer import).
fn with_job<R>(gen: u64, f: impl FnOnce(&mut Job) -> R) -> Option<R> {
    let mut g = IMPORT.lock().unwrap();
    g.job.as_mut().filter(|j| j.gen == gen && j.running()).map(f)
}

fn alive(gen: u64) -> bool {
    with_job(gen, |_| ()).is_some()
}

fn emit(state: &AppState, gen: u64, force: bool) {
    let snapshot = {
        let mut g = IMPORT.lock().unwrap();
        let Some(j) = g.job.as_mut().filter(|j| j.gen == gen) else {
            return;
        };
        if !force && j.last_emit.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        j.last_emit = Some(Instant::now());
        j.snapshot()
    };
    let _ = state.host.emit("import-progress", snapshot);
}

fn set_waiting(state: &AppState, gen: u64, until: Option<i64>) {
    if with_job(gen, |j| std::mem::replace(&mut j.waiting_until, until) != until) == Some(true) {
        emit(state, gen, true);
    }
}

fn finish(state: &AppState, gen: u64, phase: Phase, message: Option<String>) {
    if with_job(gen, |j| {
        j.phase = phase;
        j.waiting_until = None;
        j.message = message;
    })
    .is_some()
    {
        emit(state, gen, true);
    }
}

/// End the job for a [`Halt`]. A cancelled one already says so.
fn halt(state: &AppState, gen: u64, h: Halt) {
    if !matches!(h, Halt::Cancelled) {
        finish(state, gen, Phase::Failed, Some(h.code()));
    }
}

// --- reading -------------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPreview {
    kind: ListKind,
    name: String,
    owner: Option<String>,
    cover: Option<String>,
    count: usize,
    skipped: usize,
    truncated: bool,
    // ── stream: import2 ── a profile playlist whose tracks are read once it is picked.
    lazy: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    lists: Vec<ListPreview>,
    // ── stream: library ── saved albums / followed artists the export holds.
    albums: usize,
    artists: usize,
    // ── stream: import2 ── the export's streaming history, when it has one.
    history: Option<crate::history::Summary>,
    /// The profile the lists were read off, for a profile link.
    profile: Option<spotify::Profile>,
}

fn keep(mut lib: spotify::Library) -> Preview {
    // ── stream: import2 ── the history's most played songs, as one more list to match.
    let history = crate::history::summary(&lib.history);
    if history.is_some() && !lib.lists.iter().any(|l| l.kind == ListKind::History) {
        let tracks: Vec<SourceTrack> = lib.lists.iter().flat_map(|l| l.tracks.clone()).collect();
        crate::history::adopt_list_tracks(&mut lib.history, tracks.iter());
        lib.lists.push(SourceList {
            kind: ListKind::History,
            name: "Listening history".into(),
            owner: None,
            cover: None,
            url: None,
            tracks: crate::history::top_tracks(&lib.history, crate::history::TOP_SONGS),
            skipped: 0,
            truncated: false,
        });
    }
    // ── end stream: import2 ──
    let preview = Preview {
        lists: lib
            .lists
            .iter()
            .map(|l| ListPreview {
                kind: l.kind,
                name: l.name.clone(),
                owner: l.owner.clone(),
                cover: l.cover.clone(),
                count: l.tracks.len(),
                skipped: l.skipped,
                truncated: l.truncated,
                lazy: l.tracks.is_empty() && l.url.is_some(),
            })
            .collect(),
        albums: lib.albums.len(),
        artists: lib.artists.len(),
        history,
        profile: None,
    };
    IMPORT.lock().unwrap().pending = Some(lib);
    preview
}

pub async fn read_link(link: &str) -> Result<Preview, String> {
    // ── stream: import2 ── a profile link lists its public playlists; several links pasted at
    // once are read one after another into one import.
    let links: Vec<&str> = link.split_whitespace().filter(|l| !l.is_empty()).collect();
    if links.len() > 1 {
        return read_links(&links).await;
    }
    if let Some(user) = spotify::parse_profile_link(link) {
        let profile = spotify::read_profile(&user).await?;
        tracing::info!(
            user = %profile.id,
            listed = profile.playlists.len(),
            total = profile.total,
            "import: read a Spotify profile"
        );
        let lists = profile.playlists.iter().map(|p| spotify::lazy_list(p, &profile.name)).collect();
        let mut preview = keep(spotify::Library { lists, ..Default::default() });
        preview.profile = Some(profile);
        return Ok(preview);
    }
    // ── end stream: import2 ──
    let (kind, id) = spotify::parse_link(link).ok_or("not_spotify")?;
    let list = spotify::read_link(kind, &id).await?;
    Ok(keep(spotify::Library { lists: vec![list], ..Default::default() }))
}

// ── stream: import2 ──
/// Several playlist / album links pasted together. A link that won't read (private, a typo) is
/// left out rather than failing the rest; all of them failing answers the first error.
async fn read_links(links: &[&str]) -> Result<Preview, String> {
    let mut lists = Vec::new();
    let mut first_err = None;
    for (i, l) in links.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let read = match spotify::parse_link(l) {
            Some((kind, id)) => spotify::read_link(kind, &id).await,
            None => Err("not_spotify".to_string()),
        };
        match read {
            Ok(list) => lists.push(list),
            Err(e) => {
                tracing::warn!(link = %l, error = %e, "import: link not read");
                first_err.get_or_insert(e);
            }
        }
    }
    if lists.is_empty() {
        return Err(first_err.unwrap_or_else(|| "not_spotify".into()));
    }
    Ok(keep(spotify::Library { lists, ..Default::default() }))
}

/// Read the tracks of picked profile playlists, which the preview only named. One that fails is
/// dropped from the import and counted.
async fn fill_lazy(lists: &mut Vec<SourceList>) -> usize {
    let mut failed = 0;
    let mut first = true;
    for l in lists.iter_mut().filter(|l| l.tracks.is_empty() && l.url.is_some()) {
        if !std::mem::take(&mut first) {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let Some(id) = spotify::list_link_id(l) else { continue };
        match spotify::read_link(LinkKind::Playlist, &id).await {
            Ok(full) => {
                l.tracks = full.tracks;
                l.skipped = full.skipped;
                l.truncated = full.truncated;
                if l.cover.is_none() {
                    l.cover = full.cover;
                }
            }
            Err(e) => {
                tracing::warn!(playlist = %id, error = %e, "import: profile playlist not read");
                failed += 1;
            }
        }
    }
    lists.retain(|l| !l.tracks.is_empty());
    failed
}
// ── end stream: import2 ──

pub fn read_file(bytes: &[u8], name: &str) -> Result<Preview, String> {
    Ok(keep(spotify::read_file(bytes, name)?))
}

// --- matching phase ------------------------------------------------------------------------------

/// Start matching the picked lists (indices into the last [`Preview`]). Refused while a cooldown
/// runs: the import would only wait on it.
pub fn start(state: &Arc<AppState>, picked: Vec<usize>) -> Result<Snapshot, String> {
    if let Some(until) = cooldown(state) {
        return Err(Halt::Cooldown(until).code());
    }
    let mut g = IMPORT.lock().unwrap();
    if g.job.as_ref().is_some_and(Job::running) {
        return Err("busy".into());
    }
    let lib = g.pending.as_ref().ok_or("nothing_read")?;
    let lists: Vec<SourceList> = picked.iter().filter_map(|&i| lib.lists.get(i).cloned()).collect();
    // ── stream: import2 ── profile playlists are read first, inside the job, so the dialog
    // shows "Matching" at once and Stop works while they load.
    let lazy = lists.iter().any(|l| l.tracks.is_empty() && l.url.is_some());
    let job = Job::new(g.gen + 1, if lazy { Vec::new() } else { lists.clone() });
    g.gen += 1;
    let gen = g.gen;
    let snapshot = job.snapshot();
    g.job = Some(job);
    drop(g);
    let state = Arc::clone(state);
    crate::rt::spawn(async move {
        if lazy {
            let mut lists = lists;
            let failed = fill_lazy(&mut lists).await;
            if lists.is_empty() {
                return finish(&state, gen, Phase::Failed, Some("private".into()));
            }
            if failed > 0 {
                tracing::info!(failed, "import: some profile playlists could not be read");
            }
            if with_job(gen, |j| *j = Job::new(gen, lists)).is_none() {
                return;
            }
        }
        run_matching(state, gen).await;
    });
    // ── end stream: import2 ──
    Ok(snapshot)
}

// ── stream: reliability ── matching hundreds of tracks is a bulk job: paced, and it gives way
// to whatever the user clicks meanwhile.
async fn run_matching(state: Arc<AppState>, gen: u64) {
    innertube::background(run_matching_paced(state, gen)).await
}

async fn run_matching_paced(state: Arc<AppState>, gen: u64) {
    let todo: Vec<(usize, String, SourceTrack)> = with_job(gen, |j| {
        j.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.tier == Tier::Pending)
            .map(|(i, r)| (i, r.key.clone(), r.track.clone()))
            .collect()
    })
    .unwrap_or_default();

    // Everything already known first, so a re-import fills in at once.
    let mut network = Vec::new();
    for (i, key, track) in todo {
        match cached(&state, &key) {
            Some(answer) => {
                with_job(gen, |j| j.resolve(i, answer));
            }
            None => network.push((i, key, track)),
        }
    }
    emit(&state, gen, true);

    let mut failures = 0;
    for (i, key, track) in network {
        match search(&state, Some(gen), &track).await {
            Ok(ranked) => {
                failures = 0;
                let answer = classify(ranked);
                remember(&state, &key, &answer, false);
                with_job(gen, |j| j.resolve(i, answer));
            }
            Err(Halt::Failed(e)) => {
                tracing::warn!(error = %e, "import: search failed");
                failures += 1;
                if failures >= 3 {
                    return finish(&state, gen, Phase::Failed, Some(e));
                }
                // Not remembered: a failed search is not an answer.
                with_job(gen, |j| j.resolve(i, (Tier::Missing, None, Vec::new())));
            }
            Err(h) => return halt(&state, gen, h),
        }
        emit(&state, gen, false);
    }

    let update = with_job(gen, |j| j.update_of.clone()).flatten();
    match update {
        Some(playlist_id) => {
            with_job(gen, |j| j.phase = Phase::Creating);
            emit(&state, gen, true);
            run_update(state, gen, playlist_id).await;
        }
        None => {
            if with_job(gen, |j| j.phase = Phase::Review).is_some() {
                emit(&state, gen, true);
            }
        }
    }
}

// --- review --------------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRow {
    key: String,
    title: String,
    artists: String,
    album: Option<String>,
    duration_ms: Option<u64>,
    tier: Tier,
    pick: Option<SongItem>,
    /// Left empty for matched rows: there can be thousands, and they rarely need a second look.
    candidates: Vec<SongItem>,
}

pub fn rows(tier: Tier) -> Vec<ReviewRow> {
    let g = IMPORT.lock().unwrap();
    let Some(j) = g.job.as_ref() else {
        return Vec::new();
    };
    j.rows
        .iter()
        .filter(|r| r.tier == tier)
        .map(|r| ReviewRow {
            key: r.key.clone(),
            title: r.track.title.clone(),
            artists: r.track.artists.join(", "),
            album: r.track.album.clone(),
            duration_ms: r.track.duration_ms,
            tier: r.tier,
            pick: r.pick.clone(),
            candidates: if tier == Tier::Matched { Vec::new() } else { r.candidates.clone() },
        })
        .collect()
}

/// The user's call on one row: a song (theirs from now on, in every later import too) or `None`
/// to leave the track out.
pub fn pick(state: &AppState, key: &str, song: Option<SongItem>) -> Result<Snapshot, String> {
    let mut g = IMPORT.lock().unwrap();
    let j = g.job.as_mut().filter(|j| j.phase == Phase::Review).ok_or("gone")?;
    let i = *j.index.get(key).ok_or("gone")?;
    let row = &mut j.rows[i];
    row.tier = if song.is_some() { Tier::Matched } else { Tier::Missing };
    row.left_out = song.is_none();
    row.pick = song;
    if row.pick.is_some() {
        let answer = (row.tier, row.pick.clone(), row.candidates.clone());
        remember(state, key, &answer, true);
    }
    Ok(j.snapshot())
}

pub fn status() -> Option<Snapshot> {
    IMPORT.lock().unwrap().job.as_ref().map(Job::snapshot)
}

/// Stop a running import (what was written stays written), or put away a finished one.
pub fn cancel(state: &AppState) {
    let mut g = IMPORT.lock().unwrap();
    let gen = match g.job.as_mut() {
        Some(j) if j.running() => {
            j.phase = Phase::Cancelled;
            j.waiting_until = None;
            j.gen
        }
        _ => {
            g.job = None;
            return;
        }
    };
    drop(g);
    emit(state, gen, true);
}

// --- creating ------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateOptions {
    /// A new name per list (its index in the job), when the user changed one. The UI also sends
    /// one for Liked Songs, whose name Rust doesn't know in the user's language.
    names: HashMap<usize, String>,
    /// On this machine rather than the account. Forced when signed out.
    local: bool,
    // ── stream: library ── leave the export's saved albums / followed artists out.
    skip_saved: bool,
}

pub fn create(state: &Arc<AppState>, opts: CreateOptions) -> Result<(), String> {
    let local = opts.local || !state.it.is_logged_in();
    // On this machine nothing goes to YouTube, so only the account needs to wait.
    if let Some(until) = cooldown(state).filter(|_| !local) {
        return Err(Halt::Cooldown(until).code());
    }
    let (gen, extras) = {
        let mut g = IMPORT.lock().unwrap();
        // stream: library: taken, so a second create of the same read doesn't search them again.
        let extras = match g.pending.as_mut() {
            Some(lib) if !opts.skip_saved => {
                (std::mem::take(&mut lib.albums), std::mem::take(&mut lib.artists))
            }
            _ => (Vec::new(), Vec::new()),
        };
        // stream: import2: the plays go with the History list, when it was picked.
        let wants_history = g.job.as_ref().is_some_and(|j| {
            j.phase == Phase::Review && j.lists.iter().any(|l| l.kind == ListKind::History)
        });
        let plays = match g.pending.as_mut() {
            Some(lib) if wants_history => std::mem::take(&mut lib.history),
            _ => Vec::new(),
        };
        let j = g.job.as_mut().filter(|j| j.phase == Phase::Review).ok_or("gone")?;
        j.phase = Phase::Creating;
        j.plays = plays;
        (j.gen, extras)
    };
    let state = Arc::clone(state);
    // ── stream: reliability ── bulk job: background priority.
    crate::rt::spawn(innertube::background(async move {
        run_create(Arc::clone(&state), gen, opts.names, local).await;
        import_extras(state, extras.0, extras.1).await;
    }));
    Ok(())
}

struct Plan {
    kind: ListKind,
    name: String,
    cover: Option<String>,
    url: Option<String>,
    keys: Vec<String>,
    songs: Vec<SongItem>,
    missing: usize,
    // ── stream: ios-import-review ── the Spotify track each song stands for, and which Spotify
    // playlist this is across imports (`None`: Liked Songs, history, a split list).
    picks: Vec<(String, SongItem)>,
    identity: Option<String>,
    /// Tracks left out in review: filed with the import so the next one leaves them out too.
    left_out: Vec<String>,
}

/// The songs a list becomes, in order, without a video twice (two Spotify versions of one song
/// can land on the same upload), and how many of its tracks have none.
fn songs_for(j: &Job, keys: &[String]) -> (Vec<SongItem>, usize) {
    let (picks, missing) = picks_for(j, keys);
    (picks.into_iter().map(|(_, s)| s).collect(), missing)
}

/// [`songs_for`] with each song's row key.
fn picks_for(j: &Job, keys: &[String]) -> (Vec<(String, SongItem)>, usize) {
    let mut seen = HashSet::new();
    let mut picks = Vec::new();
    let mut missing = 0;
    for k in keys {
        match j.index.get(k).map(|&i| &j.rows[i]) {
            Some(Row { tier: Tier::Matched | Tier::Check, pick: Some(p), .. }) => {
                if seen.insert(p.video_id.clone()) {
                    picks.push((k.clone(), p.clone()));
                }
            }
            _ => missing += 1,
        }
    }
    (picks, missing)
}

// ── stream: import2 ──
/// Write the job's plays for every song it matched (any list, not only the history's top songs).
fn write_history(state: &AppState, gen: u64) -> Option<crate::history::Written> {
    let (plays, songs) = with_job(gen, |j| {
        let songs: HashMap<String, SongItem> = j
            .rows
            .iter()
            .filter(|r| matches!(r.tier, Tier::Matched | Tier::Check))
            .filter_map(|r| Some((r.key.clone(), r.pick.clone()?)))
            .collect();
        (std::mem::take(&mut j.plays), songs)
    })?;
    let w = crate::history::write_plays(&state.db, &plays, |t| songs.get(&key(t)).cloned());
    tracing::info!(?w, "import: listening history written");
    let _ = state.host.emit("library-changed", serde_json::json!({ "scope": "all" }));
    Some(w)
}
// ── end stream: import2 ──

async fn run_create(state: Arc<AppState>, gen: u64, names: HashMap<usize, String>, local: bool) {
    let plan = with_job(gen, |j| {
        let mut plan = Vec::new();
        // Two lists of one import with the same identity (two export playlists of one name) are
        // two playlists: only the first may land on an earlier import's.
        let mut identities = HashSet::new();
        for (i, l) in j.lists.iter().enumerate() {
            let (picks, missing) = picks_for(j, &l.keys);
            let songs: Vec<SongItem> = picks.iter().map(|(_, s)| s.clone()).collect();
            let name = names
                .get(&i)
                .map(|n| n.trim())
                .filter(|n| !n.is_empty())
                .unwrap_or(&l.name)
                .to_owned();
            // stream: library: Liked Songs become likes, which have no size cap.
            let parts: Vec<Vec<(String, SongItem)>> = if local
                || l.kind == ListKind::Liked
                || songs.len() <= YTM_PLAYLIST_MAX
            {
                vec![picks]
            } else {
                picks.chunks(YTM_PLAYLIST_MAX).map(<[_]>::to_vec).collect()
            };
            let left_out: Vec<String> = l
                .keys
                .iter()
                .filter(|k| j.index.get(*k).is_some_and(|&i| j.rows[i].left_out))
                .cloned()
                .collect();
            let identity = identity(l.kind, l.url.as_deref(), &l.name)
                .filter(|_| parts.len() == 1)
                .filter(|id| identities.insert(id.clone()));
            for (n, picks) in parts.into_iter().enumerate() {
                let songs: Vec<SongItem> = picks.iter().map(|(_, s)| s.clone()).collect();
                plan.push(Plan {
                    left_out: left_out.clone(),
                    identity: identity.clone(),
                    picks,
                    kind: l.kind,
                    name: if n == 0 { name.clone() } else { format!("{name} ({})", n + 1) },
                    cover: l.cover.clone(),
                    // A split list can't be updated as one, so it isn't offered.
                    url: l
                        .url
                        .clone()
                        .filter(|_| n == 0 && missing + songs.len() <= YTM_PLAYLIST_MAX),
                    keys: l.keys.clone(),
                    songs,
                    missing: if n == 0 { missing } else { 0 },
                });
            }
        }
        j.step = (0, plan.len());
        plan
    });
    let Some(plan) = plan else {
        return;
    };
    emit(&state, gen, true);

    for p in plan {
        // ── stream: import2 ── the history list is no playlist: its plays become this app's
        // own history, with their original times.
        if p.kind == ListKind::History {
            let written = write_history(&state, gen);
            if let Some(w) = written {
                let result = ListResult {
                    kind: p.kind,
                    name: p.name,
                    id: "history".into(),
                    local: true,
                    added: w.listens,
                    missing: w.unmatched,
                    removed: 0,
                    updated: false,
                };
                with_job(gen, |j| j.results.push(result));
            }
            with_job(gen, |j| j.step.0 += 1);
            emit(&state, gen, true);
            continue;
        }
        // ── end stream: import2 ──
        // ── stream: library ── Liked Songs import as local likes, not as a playlist.
        if p.kind == ListKind::Liked {
            if !p.songs.is_empty() {
                match crate::library::import_likes(&state.db, p.songs.clone()) {
                    // stream: ios-import-review: newly liked only, so a re-import reads "0 added".
                    Ok(added) => {
                        let result = ListResult {
                            kind: p.kind,
                            name: p.name,
                            id: crate::library::LIKED_ID.into(),
                            local: true,
                            added,
                            missing: p.missing,
                            removed: 0,
                            updated: false,
                        };
                        with_job(gen, |j| j.results.push(result));
                        let _ = state
                            .host
                            .emit("library-changed", serde_json::json!({ "scope": "liked" }));
                    }
                    Err(e) => return halt(&state, gen, Halt::Failed(e)),
                }
            }
            with_job(gen, |j| j.step.0 += 1);
            emit(&state, gen, true);
            continue;
        }
        // ── end stream: library ──
        // ── stream: ios-import-review ── the same Spotify playlist imported before: top up that
        // playlist with what is new instead of making a second copy.
        if let Some(prev) = p.identity.as_deref().and_then(|i| reimport_target(&state.db, i, local))
        {
            match top_up(&state, gen, &prev, &p.picks).await {
                Ok(Some(added)) => {
                    let identity = p.identity.as_deref().unwrap_or_default();
                    let before: Vec<String> = prev.keys.iter().chain(&p.left_out).cloned().collect();
                    remember_import(&state.db, identity, &prev.id, &before, &p.picks);
                    if let Some(url) = &p.url {
                        save_source(&state, &prev.id, url, &p.keys);
                    }
                    let result = ListResult {
                        kind: p.kind,
                        name: p.name,
                        id: prev.id,
                        local,
                        added,
                        missing: p.missing,
                        removed: 0,
                        updated: true,
                    };
                    with_job(gen, |j| j.results.push(result));
                    with_job(gen, |j| j.step.0 += 1);
                    emit(&state, gen, true);
                    continue;
                }
                // Gone on the account since: a new one, as if never imported.
                Ok(None) => {}
                Err(h) => return halt(&state, gen, h),
            }
        }
        // ── end stream: ios-import-review ──
        if !p.songs.is_empty() {
            match make_playlist(&state, gen, &p.name, &p.songs, local).await {
                Ok((id, added)) => {
                    if let Some(identity) = &p.identity {
                        remember_import(&state.db, identity, &id, &p.left_out, &p.picks);
                    }
                    // On the account the cover is three more requests to YouTube (the upload),
                    // so it waits its turn like any write, and is skipped rather than pushed
                    // through a cooldown.
                    if let Some(url) = &p.cover {
                        if local || before_youtube(&state, Some(gen), Ask::Write).await.is_ok() {
                            set_cover(&state, &id, url).await;
                        }
                    }
                    if let Some(url) = &p.url {
                        save_source(&state, &id, url, &p.keys);
                    }
                    let result = ListResult {
                        kind: p.kind,
                        name: p.name,
                        id,
                        local,
                        added,
                        missing: p.missing + p.songs.len() - added,
                        removed: 0,
                        updated: false,
                    };
                    with_job(gen, |j| j.results.push(result));
                }
                Err(h) => return halt(&state, gen, h),
            }
        }
        with_job(gen, |j| j.step.0 += 1);
        emit(&state, gen, true);
    }
    finish(&state, gen, Phase::Done, None);
}

/// Add `ids` to an account playlist, answering the ones that went in. YouTube applies a batch
/// whole or not at all, so a refused one is halved until the track it won't take (taken down,
/// blocked in the region) is alone: a few requests to find it, not one per track. Pushback or an
/// expired session stops it at once.
async fn add_all(
    state: &AppState,
    gen: Option<u64>,
    playlist_id: &str,
    ids: &[String],
) -> Result<Vec<String>, Halt> {
    let client = metadata_client(state).map_err(Halt::Failed)?;
    let mut added = Vec::new();
    let mut todo: VecDeque<&[String]> = ids.chunks(100).collect();
    while let Some(piece) = todo.pop_front() {
        before_youtube(state, gen, Ask::Write).await?;
        match state.it.playlist_add_many(client, playlist_id, piece).await {
            Ok(()) => added.extend_from_slice(piece),
            Err(e) if pushed_back(&e) || matches!(e, innertube::Error::SessionExpired) => {
                return Err(halt_for(state, e));
            }
            Err(e) if piece.len() == 1 => {
                tracing::warn!(error = %e, video_id = %piece[0], "import: YouTube won't take this track")
            }
            Err(_) => {
                let (a, b) = piece.split_at(piece.len() / 2);
                todo.push_front(b);
                todo.push_front(a);
            }
        }
    }
    Ok(added)
}

/// Create the playlist and fill it. Answers the browse id the UI opens and how many went in.
async fn make_playlist(
    state: &Arc<AppState>,
    gen: u64,
    name: &str,
    songs: &[SongItem],
    local: bool,
) -> Result<(String, usize), Halt> {
    if local {
        return create_local(&state.db, name, songs).map_err(Halt::Failed);
    }
    let client = metadata_client(state).map_err(Halt::Failed)?;
    before_youtube(state, Some(gen), Ask::Write).await?;
    let id = state.it.create_playlist(client, name).await.map_err(|e| halt_for(state, e))?;
    let browse_id = format!("VL{id}");
    let ids: Vec<String> = songs.iter().map(|s| s.video_id.clone()).collect();
    let added = add_all(state, Some(gen), &browse_id, &ids).await?;
    state.db.set_playlist_tracks(&browse_id, &added);
    Ok((browse_id, added.len()))
}

/// A new playlist on this device, filled.
fn create_local(
    db: &crate::db::Db,
    name: &str,
    songs: &[SongItem],
) -> Result<(String, usize), String> {
    let id = db.create_local_playlist(name, now_secs()).map_err(|e| e.to_string())?;
    let rows = songs
        .iter()
        .map(|s| {
            let s = crate::api::playlist_row(s.clone());
            serde_json::to_string(&s).map(|json| (s.video_id, json))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    db.add_local_playlist_tracks(id, &rows, now_secs()).map_err(|e| e.to_string())?;
    Ok((format!("{LOCAL_PLAYLIST_PREFIX}{id}"), rows.len()))
}

/// Carry the Spotify cover over. Best effort: a playlist without it is still the playlist.
async fn set_cover(state: &Arc<AppState>, playlist_id: &str, url: &str) {
    let resp = crate::http::client().get(url).timeout(Duration::from_secs(20)).send().await;
    let Ok(bytes) = (match resp {
        Ok(r) => r.bytes().await,
        Err(e) => Err(e),
    }) else {
        return;
    };
    // Spotify's image CDN serves JPEG; anything else YouTube's uploader would refuse anyway.
    let ext = if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else if bytes.starts_with(b"\x89PNG") {
        "png"
    } else {
        return;
    };
    let tmp = std::env::temp_dir().join(format!("limusic-cover-{}.{ext}", rand::random::<u32>()));
    if std::fs::write(&tmp, &bytes).is_ok() {
        if let Err(e) = crate::api::store_cover(state, playlist_id, &tmp) {
            tracing::warn!(error = %e, "import: cover not set");
        }
    }
    let _ = std::fs::remove_file(&tmp);
}

fn album_score(want: &SavedAlbum, card: &BrowseItem) -> f64 {
    let title = title_score(&strip_dressing(&want.title).0, &strip_dressing(&card.title).0);
    let artist = norm(&want.artist);
    let by = !artist.is_empty()
        && format!(" {} ", norm(card.subtitle.as_deref().unwrap_or_default()))
            .contains(&format!(" {artist} "));
    0.7 * title + if by { 0.3 } else { 0.0 }
}

async fn find_album(state: &AppState, want: &SavedAlbum) -> Result<Option<BrowseItem>, Halt> {
    let client = metadata_client(state).map_err(Halt::Failed)?;
    let q = format!("{} {}", want.title, want.artist);
    before_youtube(state, None, Ask::Search).await?;
    let cards =
        state.it.search_cards(client, q.trim(), "albums").await.map_err(|e| halt_for(state, e))?;
    Ok(cards
        .into_iter()
        .map(|c| (album_score(want, &c), c))
        .filter(|(s, _)| *s >= 0.75)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c))
}

// ── stream: library ── saved albums and followed artists ─────────────────────────────────────────

/// A cached card answer: `Some(None)` = searched before, nothing matched.
fn cached_card(state: &AppState, key: &str) -> Option<Option<serde_json::Value>> {
    let m = state.db.get_import_match(key)?;
    Some(m.song_json.and_then(|j| serde_json::from_str(&j).ok()))
}

fn remember_card(state: &AppState, key: &str, card: Option<&BrowseItem>) {
    let json = card.and_then(|c| serde_json::to_string(c).ok());
    let tier = if json.is_some() { Tier::Matched } else { Tier::Missing };
    state.db.put_import_match(
        key,
        &crate::db::ImportMatch {
            video_id: None,
            song_json: json,
            tier: tier.as_str().into(),
            candidates_json: None,
            manual: false,
            updated_at: now_secs(),
        },
    );
}

/// IDENTITY (Stream D): the artist pick. Swap for identity.rs's artist matcher when it lands.
async fn find_artist(state: &AppState, name: &str) -> Result<Option<BrowseItem>, Halt> {
    let client = metadata_client(state).map_err(Halt::Failed)?;
    before_youtube(state, None, Ask::Search).await?;
    let cards =
        state.it.search_cards(client, name.trim(), "artists").await.map_err(|e| halt_for(state, e))?;
    Ok(cards
        .into_iter()
        .filter(|c| c.kind == "artist")
        .map(|c| (title_score(name, &c.title), c))
        .filter(|(s, _)| *s >= 0.9)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c))
}

/// Saved albums and followed artists from a Spotify export, into the local library. Searches
/// only, nothing is written to YouTube; paced like every import search, and cached in
/// `import_matches` (`album:` / `artist:` keys) so a re-import costs no requests. Runs after the
/// playlists, in the background; a halt (cooldown, bot check) just stops it, and what was saved
/// so far stays saved. IDENTITY (Stream D): `find_album`'s `album_score` is the album matcher.
async fn import_extras(state: Arc<AppState>, albums: Vec<SavedAlbum>, artists: Vec<String>) {
    if albums.is_empty() && artists.is_empty() {
        return;
    }
    let mut saved = 0usize;
    for a in &albums {
        let key = format!("album:{}|{}", norm(&a.title), norm(&a.artist));
        let card = match cached_card(&state, &key) {
            Some(c) => c,
            None => match find_album(&state, a).await {
                Ok(c) => {
                    remember_card(&state, &key, c.as_ref());
                    c.and_then(|c| serde_json::to_value(c).ok())
                }
                Err(Halt::Failed(e)) => {
                    tracing::warn!(error = %e, "import: album search failed");
                    continue;
                }
                Err(_) => break,
            },
        };
        if let Some(c) = card {
            saved += usize::from(crate::library::import_saved(&state.db, &c).unwrap_or(false));
        }
    }
    for name in &artists {
        let key = format!("artist:{}", norm(name));
        let card = match cached_card(&state, &key) {
            Some(c) => c,
            None => match find_artist(&state, name).await {
                Ok(c) => {
                    remember_card(&state, &key, c.as_ref());
                    c.and_then(|c| serde_json::to_value(c).ok())
                }
                Err(Halt::Failed(e)) => {
                    tracing::warn!(error = %e, "import: artist search failed");
                    continue;
                }
                Err(_) => break,
            },
        };
        if let Some(c) = card {
            saved += usize::from(crate::library::import_saved(&state.db, &c).unwrap_or(false));
        }
    }
    tracing::info!(saved, albums = albums.len(), artists = artists.len(), "import: library saves");
    let _ = state.host.emit("library-changed", serde_json::json!({ "scope": "saved" }));
}
// ── end stream: library ──

// --- importing the same playlist again (stream: ios-import-review) ------------------------------
//
// A second import of a Spotify playlist lands on the playlist the first one made: what is new on
// Spotify is appended, and everything the user did to it here (renamed it, took songs out, added
// their own, reordered) stays. Liked Songs, saved albums, followed artists and the history are
// idempotent writes already (likes and saves are upserts, plays are filed in `imported_plays`).

/// What an imported Spotify playlist became here.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Imported {
    /// The browse id: `LOCALPLAYLIST:<n>` on this device, `VL…` on the account.
    id: String,
    /// The Spotify tracks (row keys) that went in. One the user took out again stays out.
    keys: Vec<String>,
}

/// Which Spotify playlist a list is, across imports: its Spotify id when it was read off a link
/// (a profile's playlists too), else its kind and name. Exports and CSVs carry no ids, so a
/// playlist of the same name from the next export is the same playlist; a link-read list never
/// lands on an export's and the other way round. Liked Songs and the history aren't playlists.
fn identity(kind: ListKind, url: Option<&str>, name: &str) -> Option<String> {
    let kind = match kind {
        ListKind::Playlist => "playlist",
        ListKind::Album => "album",
        ListKind::Liked | ListKind::History => return None,
    };
    if let Some((link, id)) = url.and_then(spotify::parse_link) {
        let link = if link == LinkKind::Album { "album" } else { "playlist" };
        return Some(format!("{link}:{id}"));
    }
    let name = norm(name);
    (!name.is_empty()).then(|| format!("{kind}-named:{name}"))
}

fn import_key(identity: &str) -> String {
    format!("spotify_import:{identity}")
}

fn local_number(id: &str) -> Option<i64> {
    id.strip_prefix(LOCAL_PLAYLIST_PREFIX)?.parse().ok()
}

/// File what a list became: the tracks it held before (and any left out in review) and the ones
/// that went in now.
fn remember_import(
    db: &crate::db::Db,
    identity: &str,
    id: &str,
    before: &[String],
    picks: &[(String, SongItem)],
) {
    let mut seen = HashSet::new();
    let keys: Vec<String> = before
        .iter()
        .chain(picks.iter().map(|(k, _)| k))
        .filter(|k| seen.insert(k.as_str()))
        .cloned()
        .collect();
    let rec = Imported { id: id.to_owned(), keys };
    if let Ok(json) = serde_json::to_string(&rec) {
        db.set_setting(&import_key(identity), &json);
    }
}

/// The playlist an earlier import of this Spotify playlist made, when it is still there and in
/// the place this import writes to (this device or the account). Playlists imported off a link
/// before imports were filed here are found through their "Update from Spotify" link.
fn reimport_target(db: &crate::db::Db, identity: &str, local: bool) -> Option<Imported> {
    let rec = db
        .get_setting(&import_key(identity))
        .and_then(|json| serde_json::from_str::<Imported>(&json).ok())
        .or_else(|| {
            let (_, want) = identity.split_once(':').filter(|(k, _)| !k.ends_with("-named"))?;
            db.local_playlists().into_iter().find_map(|p| {
                let id = format!("{LOCAL_PLAYLIST_PREFIX}{}", p.id);
                let src: Source = serde_json::from_str(&db.get_setting(&source_key(&id))?).ok()?;
                let same = spotify::parse_link(&src.url)?.1 == want;
                same.then_some(Imported { id, keys: src.keys })
            })
        })?;
    if is_local_playlist(&rec.id) != local {
        return None;
    }
    // Deleted here (even if only for this session's undo): the next import is a fresh one.
    if local && local_number(&rec.id).and_then(|n| db.local_playlist(n)).is_none() {
        return None;
    }
    Some(rec)
}

/// The songs a re-import adds: tracks that weren't in the earlier import, whose video the
/// playlist doesn't hold already, in Spotify's order, each video once.
fn fresh(before: &[String], picks: &[(String, SongItem)], have: &HashSet<String>) -> Vec<SongItem> {
    let before: HashSet<&str> = before.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    picks
        .iter()
        .filter(|(k, _)| !before.contains(k.as_str()))
        .filter(|(_, s)| !have.contains(&s.video_id) && seen.insert(s.video_id.clone()))
        .map(|(_, s)| s.clone())
        .collect()
}

/// Top up a playlist on this device. Answers how many songs went in.
fn top_up_local(
    db: &crate::db::Db,
    prev: &Imported,
    picks: &[(String, SongItem)],
) -> Result<usize, String> {
    let n = local_number(&prev.id).ok_or("gone")?;
    let have: HashSet<String> = db
        .local_playlist_tracks(n)
        .into_iter()
        .filter_map(|(_, json, _)| serde_json::from_str::<SongItem>(&json).ok())
        .map(|s| s.video_id)
        .collect();
    let rows = fresh(&prev.keys, picks, &have)
        .into_iter()
        .map(|s| {
            let s = crate::api::playlist_row(s);
            serde_json::to_string(&s).map(|json| (s.video_id, json))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok(0);
    }
    let went_in = db.add_local_playlist_tracks(n, &rows, now_secs()).map_err(|e| e.to_string())?;
    Ok(went_in.into_iter().filter(|&b| b).count())
}

/// Top up the earlier import's playlist. `None`: it is gone from the account since.
async fn top_up(
    state: &Arc<AppState>,
    gen: u64,
    prev: &Imported,
    picks: &[(String, SongItem)],
) -> Result<Option<usize>, Halt> {
    if is_local_playlist(&prev.id) {
        return top_up_local(&state.db, prev, picks).map(Some).map_err(Halt::Failed);
    }
    let current = match current_rows(state, gen, &prev.id).await {
        Ok(rows) => rows,
        Err(Halt::Failed(e)) => {
            tracing::info!(error = %e, "import: the earlier playlist is gone, making a new one");
            return Ok(None);
        }
        Err(h) => return Err(h),
    };
    let have: HashSet<String> = current.into_iter().map(|(v, _)| v).collect();
    let add = fresh(&prev.keys, picks, &have);
    apply_account(state, gen, &prev.id, &add, &[]).await.map(Some)
}

// --- "Update from Spotify" -----------------------------------------------------------------------

/// What a playlist imported from a link remembers: the link, and the Spotify tracks it held, so an
/// update knows what is new and what left.
#[derive(Serialize, Deserialize)]
struct Source {
    url: String,
    keys: Vec<String>,
}

fn source_key(playlist_id: &str) -> String {
    format!("spotify_source:{}", playlist_id.strip_prefix("VL").unwrap_or(playlist_id))
}

fn save_source(state: &AppState, playlist_id: &str, url: &str, keys: &[String]) {
    if let Ok(json) = serde_json::to_string(&Source { url: url.to_owned(), keys: keys.to_vec() }) {
        state.db.set_setting(&source_key(playlist_id), &json);
    }
}

fn source(state: &AppState, playlist_id: &str) -> Option<Source> {
    serde_json::from_str(&state.db.get_setting(&source_key(playlist_id))?).ok()
}

/// The Spotify link a playlist was imported from, for its "Update from Spotify" banner.
pub fn source_url(state: &AppState, playlist_id: &str) -> Option<String> {
    source(state, playlist_id).map(|s| s.url)
}

pub async fn update(state: &Arc<AppState>, playlist_id: String) -> Result<Snapshot, String> {
    if let Some(until) = cooldown(state).filter(|_| !is_local_playlist(&playlist_id)) {
        return Err(Halt::Cooldown(until).code());
    }
    if IMPORT.lock().unwrap().job.as_ref().is_some_and(Job::running) {
        return Err("busy".into());
    }
    let src = source(state, &playlist_id).ok_or("gone")?;
    let (kind, id) = spotify::parse_link(&src.url).ok_or("not_spotify")?;
    let list = spotify::read_link(kind, &id).await?;
    let mut g = IMPORT.lock().unwrap();
    if g.job.as_ref().is_some_and(Job::running) {
        return Err("busy".into());
    }
    g.gen += 1;
    let mut job = Job::new(g.gen, vec![list]);
    job.update_of = Some(playlist_id);
    let gen = g.gen;
    let snapshot = job.snapshot();
    g.job = Some(job);
    drop(g);
    crate::rt::spawn(run_matching(Arc::clone(state), gen));
    Ok(snapshot)
}

/// What the playlist holds right now: each track's video id and its row handle (the
/// `set_video_id` a removal needs; for a playlist on this machine, the row id).
async fn current_rows(
    state: &AppState,
    gen: u64,
    playlist_id: &str,
) -> Result<Vec<(String, String)>, Halt> {
    if is_local_playlist(playlist_id) {
        let key: i64 = playlist_id
            .strip_prefix(LOCAL_PLAYLIST_PREFIX)
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| Halt::Failed("gone".into()))?;
        return Ok(state
            .db
            .local_playlist_tracks(key)
            .into_iter()
            .filter_map(|(row, json, _)| {
                let s: SongItem = serde_json::from_str(&json).ok()?;
                Some((s.video_id, row.to_string()))
            })
            .collect());
    }
    let client = metadata_client(state).map_err(Halt::Failed)?;
    // Reads, so paced like the searches, and the same budget.
    before_youtube(state, Some(gen), Ask::Search).await?;
    let page =
        state.it.playlist(client, playlist_id, None).await.map_err(|e| halt_for(state, e))?;
    let mut out: Vec<(String, String)> = Vec::new();
    let push = |out: &mut Vec<(String, String)>, items: Vec<SongItem>| {
        out.extend(items.into_iter().map(|s| (s.video_id, s.set_video_id.unwrap_or_default())))
    };
    push(&mut out, page.items);
    let mut token = page.continuation;
    while let Some(next) = token.take() {
        before_youtube(state, Some(gen), Ask::Search).await?;
        let more =
            state.it.playlist_continuation(client, &next).await.map_err(|e| halt_for(state, e))?;
        push(&mut out, more.items);
        token = more.continuation;
    }
    Ok(out)
}

/// Apply an update: append what is new on Spotify, take out what left it. Spotify reordering
/// its playlist is not mirrored, and neither is anything the user added here by hand.
async fn run_update(state: Arc<AppState>, gen: u64, playlist_id: String) {
    let Some(old) = source(&state, &playlist_id) else {
        return finish(&state, gen, Phase::Failed, Some("gone".into()));
    };
    let Some((name, new_keys, songs, missing)) = with_job(gen, |j| {
        let l = &j.lists[0];
        let (songs, missing) = songs_for(j, &l.keys);
        (l.name.clone(), l.keys.clone(), songs, missing)
    }) else {
        return;
    };
    let current = match current_rows(&state, gen, &playlist_id).await {
        Ok(rows) => rows,
        Err(h) => return halt(&state, gen, h),
    };
    let have: HashSet<&str> = current.iter().map(|(v, _)| v.as_str()).collect();
    let old_keys: HashSet<&str> = old.keys.iter().map(String::as_str).collect();
    let kept: HashSet<&str> = new_keys.iter().map(String::as_str).collect();

    // New on Spotify and not here already, in Spotify's order.
    let add: Vec<SongItem> = with_job(gen, |j| {
        let mut seen = HashSet::new();
        new_keys
            .iter()
            .filter(|k| !old_keys.contains(k.as_str()))
            .filter_map(|k| j.index.get(k).map(|&i| &j.rows[i]))
            .filter(|r| matches!(r.tier, Tier::Matched | Tier::Check))
            .filter_map(|r| r.pick.clone())
            .filter(|p| !have.contains(p.video_id.as_str()) && seen.insert(p.video_id.clone()))
            .collect()
    })
    .unwrap_or_default();

    // Gone from Spotify: their videos, unless a track still there maps to the same one.
    let still: HashSet<&str> = songs.iter().map(|s| s.video_id.as_str()).collect();
    let gone_videos: HashSet<String> = old
        .keys
        .iter()
        .filter(|k| !kept.contains(k.as_str()))
        .filter_map(|k| state.db.get_import_match(k)?.video_id)
        .filter(|v| !still.contains(v.as_str()))
        .collect();
    let remove: Vec<(String, String)> =
        current.iter().filter(|(v, _)| gone_videos.contains(v)).cloned().collect();

    let applied = if is_local_playlist(&playlist_id) {
        apply_local(&state, &playlist_id, &add, &remove).map_err(Halt::Failed)
    } else {
        apply_account(&state, gen, &playlist_id, &add, &remove).await
    };
    let added = match applied {
        Ok(n) => n,
        Err(h) => return halt(&state, gen, h),
    };
    save_source(&state, &playlist_id, &old.url, &new_keys);
    let result = ListResult {
        kind: ListKind::Playlist,
        name,
        id: playlist_id.clone(),
        local: is_local_playlist(&playlist_id),
        added,
        missing,
        removed: remove.len(),
        updated: true,
    };
    with_job(gen, |j| j.results.push(result));
    finish(&state, gen, Phase::Done, None);
}

fn apply_local(
    state: &AppState,
    playlist_id: &str,
    add: &[SongItem],
    remove: &[(String, String)],
) -> Result<usize, String> {
    let key: i64 = playlist_id
        .strip_prefix(LOCAL_PLAYLIST_PREFIX)
        .and_then(|n| n.parse().ok())
        .ok_or("gone")?;
    let rows: Vec<i64> = remove.iter().filter_map(|(_, r)| r.parse().ok()).collect();
    if !rows.is_empty() {
        state.db.remove_local_playlist_tracks(key, &rows, now_secs()).map_err(|e| e.to_string())?;
    }
    let add = add
        .iter()
        .map(|s| {
            let s = crate::api::playlist_row(s.clone());
            serde_json::to_string(&s).map(|json| (s.video_id, json))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if add.is_empty() {
        return Ok(0);
    }
    let went_in =
        state.db.add_local_playlist_tracks(key, &add, now_secs()).map_err(|e| e.to_string())?;
    Ok(went_in.into_iter().filter(|&b| b).count())
}

async fn apply_account(
    state: &AppState,
    gen: u64,
    playlist_id: &str,
    add: &[SongItem],
    remove: &[(String, String)],
) -> Result<usize, Halt> {
    let client = metadata_client(state).map_err(Halt::Failed)?;
    // A row without its handle can't be removed, and one bad entry would sink the whole batch.
    let remove: Vec<(String, String)> =
        remove.iter().filter(|(_, h)| !h.is_empty()).cloned().collect();
    if !remove.is_empty() {
        before_youtube(state, Some(gen), Ask::Write).await?;
        state
            .it
            .playlist_remove_many(client, playlist_id, &remove)
            .await
            .map_err(|e| halt_for(state, e))?;
        for (v, _) in &remove {
            state.db.remove_playlist_track(playlist_id, v);
        }
    }
    let ids: Vec<String> = add.iter().map(|s| s.video_id.clone()).collect();
    let added = add_all(state, Some(gen), playlist_id, &ids).await?;
    for v in &added {
        state.db.add_playlist_track(playlist_id, v);
    }
    Ok(added.len())
}

// --- Spotify links anywhere ----------------------------------------------------------------------

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Resolved {
    Song {
        song: Box<SongItem>,
    },
    Album {
        id: String,
    },
    Artist {
        id: String,
    },
    /// A playlist is imported, not opened: the UI takes it to the import dialog.
    Playlist,
}

/// What a pasted Spotify link is on YouTube Music: the song a track link plays, the album or
/// artist page an album or artist link opens. Its searches are paced and budgeted like an
/// import's: someone pasting links one after another is the same traffic.
pub async fn resolve(state: &AppState, link: &str) -> Result<Resolved, String> {
    // stream: import2: a profile link goes to the import, which lists its public playlists.
    if spotify::parse_profile_link(link).is_some() {
        return Ok(Resolved::Playlist);
    }
    let (kind, id) = spotify::parse_link(link).ok_or("not_spotify")?;
    match kind {
        LinkKind::Playlist => Ok(Resolved::Playlist),
        LinkKind::Track => {
            let track = spotify::read_track(&id).await?;
            let k = key(&track);
            let answer = match cached(state, &k) {
                Some(a) => a,
                None => {
                    let a = classify(search(state, None, &track).await.map_err(Halt::code)?);
                    remember(state, &k, &a, false);
                    a
                }
            };
            answer
                .1
                .map(|song| Resolved::Song { song: Box::new(song) })
                .ok_or_else(|| "not_found".into())
        }
        LinkKind::Album => {
            let (title, artist) = spotify::read_name(kind, &id).await?;
            let want = SavedAlbum { title, artist: artist.unwrap_or_default() };
            let card = find_album(state, &want).await.map_err(Halt::code)?.ok_or("not_found")?;
            Ok(Resolved::Album { id: card.id })
        }
        LinkKind::Artist => {
            let (name, _) = spotify::read_name(kind, &id).await?;
            let client = metadata_client(state)?;
            before_youtube(state, None, Ask::Search).await.map_err(Halt::code)?;
            let cards = state
                .it
                .search_cards(client, &name, "artists")
                .await
                .map_err(|e| halt_for(state, e).code())?;
            let want = norm(&name);
            let card = cards
                .iter()
                .find(|c| norm(&c.title) == want)
                .or_else(|| cards.first())
                .ok_or("not_found")?;
            Ok(Resolved::Artist { id: card.id.clone() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(title: &str, artists: &[&str], album: Option<&str>, secs: Option<u64>) -> SourceTrack {
        SourceTrack {
            id: None,
            title: title.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album: album.map(Into::into),
            duration_ms: secs.map(|s| s * 1000),
            explicit: None,
        }
    }

    fn song(id: &str, title: &str, artists: &str, album: Option<&str>, duration: &str) -> SongItem {
        serde_json::from_value(serde_json::json!({
            "video_id": id, "title": title, "artists": artists, "album": album, "duration": duration,
        }))
        .unwrap()
    }

    /// The candidate `src` would pick out of `cands`, and its tier.
    fn best(src: &SourceTrack, cands: Vec<SongItem>) -> (Tier, Option<String>) {
        let mut ranked = Vec::new();
        rank(src, cands, &mut ranked);
        crate::identity::sort_ranked(&mut ranked);
        let (tier, pick, _) = classify(ranked);
        (tier, pick.map(|p| p.video_id))
    }

    #[test]
    fn normalizing() {
        assert_eq!(norm("Beyoncé \u{2013} Don't Stop (Me Now)!"), "beyonce dont stop me now");
        assert_eq!(norm("Simon & Garfunkel"), "simon and garfunkel");
        assert_eq!(norm("夜に駆ける"), "夜に駆ける");
        assert_eq!(strip_dressing("Panama - 2015 Remaster").0, "Panama");
        assert_eq!(strip_dressing("Get Lucky (feat. Pharrell Williams)").0, "Get Lucky");
        assert_eq!(strip_dressing("Mask Off ft. Kendrick Lamar").0, "Mask Off");
        assert_eq!(strip_dressing("(Intro)").0, "(Intro)");
        assert_eq!(versions(&strip_dressing("Creep - Acoustic").1), ["acoustic"]);
        assert_eq!(versions(&strip_dressing("Love Story (Taylor's Version)").1), ["taylors"]);
        assert!(versions(&strip_dressing("Jump - 2015 Remaster").1).is_empty());
        assert_eq!(
            query(&src("Creep - Acoustic", &["Radiohead"], None, None)),
            "Creep acoustic Radiohead"
        );
    }

    #[test]
    fn remaster_matches_the_plain_upload() {
        let s = src("Panama - 2015 Remaster", &["Van Halen"], Some("1984 (Remastered)"), Some(210));
        assert_eq!(
            best(&s, vec![song("a", "Panama", "Van Halen", Some("1984"), "3:31")]),
            (Tier::Matched, Some("a".into()))
        );
    }

    #[test]
    fn live_loses_to_studio() {
        let s = src("Creep", &["Radiohead"], Some("Pablo Honey"), Some(238));
        let cands = vec![
            song("live", "Creep (Live)", "Radiohead", None, "4:20"),
            song("studio", "Creep", "Radiohead", Some("Pablo Honey"), "3:59"),
        ];
        assert_eq!(best(&s, cands), (Tier::Matched, Some("studio".into())));
    }

    #[test]
    fn cover_by_someone_else_is_not_a_match() {
        let s = src("Hallelujah", &["Jeff Buckley"], Some("Grace"), Some(414));
        let (tier, _) = best(&s, vec![song("c", "Hallelujah", "Pentatonix", None, "4:29")]);
        assert_ne!(tier, Tier::Matched);
    }

    #[test]
    fn featured_artist_in_the_title() {
        let s =
            src("Get Lucky", &["Daft Punk", "Pharrell Williams", "Nile Rodgers"], None, Some(369));
        let cands = vec![song(
            "g",
            "Get Lucky (feat. Pharrell Williams & Nile Rodgers)",
            "Daft Punk",
            Some("Random Access Memories"),
            "6:09",
        )];
        assert_eq!(best(&s, cands), (Tier::Matched, Some("g".into())));
    }

    #[test]
    fn wrong_length_needs_a_look() {
        // Right name, right artist, a minute too long: an extended mix or a video with an intro.
        let s = src("Blinding Lights", &["The Weeknd"], None, Some(200));
        let (tier, _) = best(&s, vec![song("x", "Blinding Lights", "The Weeknd", None, "4:22")]);
        assert_eq!(tier, Tier::Check);
    }

    #[test]
    fn explicit_breaks_the_tie() {
        let mut s = src("HUMBLE.", &["Kendrick Lamar"], None, Some(177));
        s.explicit = Some(true);
        let mut clean = song("clean", "HUMBLE.", "Kendrick Lamar", None, "2:57");
        clean.explicit = false;
        let mut dirty = song("dirty", "HUMBLE.", "Kendrick Lamar", None, "2:57");
        dirty.explicit = true;
        assert_eq!(best(&s, vec![clean, dirty]).1.as_deref(), Some("dirty"));
    }

    #[test]
    fn non_latin_titles() {
        let s = src("夜に駆ける", &["YOASOBI"], None, Some(261));
        assert_eq!(
            best(&s, vec![song("y", "夜に駆ける", "YOASOBI", None, "4:21")]),
            (Tier::Matched, Some("y".into()))
        );
    }

    #[test]
    fn nothing_close_is_missing() {
        let s = src("Some Obscure Demo", &["Nobody Known"], None, Some(100));
        assert_eq!(
            best(&s, vec![song("z", "Bohemian Rhapsody", "Queen", None, "5:55")]).0,
            Tier::Missing
        );
        assert_eq!(best(&s, vec![]).0, Tier::Missing);
    }

    #[test]
    fn album_cards() {
        let want = SavedAlbum { title: "1984 (Remastered)".into(), artist: "Van Halen".into() };
        let card = |title: &str, sub: &str| BrowseItem {
            kind: "album",
            id: "MPRE".into(),
            title: title.into(),
            subtitle: Some(sub.into()),
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: Vec::new(),
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
        };
        assert!(
            album_score(&want, &card("1984", "Album \u{2022} Van Halen \u{2022} 1984")) >= 0.75
        );
        assert!(
            album_score(&want, &card("1984", "Album \u{2022} Some Tribute Band \u{2022} 2004"))
                < 0.75
        );
    }

    #[test]
    fn requests_queue_behind_each_other() {
        let mut p = Pacer { last: None, since_break: 0 };
        let now = Instant::now();
        let gap = Duration::from_secs(2);
        assert_eq!(reserve(&mut p, now, gap), now);
        // Two more asked for at the same moment get consecutive slots, not the same one.
        assert_eq!(reserve(&mut p, now, gap), now + gap);
        assert_eq!(reserve(&mut p, now, gap), now + gap * 2);
        // After a long idle the next one goes straight away.
        let later = now + Duration::from_secs(60);
        assert_eq!(reserve(&mut p, later, gap), later);
    }

    #[test]
    fn hourly_budget() {
        let now = 10_000;
        assert_eq!(over_budget((0, 0), now), None);
        assert_eq!(over_budget((now - 10, SEARCHES_PER_HOUR - 1), now), None);
        assert_eq!(over_budget((now - 10, SEARCHES_PER_HOUR), now), Some(now - 10 + HOUR));
        // An hour on, the window starts over.
        assert_eq!(over_budget((now - HOUR, SEARCHES_PER_HOUR), now), None);
        assert_eq!(spend((now - 10, 5), now), (now - 10, 6));
        assert_eq!(spend((now - HOUR, SEARCHES_PER_HOUR), now), (now, 1));
    }

    // ── stream: ios-import-review ── importing the same Spotify playlist again.

    fn picks(ids: &[&str]) -> Vec<(String, SongItem)> {
        ids.iter().map(|id| (format!("sp_{id}"), song(id, id, "Artist", None, "3:00"))).collect()
    }

    fn videos(db: &crate::db::Db, id: &str) -> Vec<String> {
        db.local_playlist_tracks(local_number(id).unwrap())
            .into_iter()
            .map(|(_, json, _)| serde_json::from_str::<SongItem>(&json).unwrap().video_id)
            .collect()
    }

    /// What `run_create` does for one list on this device.
    fn import_local(
        db: &crate::db::Db,
        ident: &str,
        name: &str,
        p: &[(String, SongItem)],
    ) -> (String, usize, bool) {
        if let Some(prev) = reimport_target(db, ident, true) {
            let added = top_up_local(db, &prev, p).unwrap();
            remember_import(db, ident, &prev.id, &prev.keys, p);
            return (prev.id, added, true);
        }
        let songs: Vec<SongItem> = p.iter().map(|(_, s)| s.clone()).collect();
        let (id, added) = create_local(db, name, &songs).unwrap();
        remember_import(db, ident, &id, &[], p);
        (id, added, false)
    }

    #[test]
    fn playlist_identity() {
        let link = "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M";
        assert_eq!(
            identity(ListKind::Playlist, Some(link), "Today's Top Hits").as_deref(),
            Some("playlist:37i9dQZF1DXcBWIGoYBM5M")
        );
        // The name doesn't matter for a link: Spotify renames editorial lists.
        assert_eq!(
            identity(ListKind::Playlist, Some(link), "Renamed"),
            identity(ListKind::Playlist, Some(link), "Today's Top Hits")
        );
        // An export or a CSV has a name only, compared loosely.
        assert_eq!(
            identity(ListKind::Playlist, None, "Road Trip Classics").as_deref(),
            Some("playlist-named:road trip classics")
        );
        assert_eq!(
            identity(ListKind::Playlist, None, "  Road trip  CLASSICS "),
            identity(ListKind::Playlist, None, "Road Trip Classics")
        );
        assert_eq!(identity(ListKind::Liked, None, "Liked Songs"), None);
        assert_eq!(identity(ListKind::History, None, "Listening history"), None);
        assert_eq!(identity(ListKind::Playlist, None, "!!!"), None);
    }

    #[test]
    fn fresh_songs_skip_what_was_imported_or_is_there() {
        let p = picks(&["a", "b", "c", "d"]);
        let before = vec!["sp_a".to_string(), "sp_b".to_string()];
        let have: HashSet<String> = ["a".to_string(), "d".to_string()].into();
        // b was imported and taken out by the user: stays out. d is there already (added by hand).
        let ids: Vec<String> = fresh(&before, &p, &have).into_iter().map(|s| s.video_id).collect();
        assert_eq!(ids, ["c"]);
        // Two Spotify tracks on one video go in once.
        let mut twice = picks(&["e"]);
        twice.push(("sp_e2".into(), song("e", "e", "Artist", None, "3:00")));
        assert_eq!(fresh(&[], &twice, &HashSet::new()).len(), 1);
    }

    #[test]
    fn reimport_tops_up_the_same_playlist_and_keeps_user_edits() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let ident = identity(ListKind::Playlist, None, "Road Trip").unwrap();

        let (id, added, updated) = import_local(&db, &ident, "Road Trip", &picks(&["a", "b", "c"]));
        assert_eq!((added, updated), (3, false));
        assert_eq!(db.local_playlists().len(), 1);

        // The user renames it, takes b out and adds a song of their own.
        let n = local_number(&id).unwrap();
        db.edit_local_playlist(n, Some("Summer Drive"), None, now_secs()).unwrap();
        let rows = db.local_playlist_tracks(n);
        let b_row = rows.iter().find(|(_, j, _)| j.contains("\"b\"")).unwrap().0;
        db.remove_local_playlist_tracks(n, &[b_row], now_secs()).unwrap();
        let own = crate::api::playlist_row(song("mine", "Mine", "Me", None, "2:00"));
        let own = [(own.video_id.clone(), serde_json::to_string(&own).unwrap())];
        db.add_local_playlist_tracks(n, &own, now_secs()).unwrap();

        // The Spotify playlist gained d (and e, which matched nothing this time).
        let (again, added, updated) =
            import_local(&db, &ident, "Road Trip", &picks(&["a", "b", "c", "d"]));
        assert_eq!(again, id, "no second playlist");
        assert_eq!((added, updated), (1, true));
        assert_eq!(db.local_playlists().len(), 1);
        assert_eq!(db.local_playlist(n).unwrap().title, "Summer Drive");
        assert_eq!(videos(&db, &id), ["a", "c", "mine", "d"]);

        // Unchanged on Spotify: nothing to do.
        let (_, added, _) = import_local(&db, &ident, "Road Trip", &picks(&["a", "b", "c", "d"]));
        assert_eq!(added, 0);
        // A track that found no match before and does now is new.
        let (_, added, _) = import_local(&db, &ident, "Road Trip", &picks(&["a", "c", "d", "e"]));
        assert_eq!(added, 1);
        assert_eq!(videos(&db, &id), ["a", "c", "mine", "d", "e"]);

        // Somewhere else (the account) it is a new playlist; deleted here, a new one too.
        assert!(reimport_target(&db, &ident, false).is_none());
        crate::library::delete_playlist(&db.conn(), &id, now_secs()).unwrap();
        let (other, _, updated) = import_local(&db, &ident, "Road Trip", &picks(&["a"]));
        assert!(!updated);
        assert_ne!(other, id);
    }

    #[test]
    fn a_track_left_out_in_review_stays_out() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let ident = identity(ListKind::Playlist, None, "Mix").unwrap();
        let songs: Vec<SongItem> = picks(&["a", "b"]).into_iter().map(|(_, s)| s).collect();
        let (id, _) = create_local(&db, "Mix", &songs).unwrap();
        // c was left out in review: filed with the import, like run_create does.
        remember_import(&db, &ident, &id, &["sp_c".into()], &picks(&["a", "b"]));
        let prev = reimport_target(&db, &ident, true).unwrap();
        assert_eq!(prev.keys, ["sp_c", "sp_a", "sp_b"]);
        assert_eq!(top_up_local(&db, &prev, &picks(&["a", "b", "c"])).unwrap(), 0);
        assert_eq!(videos(&db, &id), ["a", "b"]);
    }

    #[test]
    fn link_imports_from_before_are_found_by_their_link() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let url = "https://open.spotify.com/playlist/2UAqTbNCObXAmAbJ0akxrl";
        let (id, _) = create_local(&db, "3 am", &[song("a", "a", "A", None, "3:00")]).unwrap();
        let src = Source { url: url.into(), keys: vec!["sp_a".into()] };
        db.set_setting(&source_key(&id), &serde_json::to_string(&src).unwrap());
        let ident = identity(ListKind::Playlist, Some(url), "3 am").unwrap();
        assert_eq!(reimport_target(&db, &ident, true).map(|r| r.id), Some(id.clone()));
        // An export's playlist of the same name is not that playlist.
        assert!(reimport_target(&db, "playlist-named:3 am", true).is_none());
    }

    #[test]
    fn likes_and_saves_are_idempotent() {
        let db = crate::db::Db::open(std::path::Path::new(":memory:")).unwrap();
        let songs: Vec<SongItem> = picks(&["a", "b"]).into_iter().map(|(_, s)| s).collect();
        assert_eq!(crate::library::import_likes(&db, songs.clone()).unwrap(), 2);
        assert_eq!(crate::library::import_likes(&db, songs).unwrap(), 0);
        assert_eq!(crate::library::liked_count(&db.conn()), 2);
        let card = serde_json::json!({
            "kind": "album", "id": "MPREb_x", "title": "1984", "subtitle": "Album • Van Halen",
        });
        assert!(crate::library::import_saved(&db, &card).unwrap());
        assert!(!crate::library::import_saved(&db, &card).unwrap());
        assert_eq!(crate::library::saved(&db.conn(), None).len(), 1);
    }

    #[test]
    fn bot_checks() {
        assert!(is_bot_check("Sign in to confirm you\u{2019}re not a bot"));
        assert!(is_bot_check("Melde dich an, um zu best\u{e4}tigen, dass du kein Bot bist"));
        assert!(is_bot_check("Connectez-vous pour confirmer que vous n'\u{ea}tes pas un robot"));
        assert!(!is_bot_check("This video is not available in your country"));
        // "both" has "bot" in it; only the word counts.
        assert!(!is_bot_check("Not available on both"));
    }
}

// ── stream: import2 ── live end-to-end check (ignored; see the file).
#[cfg(test)]
#[path = "import_e2e.rs"]
mod e2e;
