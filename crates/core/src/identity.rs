//! Song identity: one place that decides what a title *is* (stream: smart, research 22).
//!
//! Used by the Spotify import (matching), the search re-ranker, artist-page dedup, radio and
//! playlist recommendations. Ported, not invented:
//!
//! - **web-scrobbler `metadata-filter` (MIT)**: the title-cleaning set: remaster tags, "(Official
//!   Video)"/"[Lyrics]"/"(Visualizer)" YouTube dressing, "feat." tails, single/album version.
//! - **Metrolist `MusicVideoMatcher.kt` (GPL-3.0)**: the disqualifying-variant list (cover,
//!   karaoke, tribute, slowed, sped up, 8D, nightcore ...) and the rule that a variant counts when
//!   it is in the candidate but not in the source.
//! - **spotDL `matching.py` (MIT)**: hard rejects on artist and duration, forbidden words.
//! - **beets** `rec_gap_thresh`: a match is only "Matched" when the runner-up (a *different* song)
//!   trails by a clear gap.
//!
//! The karaoke bug this replaces: Limusic's `import::artist_score` looked for the source artist in
//! the candidate's *title* too, so "Fantasy (As Made Famous By Earth, Wind & Fire)" by The Karaoke
//! Crew scored a perfect artist match. Here the artist is only ever matched against the artist
//! line (rule R4), and "as made famous by" marks the candidate non-original (rule R3) wherever it
//! appears: title, album or artist line.

use std::collections::{HashMap, HashSet};

use innertube::{BrowseItem, SongItem};
use serde::Serialize;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

// --- normalising ---------------------------------------------------------------------------------

/// Lowercase, accents off, apostrophes gone ("don't" is "dont"), `&` as "and", anything else that
/// isn't a letter or digit as a space. Letters of every script survive.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfkd() {
        match c {
            _ if is_combining_mark(c) => {}
            '\'' | '\u{2019}' | '`' => {}
            '&' | '+' => out.push_str(" and "),
            _ if c.is_alphanumeric() => out.extend(c.to_lowercase()),
            _ => out.push(' '),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `" a b c "`: padded so a phrase is found on word boundaries with `contains`.
fn padded(s: &str) -> String {
    format!(" {} ", norm(s))
}

fn has_phrase(padded_hay: &str, phrase: &str) -> bool {
    padded_hay.contains(&format!(" {phrase} "))
}

/// "3:45" or "1:02:03" in seconds.
pub fn secs(d: &str) -> Option<f64> {
    d.split(':').try_fold(0.0, |acc, p| p.trim().parse::<f64>().ok().map(|n| acc * 60.0 + n))
}

// --- version classes -----------------------------------------------------------------------------

/// What kind of recording a title names. `Studio` is the canonical one: remasters, radio edits,
/// single/album versions, mono/stereo and "(Official Video)" dressing are all still `Studio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Version {
    Studio,
    Live,
    Acoustic,
    Remix,
    Extended,
    Demo,
    Instrumental,
    ReRecorded,
    /// Piano/orchestral/lo-fi/8D/a cappella/medley: the same song, dressed differently.
    Alternate,
    SpedUp,
    Slowed,
    Nightcore,
    Cover,
    Karaoke,
}

impl Version {
    /// Someone else's recording of the song: never the one the user meant unless they asked.
    pub fn non_original(self) -> bool {
        matches!(self, Version::Cover | Version::Karaoke)
    }

    /// The query/title word that asks for this version, used to decide whether a search wants it.
    fn priority(self) -> u8 {
        match self {
            Version::Karaoke => 0,
            Version::Cover => 1,
            Version::Nightcore => 2,
            Version::SpedUp => 3,
            Version::Slowed => 4,
            Version::Instrumental => 5,
            Version::Live => 6,
            Version::Acoustic => 7,
            Version::Remix => 8,
            Version::Extended => 9,
            Version::Demo => 10,
            Version::ReRecorded => 11,
            Version::Alternate => 12,
            Version::Studio => 13,
        }
    }
}

/// Phrases (already normalised) that mark a version when they appear in a title's dressing: the
/// bracketed parts and the " - " tail. Order inside a class does not matter.
const DRESSING_RULES: &[(Version, &[&str])] = &[
    (
        Version::Karaoke,
        &[
            "karaoke",
            "backing track",
            "in the style of",
            "made famous by",
            "made popular by",
            "originally performed by",
            "originally by",
            "im stil von",
            "hecho famoso por",
            "popularizado por",
            "rendu celebre par",
            "sing along",
            "singalong",
            "minus one",
        ],
    ),
    (
        Version::Cover,
        &[
            "cover",
            "covers",
            "tribute",
            "ai cover",
            "lullaby",
            "lullabies",
            "string quartet",
            "8 bit",
            "8bit",
            "parody",
            "fan made",
            "fanmade",
            "piano cover",
            "guitar cover",
            "cover version",
        ],
    ),
    (Version::Nightcore, &["nightcore"]),
    (Version::SpedUp, &["sped up", "speed up", "spedup", "sped", "super sped up", "speed"]),
    (Version::Slowed, &["slowed", "slowed down", "reverb", "slowed and reverb", "slowed reverb"]),
    (Version::Instrumental, &["instrumental", "inst", "off vocal", "no vocals"]),
    (
        Version::Live,
        &[
            "live",
            "en vivo",
            "ao vivo",
            "en directo",
            "dal vivo",
            "en concierto",
            "unplugged",
            "live version",
            "live session",
            "live sessions",
            "in concert",
            "concert",
        ],
    ),
    (Version::Acoustic, &["acoustic", "acustico", "acustica", "stripped", "akustik"]),
    (Version::Extended, &["extended", "extended mix", "extended version", "long version"]),
    (Version::Demo, &["demo", "early demo", "home demo"]),
    (
        Version::ReRecorded,
        &["taylors version", "re recorded", "rerecorded", "re record", "re recording"],
    ),
    (
        Version::Alternate,
        &[
            "piano version",
            "orchestral",
            "orchestral version",
            "lofi",
            "lo fi",
            "8d",
            "8d audio",
            "a cappella",
            "acapella",
            "acappella",
            "medley",
            "mashup",
            "reprise",
            "symphonic",
        ],
    ),
    (Version::Remix, &["remix", "rmx", "bootleg", "rework", "vip", "club mix", "dub mix", "flip"]),
];

/// "X mix" tails that are *not* a remix.
const PLAIN_MIXES: &[&str] =
    &["original mix", "radio mix", "album mix", "single mix", "main mix", "mix cut", "mixed"];

/// Phrases that are strong enough to classify even outside brackets ("Fantasy Karaoke Version",
/// "Song Nightcore"). Words like "live" or "cover" are not here: "Live Forever", "Cover Me".
const BASE_RULES: &[(Version, &[&str])] = &[
    (
        Version::Karaoke,
        &[
            "karaoke",
            "made famous by",
            "in the style of",
            "originally performed by",
            "backing track",
        ],
    ),
    (Version::Nightcore, &["nightcore"]),
    (Version::SpedUp, &["sped up", "speed up"]),
    (Version::Slowed, &["slowed and reverb", "slowed reverb", "slowed"]),
    (Version::Alternate, &["8d audio"]),
];

