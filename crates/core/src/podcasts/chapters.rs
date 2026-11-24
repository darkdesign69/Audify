//! Podcast chapters (stream: chapters): the "Chapters" list on the episode page and in Now Playing,
//! the marks on the scrubber and the current chapter's title.
//!
//! Sources, first hit wins:
//! - an RSS episode:
//!   1. the feed's Podcasting 2.0 `<podcast:chapters>` JSON file ([`parse_json`]);
//!   2. ID3 `CHAP` frames in the enclosure's header, read with two small Range requests at most
//!      ([`parse_id3`], the `id3` crate);
//!   3. when it streams from its YouTube twin (`polish::podmatch`), the YouTube sources below;
//!   4. timestamps in the show notes ([`parse_description`]).
//! - a YouTube episode:
//!   1. YouTube's own chapter markers, from the watch page's `next` ([`parse_youtube`]);
//!   2. timestamps in the video description, the lines YouTube itself derives chapters from;
//!   3. SponsorBlock's `chapter` segments ([`super::sponsorblock::parse_chapters`]).
//!
//! Every list goes through [`clean`]: sorted, one chapter per start, nothing past the end, and
//! fewer than two chapters is no chapters. A result is cached per episode for a week (a miss for a
//! day: creators and SponsorBlock add chapters later). Nothing here is on the playback path: the
//! UI asks with `podcast_chapters` while the episode plays.

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{format_duration, sanitize, store};
use crate::state::AppState;

/// One chapter. `start_label` is the start as every row prints it ("1:02:05").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Chapter {
    pub start_secs: f64,
    /// The next chapter's start, or the episode's end. `None` for the last chapter of an episode
    /// whose length is unknown.
    pub end_secs: Option<f64>,
    pub title: String,
    pub start_label: String,
    /// The chapter's length as the chapters sheet prints it ("2min", "1hr 5min"); empty when the
    /// end is unknown.
    pub duration_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub img: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Where an episode's chapters came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Podcasting 2.0 JSON chapters linked from the feed.
    Feed,
    /// ID3 CHAP frames in the MP3.
    Id3,
    /// YouTube's chapter markers.
    Youtube,
    /// Timestamps in the description or show notes.
    Description,
    /// SponsorBlock's community chapters.
    Sponsorblock,
}

/// An episode's chapters. Empty (and `source: None`) when it has none.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Chapters {
    pub source: Option<Source>,
    pub chapters: Vec<Chapter>,
    /// YouTube made these chapters itself (its `AUTO_CHAPTERS`), not the creator: the sheet says
    /// "These chapters are auto-generated." (Spotify).
    pub auto_generated: bool,
}

const FOUND_TTL: i64 = 7 * 86_400;
const MISS_TTL: i64 = 86_400;
const UA: &str = "Spotless/1.0 (podcast player; +https://github.com/fjenn/spotless)";
/// A chapters JSON file is a few KB; this only stops a hostile URL.
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
/// Covers an ID3 tag with its artwork in one request most of the time.
const ID3_FIRST_READ: usize = 256 * 1024;
/// A tag bigger than this (huge embedded art) isn't worth downloading for its chapters.
const MAX_ID3_BYTES: usize = 8 * 1024 * 1024;

fn chapter(start_secs: f64, title: &str) -> Chapter {
    Chapter { start_secs, title: title.to_owned(), ..Default::default() }
}

/// The house rules for any source's list: drop untitled and out-of-range chapters, sort, keep the
/// first of chapters starting within a second of each other, fill in ends and labels, and give up
/// below two chapters (a single "chapter" is the episode).
pub fn clean(mut list: Vec<Chapter>, duration: Option<f64>) -> Vec<Chapter> {
    let duration = duration.filter(|d| *d > 0.0);
    for c in list.iter_mut() {
        c.title = c.title.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    list.retain(|c| {
        c.start_secs.is_finite()
            && c.start_secs >= 0.0
            && !c.title.is_empty()
            && !duration.is_some_and(|d| c.start_secs >= d - 1.0)
    });
    list.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    list.dedup_by(|b, a| b.start_secs - a.start_secs < 1.0);
    if list.len() < 2 {
        return Vec::new();
    }
    let starts: Vec<f64> = list.iter().map(|c| c.start_secs).collect();
    for (i, c) in list.iter_mut().enumerate() {
        c.end_secs = starts.get(i + 1).copied().or(duration);
        c.start_label = format_duration(c.start_secs as i64);
        c.duration_label = c.end_secs.map(|e| length_label(e - c.start_secs)).unwrap_or_default();
    }
    list
}

/// A chapter's length in whole minutes, at least one: "2min", "1hr", "1hr 5min" (Spotify's sheet).
pub fn length_label(secs: f64) -> String {
    let mins = ((secs.max(0.0) / 60.0).round() as i64).max(1);
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m}min"),
        (h, 0) => format!("{h}hr"),
        (h, m) => format!("{h}hr {m}min"),
    }
}