/// Seen in artist lines and album titles: a non-original wherever it shows.
const ARTIST_RULES: &[(Version, &[&str])] = &[
    (Version::Karaoke, &["karaoke", "backing tracks", "sing king", "party tyme", "the hit crew"]),
    (
        Version::Cover,
        &[
            "tribute",
            "vitamin string quartet",
            "rockabye baby",
            "string quartet",
            "lullaby renditions",
            "lullabies",
            "the cover crew",
            "covers",
            "8 bit arcade",
        ],
    ),
    (Version::Nightcore, &["nightcore"]),
];

/// Same song, different wrapping: these set a flag, never a version.
const REMASTER: &[&str] =
    &["remaster", "remastered", "remasterizado", "remasterizada", "remasterise"];
const EDIT: &[&str] = &[
    "radio edit",
    "single edit",
    "edit",
    "radio version",
    "clean",
    "censored",
    "short version",
    "radio",
];
const VIDEO: &[&str] = &[
    "official video",
    "official music video",
    "music video",
    "official lyric video",
    "lyric video",
    "lyrics video",
    "lyrics",
    "lyric",
    "visualizer",
    "visualiser",
    "official visualizer",
    "mv",
    "m v",
    "official mv",
    "video oficial",
    "videoclip",
    "video clip",
    "official audio",
    "audio",
    "official",
    "hd",
    "hq",
    "4k",
];

/// Video dressing that also turns up outside brackets, at the very end of a title.
const TRAILING_VIDEO: &[&str] =
    &["official music video", "official video", "official audio", "lyric video", "m v", "mv"];

/// A title taken apart.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Parsed {
    /// The bare title as written ("Fantasy").
    pub base_raw: String,
    /// The bare title normalised ("fantasy").
    pub base: String,
    /// Artists named in the title ("feat. X", "(with Y)"), normalised.
    pub feat: Vec<String>,
    /// The headline version (most specific one found).
    pub version: Version,
    /// Every version marker found.
    pub tags: Vec<Version>,
    pub remaster: bool,
    pub edit: bool,
    /// "(Official Video)", "[Lyrics]", "(Visualizer)" ... in the title.
    pub video_marker: bool,
}

impl Parsed {
    pub fn non_original(&self) -> bool {
        self.version.non_original()
    }
}

/// Split a title into the bare title and its dressing segments: each bracketed part and each
/// " - " / " | " tail. Returns `(base, segments)`, both as written.
fn split_title(title: &str) -> (String, Vec<String>) {
    let mut base = String::new();
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut depth = 0u32;
    for c in title.chars() {
        match c {
            '(' | '[' | '{' | '\u{3010}' => {
                if depth == 0 {
                    cur.clear();
                } else {
                    cur.push(' ');
                }
                depth += 1;
            }
            ')' | ']' | '}' | '\u{3011}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    segs.push(std::mem::take(&mut cur));
                    base.push(' ');
                } else {
                    cur.push(' ');
                }
            }
            _ if depth > 0 => cur.push(c),
            _ => base.push(c),
        }
    }
    if depth > 0 && !cur.is_empty() {
        segs.push(cur);
    }
    // " - tail", " | tail": each tail is its own segment.
    for sep in [" | ", " - ", " \u{2013} ", " \u{2014} ", " ~ "] {
        while let Some(i) = base.find(sep) {
            let tail = base[i + sep.len()..].to_owned();
            base.truncate(i);
            // A tail may hold several more separated parts; split it again on the same separator.
            for part in tail.split(sep) {
                segs.push(part.to_owned());
            }
        }
    }
    let base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    (base, segs)
}

/// A "feat. X" / "ft. X" / "featuring X" / "with X" segment: the names in it, normalised.
fn feat_names(seg: &str, allow_with: bool) -> Option<Vec<String>> {
    let n = norm(seg);
    let mut words = n.splitn(2, ' ');
    let first = words.next()?;
    let rest = words.next().unwrap_or("");
    let is_feat = matches!(first, "feat" | "ft" | "featuring" | "feats")
        || (allow_with && first == "with")
        || (first == "prod" || first == "produced");
    if !is_feat || rest.is_empty() {
        return None;
    }
    if first == "prod" || first == "produced" {
        return Some(Vec::new());
    }
    // "x, y and z" -> names. Normalised `&` is already "and".
    let names = rest
        .split(" and ")
        .flat_map(|p| p.split(" x "))
        .flat_map(|p| p.split(" with "))
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    Some(names)
}

fn classify_segment(seg_padded: &str, tags: &mut Vec<Version>) {
    for (v, phrases) in DRESSING_RULES {
        if phrases.iter().any(|p| has_phrase(seg_padded, p)) && !tags.contains(v) {
            tags.push(*v);
        }
    }
    // "<Name> Mix" is a remix unless it is a plain mix.
    let s = seg_padded.trim();
    if s.ends_with(" mix") || s == "mix" {
        let plain = PLAIN_MIXES.iter().any(|p| has_phrase(seg_padded, p));
        let extended = has_phrase(seg_padded, "extended mix");
        if !plain && !extended && !tags.contains(&Version::Remix) {
            tags.push(Version::Remix);
        }
    }
    // "<Anything> Version" names another recording ("Wedding Version", "TikTok Version", "Female
    // Version", "English Version") unless it is one of the plain release names or a tag already
    // said what it is (stream: bench, from the live import report).
    if (s.ends_with(" version") || s.ends_with(" ver")) && tags.is_empty() {
        let plain = PLAIN_VERSIONS.iter().any(|p| has_phrase(seg_padded, p))
            || REMASTER.iter().any(|p| has_phrase(seg_padded, p))
            || s.split(' ').next().is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()));
        if !plain {
            tags.push(Version::Alternate);
        }
    }
}

/// "X Version" tails that are the ordinary release, not another recording.
const PLAIN_VERSIONS: &[&str] = &[
    "single version",
    "album version",
    "radio version",
    "original version",
    "explicit version",
    "clean version",
    "censored version",
    "mono version",
    "stereo version",
    "lp version",
    "45 version",
    "7 version",
    "12 version",
    "full version",
    "full length version",
    "edit version",
    "short version",
    "main version",
    "dirty version",
    "uncensored version",
    "video version",
    "bonus track version",
    "deluxe version",
    "standard version",
];

/// Take a title apart. `artist_line` and `album` are only checked for non-original markers
/// (karaoke mills, tribute bands, "Karaoke Hits" albums); they never change the base title,
/// except that a leading "Artist - " on an uploaded video title is dropped when it names one of
/// the artists.
pub fn parse(title: &str, artist_line: &str, album: Option<&str>) -> Parsed {
    let (mut base_raw, mut segs) = split_title(title);
    // "Earth, Wind & Fire - Fantasy (Official Video)": the first tail is the title.
    if !segs.is_empty() && !artist_line.is_empty() {
        let artist_n = norm(artist_line);
        let base_n = norm(&base_raw);
        let lead_is_artist = !base_n.is_empty()
            && (artist_n == base_n
                || padded(artist_line).contains(&format!(" {base_n} "))
                || artist_names(artist_line).iter().any(|a| *a == base_n));
        if lead_is_artist {
            if let Some(pos) = title.find(" - ").or_else(|| title.find(" \u{2013} ")) {
                let after = &title[pos..];
                let after = after.trim_start_matches([' ', '-', '\u{2013}']);
                let (b, s) = split_title(after);
                if !b.is_empty() {
                    base_raw = b;
                    segs = s;
                }
            }
        }
    }
    let mut feat = Vec::new();
    let mut tags: Vec<Version> = Vec::new();
    let (mut remaster, mut edit, mut video_marker) = (false, false, false);
    for seg in &segs {
        if let Some(names) = feat_names(seg, true) {
            feat.extend(names);
            continue;
        }
        let p = padded(seg);
        classify_segment(&p, &mut tags);
        remaster |= REMASTER.iter().any(|w| has_phrase(&p, w));
        edit |= EDIT.iter().any(|w| has_phrase(&p, w)) && !has_phrase(&p, "radio mix");
        video_marker |= VIDEO.iter().any(|w| has_phrase(&p, w));
    }
    // A bare "feat. X" tail in the base: "Mask Off ft. Kendrick Lamar".
    let feat_at = base_raw
        .char_indices()
        .filter(|(_, c)| *c == ' ')
        .map(|(i, _)| i)
        .find(|&i| feat_names(&base_raw[i + 1..], false).is_some());
    if let Some(i) = feat_at {
        if let Some(names) = feat_names(&base_raw[i + 1..], false) {
            feat.extend(names);
        }
        base_raw.truncate(i);
        base_raw = base_raw.trim().to_owned();
    }
    // Strong markers anywhere in the title.
    let whole = padded(title);
    for (v, phrases) in BASE_RULES {
        if phrases.iter().any(|p| has_phrase(&whole, p)) && !tags.contains(v) {
            tags.push(*v);
        }
    }
    // Non-original markers in the artist line and the album.
    for hay in [Some(artist_line), album].into_iter().flatten() {
        let p = padded(hay);
        for (v, phrases) in ARTIST_RULES {
            if phrases.iter().any(|ph| has_phrase(&p, ph)) && !tags.contains(v) {
                tags.push(*v);
            }
        }
    }
    if let Some(a) = album {
        let p = padded(a);
        if ["made famous by", "in the style of", "karaoke"].iter().any(|ph| has_phrase(&p, ph))
            && !tags.contains(&Version::Karaoke)
        {
            tags.push(Version::Karaoke);
        }
    }
    // All dressing ("(Intro)"): the dressing is the title.
    if norm(&base_raw).is_empty() {
        base_raw = title.trim().to_owned();
    }
    tags.sort_by_key(|v| v.priority());
    let version = tags.first().copied().unwrap_or(Version::Studio);
    // An unbracketed video tail: "GANGNAM STYLE M/V", "Song Official Video".
    let mut base = norm(&base_raw);
    for tail in TRAILING_VIDEO {
        if let Some(cut) = base.strip_suffix(&format!(" {tail}")) {
            base = cut.to_owned();
            video_marker = true;
            break;
        }
    }
    Parsed { base, base_raw, feat, version, tags, remaster, edit, video_marker }
}

/// The names in an artist line, normalised. "A, B & C" -> [a, b, c]; a name that itself holds a
/// comma ("Earth, Wind & Fire") is also kept whole, so containment checks still find it.
pub fn artist_names(line: &str) -> Vec<String> {
    let mut out: Vec<String> = line
        .split([',', '&', '\u{2022}', '/', ';'])
        .flat_map(|p| p.split(" x "))
        .flat_map(|p| p.split(" X "))
        .flat_map(|p| p.split(" and "))
        .flat_map(|p| p.split(" feat. "))
        .flat_map(|p| p.split(" ft. "))
        .map(norm)
        .filter(|p| !p.is_empty())
        .collect();
    let whole = norm(line);
    if !whole.is_empty() && !out.contains(&whole) {
        out.push(whole);
    }
    out
}

/// The display primary artist of a line, normalised: its first name.
pub fn primary_artist(line: &str) -> String {
    line.split([',', '&', '\u{2022}']).map(norm).find(|p| !p.is_empty()).unwrap_or_default()
}

// --- the comparable view of a track --------------------------------------------------------------

/// Whatever is being compared, from a `SongItem`, a `BrowseItem` song card or a source track.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub title: String,
    /// The artist line as displayed ("A, B & C").
    pub artist_line: String,
    /// The artist runs' texts when known; otherwise empty and the line is split.
    pub artists: Vec<String>,
    /// Channel ids of the artist runs, when known.
    pub artist_ids: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    /// A video row (OMV or upload) rather than the audio track.
    pub is_video: bool,
    pub explicit: Option<bool>,
}

impl Track {
    pub fn from_song(s: &SongItem) -> Track {
        let artists: Vec<String> = s
            .artist_runs
            .iter()
            .map(|r| r.text.trim().to_owned())
            .filter(|t| !is_separator(t))
            .collect();
        let mut artist_ids: Vec<String> =
            s.artist_runs.iter().filter_map(|r| r.id.clone()).collect();
        if let Some(id) = &s.artist_id {
            if !artist_ids.contains(id) {
                artist_ids.push(id.clone());
            }
        }
        Track {
            title: s.title.clone(),
            artist_line: s.artists.clone(),
            artists,
            artist_ids,
            album: s.album.clone(),
            duration: s.duration.as_deref().and_then(secs),
            is_video: s.is_video,
            explicit: Some(s.explicit),
        }
    }

    pub fn from_card(c: &BrowseItem) -> Track {
        let artists: Vec<String> = c
            .artist_runs
            .iter()
            .map(|r| r.text.trim().to_owned())
            .filter(|t| !is_separator(t))
            .collect();
        let line = if artists.is_empty() {
            // "Song • Artist • Album" style subtitles: the artist is the part that isn't a kind.
            c.subtitle.clone().unwrap_or_default()
        } else {
            artists.join(", ")
        };
        Track {
            title: c.title.clone(),
            artist_line: line,
            artists,
            artist_ids: c.artist_runs.iter().filter_map(|r| r.id.clone()).collect(),
            album: None,
            duration: c.duration.as_deref().and_then(secs),
            is_video: c.is_video,
            explicit: Some(c.explicit),
        }
    }

    pub fn names(&self) -> Vec<String> {
        if self.artists.is_empty() {
            artist_names(&self.artist_line)
        } else {
            let mut v: Vec<String> =
                self.artists.iter().map(|a| norm(a)).filter(|a| !a.is_empty()).collect();
            let whole = norm(&self.artist_line);
            if !whole.is_empty() && !v.contains(&whole) {
                v.push(whole);
            }
            v
        }
    }