/// A Podcasting 2.0 chapters file (`application/json+chapters`). Chapters marked `"toc": false`
/// are silent markers, not table-of-contents entries, so they are left out.
pub fn parse_json(body: &str) -> Vec<Chapter> {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let s = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty());
    v.get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("toc").and_then(Value::as_bool) != Some(false))
        .filter_map(|c| {
            let start = c.get("startTime").and_then(Value::as_f64)?;
            Some(Chapter { img: s(c, "img"), url: s(c, "url"), ..chapter(start, &s(c, "title")?) })
        })
        .collect()
}

/// ID3v2 `CHAP` frames (title from each chapter's `TIT2`). Their order is the tag's, which `clean`
/// sorts, so the `CTOC` frame that lists them adds nothing. A truncated tag still yields the
/// frames read before the cut.
pub fn parse_id3(bytes: &[u8]) -> Vec<Chapter> {
    let Ok(tag) = id3::partial_tag_ok(id3::Tag::read_from2(std::io::Cursor::new(bytes))) else {
        return Vec::new();
    };
    tag.chapters()
        .map(|c| {
            let title = c.frames.iter().find(|f| f.id() == "TIT2").and_then(|f| f.content().text());
            chapter(c.start_time as f64 / 1000.0, title.unwrap_or_default())
        })
        .collect()
}

/// The size of the ID3v2 tag that `head` starts with, header included; `None` when there is none.
pub fn id3_size(head: &[u8]) -> Option<usize> {
    if head.len() < 10 || &head[..3] != b"ID3" {
        return None;
    }
    let body = head[6..10].iter().fold(0usize, |n, b| (n << 7) | (*b as usize & 0x7f));
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + body + footer)
}

/// YouTube's chapter markers in a WEB `next` response: the creator's (`DESCRIPTION_CHAPTERS`)
/// before YouTube's automatic ones, else the chapters panel (`macroMarkersListItemRenderer`).
pub fn parse_youtube(v: &Value) -> Vec<Chapter> {
    parse_youtube_markers(v).0
}

/// [`parse_youtube`], and whether the list is YouTube's automatic one (`AUTO_CHAPTERS`).
pub fn parse_youtube_markers(v: &Value) -> (Vec<Chapter>, bool) {
    let mut maps = Vec::new();
    find_all(v, "markersMap", &mut maps);
    let markers: Vec<&Value> = maps.iter().filter_map(|m| m.as_array()).flatten().collect();
    for key in ["DESCRIPTION_CHAPTERS", "AUTO_CHAPTERS"] {
        let list: Vec<Chapter> = markers
            .iter()
            .filter(|m| m.get("key").and_then(Value::as_str) == Some(key))
            .filter_map(|m| m.pointer("/value/chapters")?.as_array())
            .flatten()
            .filter_map(|c| {
                let r = c.get("chapterRenderer")?;
                let start = r.get("timeRangeStartMillis")?.as_f64()? / 1000.0;
                let img = r.pointer("/thumbnail/thumbnails")?.as_array()?.last()?.get("url")?.as_str();
                Some(Chapter { img: img.map(str::to_owned), ..chapter(start, r.pointer("/title/simpleText")?.as_str()?) })
            })
            .collect();
        if !list.is_empty() {
            return (list, key == "AUTO_CHAPTERS");
        }
    }
    let mut items = Vec::new();
    find_all(v, "macroMarkersListItemRenderer", &mut items);
    let list = items
        .iter()
        .filter_map(|r| {
            let start = sanitize::parse_clock(r.pointer("/timeDescription/simpleText")?.as_str()?)?;
            Some(chapter(start as f64, r.pointer("/title/simpleText")?.as_str()?))
        })
        .collect();
    (list, false)
}

/// The video description in a WEB `next` response.
pub fn youtube_description(v: &Value) -> Option<String> {
    let mut found = Vec::new();
    find_all(v, "attributedDescription", &mut found);
    find_all(v, "attributedDescriptionBodyText", &mut found);
    found.iter().find_map(|d| d.get("content")?.as_str().map(str::to_owned))
}

fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, child) in m {
                if k == key {
                    out.push(child);
                }
                find_all(child, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| find_all(c, key, out)),
        _ => {}
    }
}

/// Timestamped lines in a description or show notes, the way YouTube reads them: a line that
/// starts with a timestamp ("0:00 Intro", "(12:05) Guest", "[1:02:03] - Q&A") or ends with one
/// ("Intro - 0:00"). A timestamp in the middle of a sentence is not a chapter.
pub fn parse_description(text: &str) -> Vec<Chapter> {
    static LEAD: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();
    const TS: &str = r"((?:\d{1,2}:)?\d{1,2}:\d{2})";
    let lead = LEAD.get_or_init(|| {
        Regex::new(&format!(r"^[\s•·*\-–—>]*[(\[]?{TS}[)\]]?\s*[-–—:|.]?\s*(.+?)\s*$")).unwrap()
    });
    let tail = TAIL.get_or_init(|| Regex::new(&format!(r"^[\s•·*\-–—>]*(.+?)\s*[-–—:|]?\s+[(\[]?{TS}[)\]]?\s*$")).unwrap());
    text.lines()
        .filter_map(|line| {
            let (ts, title) = if let Some(c) = lead.captures(line) {
                (c.get(1)?.as_str(), c.get(2)?.as_str())
            } else {
                let c = tail.captures(line)?;
                (c.get(2)?.as_str(), c.get(1)?.as_str())
            };
            Some(chapter(sanitize::parse_clock(ts)? as f64, title))
        })
        .collect()
}

// --- fetching ------------------------------------------------------------------------------------