    pub fn parsed(&self) -> Parsed {
        parse(&self.title, &self.artist_line, self.album.as_deref())
    }

    pub fn primary(&self) -> String {
        self.artists.first().map(|a| norm(a)).unwrap_or_else(|| primary_artist(&self.artist_line))
    }
}

fn is_separator(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || matches!(t, "," | "&" | "\u{2022}" | "x" | "X" | "and" | "feat." | "ft.")
}

/// A track identity for display dedup: the same song by the same artist in the same version
/// (Studio covers remasters, edits, single/album versions and music videos). Live and remixes
/// stay apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct WorkKey {
    pub artist: String,
    pub base: String,
    pub version: Version,
}

pub fn work_key(t: &Track) -> WorkKey {
    let p = t.parsed();
    WorkKey { artist: t.primary(), base: p.base, version: p.version }
}

pub fn work_key_song(s: &SongItem) -> WorkKey {
    work_key(&Track::from_song(s))
}

// --- scoring -------------------------------------------------------------------------------------

/// Hard checks: failing one caps a candidate at "To check".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// G1: a different version, or someone else's recording (karaoke, cover).
    Version,
    /// G2: the source's artist is not on the candidate's artist line.
    Artist,
    /// G3: the lengths are too far apart.
    Duration,
    /// G4: the titles barely agree.
    Title,
}

#[derive(Debug, Clone, Serialize)]
pub struct Score {
    /// 0..1, what the tiers read.
    pub value: f64,
    /// `value` minus a penalty per failed gate: what candidates are ranked by.
    pub rank: f64,
    pub gates: Vec<Gate>,
    pub work: WorkKey,
}

/// Tier thresholds (research 22 §3).
pub const MATCHED: f64 = 0.85;
pub const CHECK: f64 = 0.60;
/// beets' gap: the best and the best *different* song closer than this is not a sure match.
pub const GAP: f64 = 0.08;
const GATE_PENALTY: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchTier {
    Matched,
    Check,
    Missing,
}

fn dice(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    strsim::sorensen_dice(a, b)
}

/// R4: how much of the source's artist list the candidate's *artist line* names. The first
/// artist has to be on the line; the others may also be named in the candidate title's
/// "feat." part, which is where YouTube often puts them. The title proper is never searched.
pub fn artist_score(src: &Track, cand: &Track, cand_parsed: &Parsed) -> f64 {
    let src_names: Vec<String> = if src.artists.is_empty() {
        vec![norm(&src.artist_line)]
    } else {
        src.artists.iter().map(|a| norm(a)).collect()
    };
    let Some((first, rest)) = src_names.split_first() else { return 0.5 };
    if first.is_empty() {
        return 0.5;
    }
    let line = padded(&cand.artist_line);
    let pieces = cand.names();
    // A shared channel id is the strongest proof there is.
    let shares_id =
        !src.artist_ids.is_empty() && cand.artist_ids.iter().any(|id| src.artist_ids.contains(id));
    // An uploaded video titled "Artist - Title" names its artist in the title by convention: the
    // part before the dash may count, the title proper never does.
    let ugc_lead: Option<String> = (cand.is_video && cand.title.contains(" - "))
        .then(|| cand.title.split(" - ").next().map(norm).unwrap_or_default());
    let on_line = |a: &String| {
        !a.is_empty()
            && (line.contains(&format!(" {a} "))
                || pieces.iter().any(|p| strsim::jaro_winkler(p, a) >= 0.92))
    };
    let in_feat = |a: &String| cand_parsed.feat.iter().any(|f| f == a || f.contains(a.as_str()));
    let ugc_names =
        |a: &String| ugc_lead.as_ref().is_some_and(|pre| padded(pre).contains(&format!(" {a} ")));
    let first_found = on_line(first) || ugc_names(first) || (shares_id && rest.is_empty());
    let others = (!rest.is_empty()).then(|| {
        rest.iter().filter(|a| on_line(a) || in_feat(a) || ugc_names(a)).count() as f64
            / rest.len() as f64
    });
    if first_found || shares_id {
        0.7 + 0.3 * others.unwrap_or(1.0)
    } else {
        // Listed the other way round (a collab): partial credit, and the gate fails.
        0.35 * others.unwrap_or(0.0)
    }
}

fn same_album(a: Option<&str>, b: Option<&str>) -> Option<bool> {
    let (a, b) = (a?, b?);
    let (a, b) = (parse(a, "", None).base, parse(b, "", None).base);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some(a == b || a.contains(&b) || b.contains(&a))
}

/// The candidate's artist line names someone who is neither a source artist nor featured in the
/// source title.
fn extra_credit(src: &Track, cand: &Track, src_parsed: &Parsed) -> bool {
    let mut known: Vec<String> = src.names();
    known.extend(src_parsed.feat.iter().cloned());
    let whole = norm(&cand.artist_line);
    let pieces: Vec<String> = if cand.artists.is_empty() {
        artist_names(&cand.artist_line)
    } else {
        cand.artists.iter().map(|a| norm(a)).collect()
    };
    pieces.iter().filter(|p| !p.is_empty() && **p != whole || pieces.len() == 1).any(|p| {
        !known.iter().any(|k| {
            k == p
                || strsim::jaro_winkler(k, p) >= 0.92
                || k.contains(p.as_str())
                || p.contains(k.as_str())
        })
    }) && !pieces.is_empty()
}

/// A title with its "feat. X" parts dropped, normalised: the words that name the recording.
fn title_sans_feat(title: &str) -> String {
    let (base, segs) = split_title(title);
    let mut out = norm(&base);
    for seg in segs.iter().filter(|s| feat_names(s, true).is_none()) {
        out.push(' ');
        out.push_str(&norm(seg));
    }
    out.trim().to_owned()
}

/// One line is written in Latin letters and the other has none ("Kino" / "Кино").
fn other_script(a: &str, b: &str) -> bool {
    let latin = |s: &str| s.chars().any(|c| c.is_ascii_alphabetic());
    let letters = |s: &str| s.chars().any(|c| c.is_alphabetic());
    letters(a) && letters(b) && latin(a) != latin(b)
}

/// R5: lengths within 3 s count fully, falling to nothing at 15 s.
fn duration_score(delta: f64) -> f64 {
    (1.0 - (delta - 3.0).max(0.0) / 12.0).clamp(0.0, 1.0)
}

/// How sure we are that `cand` (at `rank_index` in YouTube's answer) is `src`. Research 22 R7:
/// `0.35 title + 0.30 artist + 0.20 duration + 0.05 album + 0.05 type + 0.05 rank prior`, a
/// missing signal dropping out of the denominator, then the gates.
pub fn score(src: &Track, cand: &Track, rank_index: usize) -> Score {
    let sp = src.parsed();
    let cp = cand.parsed();
    let mut title = dice(&sp.base, &cp.base).max(dice(&norm(&src.title), &norm(&cand.title)));
    // An upload titled "Artist - Title" whose channel isn't the artist's: compare the part after
    // the dash too.
    if cand.is_video {
        if let Some((_, after)) = cand.title.split_once(" - ") {
            title = title.max(dice(&sp.base, &parse(after, "", None).base));
        }
    }
    let mut artist = artist_score(src, cand, &cp);
    // stream: bench: the same artist written in another script ("Kino" / "Кино", "Jay Chou" /
    // "周杰倫") can't match by name; an exact title in the same length counts as the artist's own
    // upload. Non-originals never get here: their markers gate them anyway.
    if artist < 0.7 && !cand.is_video && other_script(&src.artist_line, &cand.artist_line) {
        let same_len =
            matches!((src.duration, cand.duration), (Some(a), Some(b)) if (a - b).abs() <= 5.0);
        if same_len && !sp.base.is_empty() && sp.base == cp.base {
            artist = 0.7;
        }
    }
    let mut num = 0.35 * title + 0.30 * artist;
    let mut den = 0.65;
    let mut gates = Vec::new();
    if let (Some(a), Some(b)) = (src.duration, cand.duration) {
        let delta = (a - b).abs();
        num += 0.20 * duration_score(delta);
        den += 0.20;
        let limit = if sp.edit != cp.edit { 30.0 } else { 15.0 };
        if delta > limit {
            gates.push(Gate::Duration);
        }
    }
    if let Some(same) = same_album(src.album.as_deref(), cand.album.as_deref()) {
        num += if same { 0.05 } else { 0.0 };
        den += 0.05;
    }
    // R6: the audio track beats an official video beats an upload.
    let kind = if !cand.is_video {
        1.0
    } else if artist >= 0.7 {
        0.7
    } else {
        0.2
    };
    num += 0.05 * kind + 0.05 / (1.0 + rank_index as f64);
    den += 0.10;
    let mut value = num / den;
    if let (Some(a), Some(b)) = (src.explicit, cand.explicit) {
        if a != b {
            value -= 0.04;
        }
    }
    // stream: bench: a credit the source doesn't have ("Babydoll" by Jamie Miller vs the duet
    // release by "Jamie Miller & Antonio Cipriano") is usually another release of the song: the
    // row with exactly the source's credits wins the tie.
    // Ranking only: the tier keeps the plain score, so a credit spelled another way ("Vienna
    // Philharmonic" / "Wiener Philharmoniker") never costs a match its tier.
    let credit_penalty = if extra_credit(src, cand, &sp) { 0.03 } else { 0.0 };
    // stream: bench: every version tag has to agree, not just the headline one: "State Of Grace
    // (Acoustic Version) [Taylor's Version]" is not the 2012 "State Of Grace - Acoustic".
    let tagset = |p: &Parsed| {
        let mut t: Vec<Version> =
            p.tags.iter().copied().filter(|v| *v != Version::Studio).collect();
        t.sort();
        t.dedup();
        t
    };
    if sp.version != cp.version
        || sp.non_original() != cp.non_original()
        || tagset(&sp) != tagset(&cp)
    {
        gates.push(Gate::Version);
    }
    if artist < 0.7 {
        gates.push(Gate::Artist);
    }
    if title < 0.6 {
        gates.push(Gate::Title);
    }
    let value = value.clamp(0.0, 1.0);
    // Ranking only: between equal scores, the row whose whole title reads like the source's wins
    // ("Waka Waka (This Time for Africa)" over the Spanish "Waka Waka (Esto es Africa)").
    let full_title = 0.1 * dice(&title_sans_feat(&src.title), &title_sans_feat(&cand.title));
    let rank = value - GATE_PENALTY * gates.len() as f64 - credit_penalty + full_title;
    Score { value, rank, gates, work: work_key(cand) }
}

/// Sort scored candidates best first (by `rank`, stable so YouTube's order breaks ties).
pub fn sort_ranked<T>(v: &mut [(Score, T)]) {
    v.sort_by(|a, b| b.0.rank.total_cmp(&a.0.rank));
}

/// The same work, allowing a misspelt upload artist ("Avicil" for "Avicii"): a typo'd re-upload
/// of the same song is not a *different* song for the gap rule (stream: bench).
fn same_work(a: &WorkKey, b: &WorkKey) -> bool {
    a == b
        || (a.base == b.base
            && a.version == b.version
            && strsim::jaro_winkler(&a.artist, &b.artist) >= 0.9)
}

/// Import tiers over candidates already sorted by [`sort_ranked`]:
/// 1. **Matched**: value >= 0.85, no gate failed, and the best *different* song (another work
///    key) trails by at least 0.08.
/// 2. **To check**: value >= 0.60, or a gate failed, or the gap is too small.
/// 3. **Not found**: below 0.60.
pub fn tier(sorted: &[&Score]) -> MatchTier {
    let Some(best) = sorted.first() else { return MatchTier::Missing };
    if best.value < CHECK {
        return MatchTier::Missing;
    }
    let runner = sorted.iter().skip(1).find(|s| !same_work(&s.work, &best.work));
    let clear = runner.map_or(true, |r| best.rank - r.rank >= GAP);
    if best.value >= MATCHED && best.gates.is_empty() && clear {
        MatchTier::Matched
    } else {
        MatchTier::Check
    }
}

// --- search re-rank ------------------------------------------------------------------------------

/// What a search query asks for: its version words, and its words.
#[derive(Debug, Clone)]
pub struct Query {
    pub words: HashSet<String>,
    pub wants: HashSet<Version>,
    /// The version phrases the query used ("karaoke", "live"), to tell "creep karaoke" from
    /// "live forever", where the word is part of the title.
    pub phrases: Vec<String>,
    pub wants_video: bool,
}

impl Query {
    pub fn new(q: &str) -> Query {
        let p = padded(q);
        let mut wants = HashSet::new();
        let mut used = Vec::new();
        for (v, phrases) in DRESSING_RULES.iter().chain(BASE_RULES) {
            for ph in phrases.iter().filter(|ph| has_phrase(&p, ph)) {
                wants.insert(*v);
                used.push(ph.to_string());
            }
        }
        if has_phrase(&p, "mix") {
            wants.insert(Version::Remix);
            used.push("mix".into());
        }
        let wants_video =
            ["video", "mv", "official video", "music video"].iter().any(|w| has_phrase(&p, w));
        let words = norm(q).split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect();
        Query { words, wants, phrases: used, wants_video }
    }
}