/// GET up to `limit` bytes (a Range request; a host that ignores Range is cut off at the limit).
async fn get_bytes(url: &str, limit: usize, range: bool) -> Option<Vec<u8>> {
    let mut req = crate::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .timeout(Duration::from_secs(15));
    if range {
        req = req.header(reqwest::header::RANGE, format!("bytes=0-{}", limit - 1));
    }
    let mut resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        tracing::debug!("chapters: HTTP {} for {url}", resp.status());
        return None;
    }
    let mut out = Vec::new();
    while out.len() < limit {
        match resp.chunk().await {
            Ok(Some(c)) => out.extend_from_slice(&c),
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    out.truncate(limit);
    Some(out)
}

pub(crate) async fn from_feed(url: &str) -> Vec<Chapter> {
    let Some(bytes) = get_bytes(url, MAX_JSON_BYTES, false).await else { return Vec::new() };
    parse_json(&String::from_utf8_lossy(&bytes))
}

pub(crate) async fn from_id3(enclosure: &str) -> Vec<Chapter> {
    let Some(head) = get_bytes(enclosure, ID3_FIRST_READ, true).await else { return Vec::new() };
    let Some(size) = id3_size(&head) else { return Vec::new() };
    if size <= head.len() {
        return parse_id3(&head[..size]);
    }
    if size > MAX_ID3_BYTES {
        return Vec::new();
    }
    match get_bytes(enclosure, size, true).await {
        Some(tag) => parse_id3(&tag),
        None => Vec::new(),
    }
}

/// YouTube markers, then description timestamps, then SponsorBlock, for one video.
async fn from_youtube(st: &AppState, video_id: &str, duration: Option<f64>) -> Chapters {
    if let Some(web) = st.clients.get("WEB") {
        match st.it.podcast_watch_json(web, video_id).await {
            Ok(v) => {
                let (list, auto_generated) = parse_youtube_markers(&v);
                let found = clean(list, duration);
                if !found.is_empty() {
                    return Chapters { source: Some(Source::Youtube), chapters: found, auto_generated };
                }
                let found = clean(parse_description(&youtube_description(&v).unwrap_or_default()), duration);
                if !found.is_empty() {
                    return Chapters { source: Some(Source::Description), chapters: found, auto_generated: false };
                }
            }
            Err(e) => tracing::debug!("chapters: watch page for {video_id}: {e}"),
        }
    }
    let found = clean(super::sponsorblock::chapters(&st.db, video_id).await, duration);
    let source = (!found.is_empty()).then_some(Source::Sponsorblock);
    Chapters { source, chapters: found, auto_generated: false }
}

async fn fetch(st: &AppState, id: &str, duration: Option<f64>) -> Chapters {
    let ep = store::get_episode(&st.db.podcasts_conn(), id);
    let found = |source, list| {
        let chapters = clean(list, duration);
        (!chapters.is_empty()).then_some(Chapters { source: Some(source), chapters, auto_generated: false })
    };
    let Some(enclosure) = super::rss_url(id) else {
        return from_youtube(st, id, duration).await;
    };
    let url = store::chapters_url(&st.db.podcasts_conn(), id);
    if let Some(c) = match url {
        Some(u) => found(Source::Feed, from_feed(&u).await),
        None => None,
    } {
        return c;
    }
    if let Some(c) = found(Source::Id3, from_id3(enclosure).await) {
        return c;
    }
    let twin = crate::polish::podmatch::enabled(&st.db)
        .then(|| crate::polish::podmatch::known_twin(&st.db, id))
        .flatten();
    if let Some(vid) = twin {
        let c = from_youtube(st, &vid, duration).await;
        if c.source.is_some() {
            return c;
        }
    }
    let notes = ep.and_then(|e| e.description).map(|d| sanitize::to_text(&d)).unwrap_or_default();
    found(Source::Description, parse_description(&notes)).unwrap_or_default()
}

/// A cached answer still fresh: a week for chapters found, a day for none.
fn cached(conn: &rusqlite::Connection, key: &str) -> Option<Chapters> {
    let c: Chapters = serde_json::from_str(&store::cache_get(conn, key, FOUND_TTL)?).ok()?;
    (c.source.is_some() || store::cache_get(conn, key, MISS_TTL).is_some()).then_some(c)
}

/// An episode's chapters, from the cache or its sources. `duration_secs` is the player's length
/// when it knows it better than the stored row. Never fails: no chapters is an empty list.
pub async fn chapters(st: &AppState, id: &str, duration_secs: Option<f64>) -> Chapters {
    let duration = duration_secs
        .filter(|d| *d > 0.0)
        .or_else(|| store::get_episode(&st.db.podcasts_conn(), id)?.duration_secs.map(|d| d as f64));
    let key = format!("chapters:{id}");
    if let Some(c) = cached(&st.db.podcasts_conn(), &key) {
        let chapters = clean(c.chapters, duration);
        let source = c.source.filter(|_| !chapters.is_empty());
        let auto_generated = c.auto_generated && source.is_some();
        return Chapters { source, chapters, auto_generated };
    }
    let c = fetch(st, id, duration).await;
    store::cache_put(&st.db.podcasts_conn(), &key, &serde_json::to_string(&c).unwrap_or_default());
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/src/podcasts/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    fn titles(c: &[Chapter]) -> Vec<&str> {
        c.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn json_chapters_drop_silent_markers_duplicates_and_overruns() {
        let raw = parse_json(&fixture("chapters.json"));
        assert_eq!(raw.len(), 6, "the toc:false marker is not a chapter");
        let c = clean(raw, Some(2400.0));
        assert_eq!(titles(&c), ["Cold open", "Why chapters", "Listener mail", "Wrap-up"]);
        assert_eq!(c[1].img.as_deref(), Some("https://example.com/ch2.jpg"));
        assert_eq!(c[1].url.as_deref(), Some("https://example.com/link"));
        assert_eq!(c[1].start_label, "1:35");
        assert_eq!(c[1].end_secs, Some(600.0));
        assert_eq!(c[3].end_secs, Some(2400.0), "the last chapter runs to the end");
        assert!(parse_json("not json").is_empty());
    }

    #[test]
    fn clean_needs_two_chapters_and_handles_unknown_length() {
        assert!(clean(vec![chapter(0.0, "Only")], None).is_empty());
        assert!(clean(vec![chapter(0.0, "A"), chapter(0.4, "B")], None).is_empty(), "same start");
        assert!(clean(vec![chapter(0.0, "A"), chapter(500.0, "B")], Some(400.0)).is_empty(), "B is past the end");
        let c = clean(vec![chapter(3725.0, "B"), chapter(0.0, " A ")], None);
        assert_eq!(titles(&c), ["A", "B"]);
        assert_eq!(c[1].start_label, "1:02:05");
        assert_eq!(c[1].end_secs, None);
        assert_eq!(c[0].duration_label, "1hr 2min");
        assert_eq!(c[1].duration_label, "", "no end, no length");
        assert!(clean(vec![chapter(0.0, "A"), chapter(f64::NAN, "B"), chapter(9.0, "")], None).is_empty());
    }

    #[test]
    fn length_labels() {
        assert_eq!(length_label(10.0), "1min", "never zero");
        assert_eq!(length_label(119.0), "2min");
        assert_eq!(length_label(3600.0), "1hr");
        assert_eq!(length_label(3900.0), "1hr 5min");
    }

    #[test]
    fn youtube_auto_chapters_are_flagged() {
        let v: Value = serde_json::from_str(&fixture("yt_watch_chapters.json")).unwrap();
        assert!(!parse_youtube_markers(&v).1, "the creator's own markers");
        let auto = serde_json::json!({"markersMap": [{"key": "AUTO_CHAPTERS", "value": {"chapters": [
            {"chapterRenderer": {"timeRangeStartMillis": 0, "title": {"simpleText": "Intro"},
                "thumbnail": {"thumbnails": [{"url": "https://i.ytimg.com/a.jpg"}]}}},
            {"chapterRenderer": {"timeRangeStartMillis": 90000, "title": {"simpleText": "Main"},
                "thumbnail": {"thumbnails": [{"url": "https://i.ytimg.com/b.jpg"}]}}}]}}]});
        let (list, generated) = parse_youtube_markers(&auto);
        assert!(generated);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].img.as_deref(), Some("https://i.ytimg.com/b.jpg"));
    }

    #[test]
    fn id3_chap_frames() {
        use id3::frame::{Chapter as Chap, Content, Frame, TableOfContents};
        use id3::TagLike;
        let mut tag = id3::Tag::new();
        tag.set_title("Episode");
        for (id, start, end, title) in [("ch1", 60_000, 120_000, "Second"), ("ch0", 0, 60_000, "First"), ("ch2", 120_000, 180_000, "Third")] {
            tag.add_frame(Chap {
                element_id: id.into(),
                start_time: start,
                end_time: end,
                start_offset: u32::MAX,
                end_offset: u32::MAX,
                frames: vec![Frame::with_content("TIT2", Content::Text(title.into()))],
            });
        }
        tag.add_frame(TableOfContents {
            element_id: "toc".into(),
            top_level: true,
            ordered: true,
            elements: vec!["ch0".into(), "ch1".into(), "ch2".into()],
            frames: vec![],
        });
        let mut bytes = Vec::new();
        tag.write_to(&mut bytes, id3::Version::Id3v24).unwrap();
        bytes.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00]); // the first MPEG frame header
        assert_eq!(id3_size(&bytes), Some(bytes.len() - 4));
        let c = clean(parse_id3(&bytes), Some(200.0));
        assert_eq!(titles(&c), ["First", "Second", "Third"]);
        assert_eq!(c[1].start_secs, 60.0);
        assert!(parse_id3(b"\xff\xfb\x90\x00 no tag").is_empty());
        assert_eq!(id3_size(b"\xff\xfb\x90\x00 no tag"), None);
    }

    #[test]
    fn youtube_markers_and_description() {
        let v: Value = serde_json::from_str(&fixture("yt_watch_chapters.json")).unwrap();
        let c = clean(parse_youtube(&v), Some(10_800.0));
        assert_eq!(c.len(), 12);
        assert_eq!(c[0].title, "Introduction");
        assert_eq!(c[2].start_secs, 879.0);
        assert_eq!(c[2].start_label, "14:39");
        assert!(c[0].img.as_deref().unwrap().starts_with("https://i.ytimg.com/"));
        // The description alone gives the same chapters (it is what YouTube read them from).
        let d = clean(parse_description(&youtube_description(&v).unwrap()), Some(10_800.0));
        assert_eq!(titles(&d), titles(&c));
        assert_eq!(d.iter().map(|c| c.start_secs).collect::<Vec<_>>(), c.iter().map(|c| c.start_secs).collect::<Vec<_>>());
    }

    #[test]
    fn description_lines() {
        let text = "Great talk with a guest. At 12:30 he says something.\n\
            (00:00) Intro\n\
            [05:10] - Guest's story\n\
            • 1:02:03 | Q&A: listeners\n\
            Wrap-up — 1:30:00\n\
            https://example.com/12:34\n";
        let c = clean(parse_description(text), None);
        assert_eq!(titles(&c), ["Intro", "Guest's story", "Q&A: listeners", "Wrap-up"]);
        assert_eq!(c[1].start_secs, 310.0);
        assert_eq!(c[3].start_secs, 5400.0);
        assert!(clean(parse_description("no timestamps here"), None).is_empty());
    }
}