/// Search re-rank score for one row (research 22 "Search re-rank", 06 §B.3): YouTube's order is
/// the base (a strong prior), then a version or non-original the query did not ask for costs
/// 0.3, the audio track gains 0.1, and an artist named in the query gains 0.1.
pub fn search_score(q: &Query, t: &Track, rank_index: usize) -> f64 {
    let p = t.parsed();
    let mut s = 1.0 / (1.0 + rank_index as f64 * 0.35);
    let unasked = p.tags.iter().any(|v| *v != Version::Studio && !q.wants.contains(v));
    if p.tags.iter().any(|v| q.wants.contains(v)) {
        s += 0.2;
    } else {
        // The query asks for a version ("creep karaoke") with a word this title doesn't simply
        // contain ("live forever" is a title, not a request): rows without it step down.
        let title = padded(&t.title);
        if q.phrases.iter().any(|ph| !has_phrase(&title, ph)) {
            s -= 0.25;
        }
    }
    if unasked {
        s -= 0.45;
        if p.non_original() {
            s -= 0.1;
        }
    }
    if !t.is_video {
        s += 0.1;
    } else if q.wants_video {
        s += 0.1;
    }
    let names = t.names();
    if names.iter().any(|n| {
        let ws: Vec<&str> = n.split(' ').collect();
        !ws.is_empty() && ws.iter().all(|w| q.words.contains(*w))
    }) {
        s += 0.1;
    }
    s
}

/// Re-rank a list of rows for `query`, then move every later row with the same work key as an
/// earlier one behind the rest ("Other versions" stay reachable, just not on top). `track` maps a
/// row to its comparable view.
pub fn rerank<T>(query: &str, items: Vec<T>, track: impl Fn(&T) -> Track) -> Vec<T> {
    rerank_by(query, items, track, |_, _, _| 0.0)
}

/// [`rerank`] for song-search rows that carry YouTube's play count: popularity joins the score.
///
/// YouTube's song-filter order is not stable: the same "daft punk" query comes back shuffled
/// between requests (fresh visitor data, A/B buckets), so a rank-only score showed different
/// Songs on every visit. Plays are stable. When the query *is* the row's artist ("daft punk"),
/// plays dominate, like Spotify's Songs for an artist query (Instant Crush, One More Time, Lose
/// Yourself to Dance, Around the World); otherwise they only break near-ties, so "shake it off"
/// still puts the title match above a more-played song of the same artist.
pub fn rerank_search_songs(query: &str, items: Vec<SongItem>) -> Vec<SongItem> {
    rerank_search_songs_with(query, items, SEARCH_ARTIST_PLAYS_WEIGHT, SEARCH_PLAYS_WEIGHT)
}

/// [`rerank_search_songs`] with the two plays weights spelled out (the benchmark grids them).
pub fn rerank_search_songs_with(
    query: &str,
    items: Vec<SongItem>,
    artist_w: f64,
    other_w: f64,
) -> Vec<SongItem> {
    // ── stream: bench2 ── the artist intent is decided once per query (docs/bench/search.md).
    // Before, any row whose artist name merely *contained* every query word got plays at full
    // weight, so "uptown funk" put the band Uptown Funk Empire above Mark Ronson, and "hello adele"
    // the "Hello Adele Tribute" act above Adele's Hello. Now a row is the artist's when its name
    // *is* the query (or one typo away, or the query is the start of the name: "billie elish",
    // "kend"), and the plays weigh in fully only when that artist's most-played row beats every
    // row titled like the query ("halo" is Beyoncé's song, not the game soundtrack act Halo).
    let qn = norm(query);
    let artist_rows: Vec<bool> =
        items.iter().map(|s| names_query(&qn, &Track::from_song(s))).collect();
    let plays_of = |s: &SongItem| s.play_count.as_deref().map(play_count).unwrap_or(0.0);
    let best = |pick: &dyn Fn(usize, &SongItem) -> bool| {
        items
            .iter()
            .enumerate()
            .filter(|(i, s)| pick(*i, s))
            .map(|(_, s)| plays_of(s))
            .fold(0.0, f64::max)
    };
    let best_artist = best(&|i, _| artist_rows[i]);
    let best_title = best(&|i, s| !artist_rows[i] && titled_like(&qn, &s.title));
    let artist_intent = best_artist > 0.0 && best_artist >= best_title;
    let by_id: HashMap<String, bool> =
        items.iter().zip(&artist_rows).map(|(s, a)| (s.video_id.clone(), *a)).collect();
    rerank_by(query, items, Track::from_song, |s, _, _| {
        // Without an artist intent, a row titled exactly like the query outranks one that only
        // matched through an act's name ("halo": Beyoncé's Halo over the act Halo).
        let title = if !artist_intent && parse(&s.title, "", None).base == qn {
            SEARCH_TITLE_BONUS
        } else {
            0.0
        };
        let plays = plays_of(s);
        if plays <= 0.0 {
            return title;
        }
        let full = artist_intent && by_id.get(&s.video_id).copied().unwrap_or(false);
        title + (if full { artist_w } else { other_w }) * plays.log10()
    })
}

// ── stream: podtop ── artist queries follow the artist page's "Top songs" (docs/streams/podtop.md).

/// Whether the query asks for an artist (the same rule [`rerank_search_songs`] uses): some row's
/// artist is the query (or one typo away, or a cut-off prefix), and that artist's best row
/// out-plays every row titled like the query. Returns the matched artist's normalised name.
pub fn search_artist_intent(query: &str, items: &[SongItem]) -> Option<String> {
    let qn = norm(query);
    let plays_of = |s: &SongItem| s.play_count.as_deref().map(play_count).unwrap_or(0.0);
    let mut best_artist: Option<(f64, String)> = None;
    let mut best_title = 0.0f64;
    for s in items {
        let t = Track::from_song(s);
        if names_query(&qn, &t) {
            let p = plays_of(s);
            if best_artist.as_ref().map_or(true, |(b, _)| p > *b) {
                let name = t.names().into_iter().next().unwrap_or_default();
                best_artist = Some((p, name));
            }
        } else if titled_like(&qn, &s.title) {
            best_title = best_title.max(plays_of(s));
        }
    }
    best_artist.filter(|(p, _)| *p > 0.0 && *p >= best_title).map(|(_, n)| n)
}

/// For a query that asks for an artist, blend in the artist page's "Top songs" shelf, which
/// YouTube Music keeps in roughly *recent* popularity (Spotify's Songs for an artist follow
/// recent streams; the search rows only carry all-time plays). `ranked` is
/// [`rerank_search_songs`]'s answer; `artist` and `top` the artist's name and its page's Top songs.
/// Each row's place is its rank in `ranked` plus its rank on the shelf (rows not on the shelf
/// count as last there): a parameter-free Borda count, ties kept in `ranked` order. Putting the
/// shelf first outright scored worse (docs/streams/podtop.md). No artist intent, or a shelf of
/// another artist: `ranked` unchanged.
pub fn apply_artist_top_songs(
    query: &str,
    ranked: Vec<SongItem>,
    artist: &str,
    top: &[SongItem],
) -> Vec<SongItem> {
    let Some(who) = search_artist_intent(query, &ranked) else { return ranked };
    apply_artist_top_songs_as(&who, ranked, artist, top)
}

// ── stream: search-pop ── the Top card's artist as the query's artist (docs/streams/search-pop.md).

/// Monthly audience from which YouTube's artist card names the artist a partial query means.
pub const CARD_INTENT_AUDIENCE: f64 = 1e6;

/// The artist a partial query means, from YouTube's own popularity-aware Top card: an artist
/// card whose name *begins* with the query at a word ("taylor" → Taylor Swift, "billie" → Billie
/// Eilish, "sabrina" → Sabrina Carpenter) or is it, with an audience of at least
/// [`CARD_INTENT_AUDIENCE`]. The row-based rule ([`search_artist_intent`]) only knows a name that
/// *is* the query, one typo from it, or cut inside a word, so "billie" went to the K-pop group
/// Billlie (one typo, 89M plays) and "taylor" had no artist at all. Returns the normalised name.
pub fn card_artist_intent(query: &str, card: &innertube::BrowseItem) -> Option<String> {
    if card.kind != "artist" {
        return None;
    }
    let (qn, name) = (norm(query), norm(&card.title));
    if qn.chars().count() < 3 || name.is_empty() {
        return None;
    }
    let begins = name == qn || name.starts_with(&format!("{qn} "));
    let audience = crate::podcasts::search::card_audience(card).unwrap_or(0.0);
    (begins && audience >= CARD_INTENT_AUDIENCE).then_some(name)
}

/// The query's artist with the card's evidence: [`search_artist_intent`] over the rows, unless
/// YouTube's Top card names an artist the query begins ([`card_artist_intent`]) whose rows
/// out-play the rows' pick. An artist whose name *is* the query keeps it when their rows play as
/// much ("drake" stays Drake either way).
pub fn search_artist_intent_with(query: &str, items: &[SongItem], card: Option<&str>) -> Option<String> {
    let base = search_artist_intent(query, items);
    let Some(card) = card else { return base };
    if base.as_deref() == Some(card) {
        return base;
    }
    let plays_by = |who: &str| {
        items
            .iter()
            .filter(|s| Track::from_song(s).names().iter().any(|n| n == who))
            .map(|s| s.play_count.as_deref().map(play_count).unwrap_or(0.0))
            .fold(0.0, f64::max)
    };
    let card_best = plays_by(card);
    if card_best <= 0.0 {
        return base;
    }
    match base {
        Some(b) if b == norm(query) && plays_by(&b) >= card_best => Some(b),
        _ => Some(card.to_owned()),
    }
}

/// [`rerank_search_songs`] for a query whose artist is `who` (normalised), decided elsewhere: that
/// artist's rows by plays (weight [`SEARCH_ARTIST_PLAYS_WEIGHT`]), the rest as near-ties.
pub fn rerank_search_songs_for(query: &str, items: Vec<SongItem>, who: &str) -> Vec<SongItem> {
    rerank_by(query, items, Track::from_song, |s, _, t| {
        let plays = s.play_count.as_deref().map(play_count).unwrap_or(0.0);
        if plays <= 0.0 {
            return 0.0;
        }
        let w = if t.names().iter().any(|n| n == who) { SEARCH_ARTIST_PLAYS_WEIGHT } else { SEARCH_PLAYS_WEIGHT };
        w * plays.log10()
    })
}

/// [`apply_artist_top_songs`] for an artist decided elsewhere (`who`, normalised).
pub fn apply_artist_top_songs_as(who: &str, ranked: Vec<SongItem>, artist: &str, top: &[SongItem]) -> Vec<SongItem> {
    if top.is_empty() || norm(artist) != who {
        return ranked;
    }
    let shelf: Vec<WorkKey> = top.iter().map(work_key_song).collect();
    let n = ranked.len();
    let mut scored: Vec<(usize, usize, SongItem)> = ranked
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let k = work_key_song(&s);
            (i + shelf.iter().position(|x| *x == k).unwrap_or(n), i, s)
        })
        .collect();
    scored.sort_by_key(|x| (x.0, x.1));
    scored.into_iter().map(|x| x.2).collect()
}

/// Weight of `log10(plays)` on rows that aren't the queried artist's: a near-tie breaker only.
pub const SEARCH_PLAYS_WEIGHT: f64 = 0.08;
/// Bonus for a row titled exactly like a query with no artist intent.
pub const SEARCH_TITLE_BONUS: f64 = 0.3;
/// Weight of `log10(plays)` on the queried artist's rows: plays lead (Spotify's Songs for an
/// artist are their most played), YouTube's order still breaks near-ties.
pub const SEARCH_ARTIST_PLAYS_WEIGHT: f64 = 1.0;

/// One of the row's artists is the query: the same name, one typo away (names of 5+ letters), or
/// the query is the start of the name, cut inside a word (4+ letters: "taylor sw", "kend"; not
/// "hello adele" for the act "Hello Adele Tribute").
fn names_query(qn: &str, t: &Track) -> bool {
    if qn.is_empty() {
        return false;
    }
    t.names().iter().any(|n| {
        n == qn
            || (qn.chars().count() >= 5
                && edit_distance(n, qn) <= if n.chars().count() > 9 { 2 } else { 1 })
            || (qn.chars().count() >= 4
                && n.starts_with(qn)
                && n[qn.len()..].chars().next().is_some_and(|c| c != ' '))
    })
}

/// The title (without its dressing) is the query, or starts with it.
fn titled_like(qn: &str, title: &str) -> bool {
    let base = parse(title, "", None).base;
    !qn.is_empty() && (base == qn || base.starts_with(&format!("{qn} ")))
}

/// Optimal-string-alignment distance (Damerau–Levenshtein with adjacent transpositions).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// [`rerank`] with an extra per-row score term.
pub fn rerank_by<T>(
    query: &str,
    items: Vec<T>,
    track: impl Fn(&T) -> Track,
    bonus: impl Fn(&T, &Query, &Track) -> f64,
) -> Vec<T> {
    let q = Query::new(query);
    let mut scored: Vec<(f64, WorkKey, T)> = items
        .into_iter()
        .enumerate()
        .map(|(i, it)| {
            let t = track(&it);
            (search_score(&q, &t, i) + bonus(&it, &q, &t), work_key(&t), it)
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    // One row per work in the slot of its best-scored row; the representative is the audio track
    // when the group has one (unless the query asks for a video), then the best score.
    let mut groups: Vec<(WorkKey, Vec<(T, bool)>)> = Vec::new();
    let mut at: HashMap<WorkKey, usize> = HashMap::new();
    for (_, k, it) in scored {
        let video = track(&it).is_video;
        match at.get(&k) {
            Some(&g) => groups[g].1.push((it, video)),
            None => {
                at.insert(k.clone(), groups.len());
                groups.push((k, vec![(it, video)]));
            }
        }
    }
    let (mut first, mut dupes) = (Vec::new(), Vec::new());
    for (_, mut members) in groups {
        let rep = if q.wants_video { 0 } else { members.iter().position(|(_, v)| !v).unwrap_or(0) };
        let (head, _) = members.remove(rep);
        first.push(head);
        dupes.extend(members.into_iter().map(|(it, _)| it));
    }
    first.extend(dupes);
    first
}

/// The unfiltered search page's ranking (moved from `smart::recs`, stream bench2): re-rank the
/// Songs shelf, and when the Top Result is a music video or a version the query didn't ask for,
/// swap in the audio track of the same song from the Songs shelf (research 22).
pub fn rerank_search_all(query: &str, mut r: innertube::SearchResults) -> innertube::SearchResults {
    r.songs = rerank(query, std::mem::take(&mut r.songs), Track::from_card);
    if let Some(top) = r.top.first().filter(|t| t.kind == "song") {
        let q = Query::new(query);
        let tt = Track::from_card(top);
        let tp = tt.parsed();
        let unasked = tp.tags.iter().any(|v| !q.wants.contains(v));
        if top.is_video || unasked {
            let better = r.songs.iter().find(|c| {
                let ct = Track::from_card(c);
                let cp = ct.parsed();
                !c.is_video
                    && cp.base == tp.base
                    && ct.primary() == tt.primary()
                    && cp.tags.iter().all(|v| q.wants.contains(v))
            });
            if let Some(b) = better.cloned() {
                r.top[0] = b;
            }
        }
    }
    // ── stream: bench2 ── an album on top whose title track is a hit gives way to the track, as
    // on Spotify ("hotel california", "starboy": the song; "un verano sin ti": still the album,
    // its title track is no hit). docs/bench/search.md.
    if let Some(top) = r.top.first().filter(|t| t.kind == "album") {
        let p = padded(query);
        let asks_album = ["album", "ep", "lp"].iter().any(|w| has_phrase(&p, w));
        let parts: Vec<&str> = top.subtitle.as_deref().unwrap_or("").split(" • ").collect();
        let album_artist = match parts.as_slice() {
            [_, a, ..] => norm(a),
            _ => String::new(),
        };
        let album_base = parse(top.title.split(" (").next().unwrap_or(&top.title), "", None).base;
        let track = r.songs.iter().find(|c| {
            let ct = Track::from_card(c);
            !c.is_video
                && ct.parsed().base == album_base
                && ct.names().contains(&album_artist)
                && c.play_count.as_deref().map(play_count).unwrap_or(0.0) >= TITLE_TRACK_HIT_PLAYS
        });
        if let (false, Some(t)) = (asks_album, track.cloned()) {
            r.top[0] = t;
        }
    }
    // ── stream: search-pop ── a hit song that answers the typed text better than the Top card
    // takes its place: "blinding" → Blinding Lights (3.7B plays) over a new album "BLINDED" by an
    // unknown act that YouTube put on top. Only over album, playlist and song cards (episodes and
    // shows are the podcast rules' business), and only for a hit ([`TITLE_TRACK_HIT_PLAYS`]) whose
    // title is the query, holds its words, or starts with them.
    if let Some(top) = r.top.first().filter(|t| matches!(t.kind, "album" | "playlist" | "song") && !podcast_card(t)) {
        use crate::podcasts::search::{card_text_match, MATCH_PREFIX};
        // A song's title without its dressing ("Closer (feat. Halsey)" is titled "closer").
        let text = |c: &innertube::BrowseItem| {
            if c.kind == "song" {
                let mut bare = c.clone();
                bare.title = parse(&c.title, "", None).base;
                card_text_match(query, &bare)
            } else {
                card_text_match(query, c)
            }
        };
        let card_m = text(top);
        let hit = r.songs.iter().find(|c| {
            let m = text(c);
            !c.is_video
                && !podcast_card(c)
                && c.play_count.as_deref().map(play_count).unwrap_or(0.0) >= TITLE_TRACK_HIT_PLAYS
                && m >= MATCH_PREFIX
                && m > card_m
        });
        if let Some(h) = hit.cloned() {
            r.top[0] = h;
        }
    }
    r
}

/// A podcast show or episode card in YouTube's mixed search.
fn podcast_card(c: &innertube::BrowseItem) -> bool {
    c.id.starts_with("MPSP") || c.subtitle.as_deref().is_some_and(|s| s.starts_with("Episode") || s.starts_with("Podcast"))
}

/// Plays from which an album's title track counts as a hit (and takes the Top Result).
pub const TITLE_TRACK_HIT_PLAYS: f64 = 200e6;

// --- artist-page dedup ---------------------------------------------------------------------------

/// Parse "53M", "1.2B", "820K", "1,234" plays.
pub fn play_count(s: &str) -> f64 {
    let t = s.split_whitespace().next().unwrap_or("").replace(',', "");
    let (num, mul) = match t.chars().last() {
        Some('K' | 'k') => (&t[..t.len() - 1], 1e3),
        Some('M' | 'm') => (&t[..t.len() - 1], 1e6),
        Some('B' | 'b') => (&t[..t.len() - 1], 1e9),
        _ => (t.as_str(), 1.0),
    };
    num.parse::<f64>().map(|n| n * mul).unwrap_or(0.0)
}

/// Collapse duplicate versions (research 22 "Artist-page dedup"): rows with the same work key
/// become one, in the position of the first. The one kept: undressed title, then the audio track,
/// then the most plays, then YouTube's order. Live and remixes are different work keys and stay.
pub fn dedup_songs(items: &mut Vec<SongItem>) {
    let mut groups: HashMap<WorkKey, Vec<usize>> = HashMap::new();
    let mut order = Vec::new();
    for (i, s) in items.iter().enumerate() {
        let k = work_key_song(s);
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(i);
    }
    if order.len() == items.len() {
        return;
    }
    let pick = |idx: &Vec<usize>| -> usize {
        *idx.iter()
            .min_by(|&&a, &&b| {
                let key = |i: usize| {
                    let s = &items[i];
                    let p = parse(&s.title, &s.artists, s.album.as_deref());
                    let dressed = norm(&s.title) != p.base;
                    let plays = s.play_count.as_deref().map(play_count).unwrap_or(0.0);
                    (dressed, s.is_video, -plays, i)
                };
                let (ka, kb) = (key(a), key(b));
                ka.0.cmp(&kb.0)
                    .then(ka.1.cmp(&kb.1))
                    .then(ka.2.total_cmp(&kb.2))
                    .then(ka.3.cmp(&kb.3))
            })
            .unwrap()
    };
    let keep: Vec<usize> = order.iter().map(|k| pick(&groups[k])).collect();
    let old = std::mem::take(items);
    let mut slots: Vec<Option<SongItem>> = old.into_iter().map(Some).collect();
    *items = keep.into_iter().filter_map(|i| slots[i].take()).collect();
}

#[cfg(test)]
#[path = "identity_tests.rs"]
pub mod tests;

// ── stream: bench ── the labelled Spotify → YouTube Music matcher benchmark.
#[cfg(test)]
#[path = "identity_bench.rs"]
pub mod bench;

// ── stream: bench2 ── the labelled search-ranking benchmark (Spotify's answers as the reference).
#[cfg(test)]
#[path = "search_bench.rs"]
pub mod search_bench;
