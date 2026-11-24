//! SponsorBlock for YouTube episodes: skip host-read sponsor spots, self-promotion and
//! "like and subscribe" segments.
//!
//! Privacy-preserving lookup (the API's recommended form): only the first 4 hex characters of
//! `sha256(videoId)` leave the machine; the server answers every video under that prefix and the
//! match happens here. Answers (including "no segments") are cached for a day.
//! Data: sponsor.ajay.app, CC BY-NC-SA 4.0 — credited in the UI/about box per its licence.
//!
//! Settings: `podcast_sponsorblock` ("false" turns it off; on by default) and
//! `podcast_sponsorblock_categories` (JSON array; default sponsor, selfpromo, interaction).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::store;

pub const DEFAULT_CATEGORIES: [&str; 3] = ["sponsor", "selfpromo", "interaction"];
const CACHE_TTL: i64 = 24 * 3600;
/// Segments shorter than this aren't worth a seek (and are usually mis-submissions).
const MIN_SEGMENT_SECS: f64 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub category: String,
}

pub fn hash_prefix(video_id: &str) -> String {
    let d = Sha256::digest(video_id.as_bytes());
    d.iter().take(2).map(|b| format!("{b:02x}")).collect()
}

/// The answer for a hash prefix → this video's skippable segments, sorted, in `categories`.
pub fn parse(v: &Value, video_id: &str, categories: &[String]) -> Vec<Segment> {
    let mut out: Vec<Segment> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e.get("videoID").and_then(Value::as_str) == Some(video_id))
        .flat_map(|e| e.get("segments").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|s| {
            let action = s.get("actionType").and_then(Value::as_str).unwrap_or("skip");
            if action != "skip" {
                return None;
            }
            let category = s.get("category")?.as_str()?.to_owned();
            if !categories.iter().any(|c| *c == category) {
                return None;
            }
            let seg = s.get("segment")?.as_array()?;
            let (start, end) = (seg.first()?.as_f64()?, seg.get(1)?.as_f64()?);
            (end - start >= MIN_SEGMENT_SECS).then_some(Segment { start, end, category })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The segment to skip at `pos`: one that `pos` is inside (not in its last half second, so a
/// seek that lands exactly on `end` doesn't trigger again) and that hasn't been skipped this play.
pub fn skip_at(segments: &[Segment], skipped: &[bool], pos: f64) -> Option<usize> {
    segments.iter().enumerate().position(|(i, s)| {
        !skipped.get(i).copied().unwrap_or(false) && pos >= s.start && pos < s.end - 0.5
    })
}

pub fn enabled(db: &crate::db::Db) -> bool {
    db.get_setting("podcast_sponsorblock").as_deref() != Some("false")
}

pub fn categories(db: &crate::db::Db) -> Vec<String> {
    db.get_setting("podcast_sponsorblock_categories")
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_else(|| DEFAULT_CATEGORIES.iter().map(|s| s.to_string()).collect())
}

/// This video's segments in the user's categories, from the cache or the API. Errors are an
/// empty list: SponsorBlock is a nicety and never blocks playback.
pub async fn segments(db: &crate::db::Db, video_id: &str) -> Vec<Segment> {
    parse(&raw(db, video_id).await, video_id, &categories(db))
}

/// stream: chapters. This video's community chapters (`chapter` segments, titled by their
/// `description`), for when it has no chapters of its own.
pub async fn chapters(db: &crate::db::Db, video_id: &str) -> Vec<super::chapters::Chapter> {
    parse_chapters(&raw(db, video_id).await, video_id)
}

pub fn parse_chapters(v: &Value, video_id: &str) -> Vec<super::chapters::Chapter> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|e| e.get("videoID").and_then(Value::as_str) == Some(video_id))
        .flat_map(|e| e.get("segments").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter(|s| s.get("actionType").and_then(Value::as_str) == Some("chapter"))
        .filter_map(|s| {
            Some(super::chapters::Chapter {
                start_secs: s.pointer("/segment/0")?.as_f64()?,
                title: s.get("description")?.as_str()?.to_owned(),
                ..Default::default()
            })
        })
        .collect()
}

/// This video's entry in the API's answer, every category, from the cache or the API (`[]` on any
/// error, uncached).
async fn raw(db: &crate::db::Db, video_id: &str) -> Value {
    let cached = store::sponsorblock_get(&db.podcasts_conn(), video_id, CACHE_TTL);
    let raw = match cached {
        Some(raw) => raw,
        None => {
            let all_cats = serde_json::to_string(&["sponsor", "selfpromo", "interaction", "intro", "outro", "preview", "music_offtopic", "filler", "chapter"]).unwrap_or_default();
            let url = format!(
                "https://sponsor.ajay.app/api/skipSegments/{}?categories={}&actionTypes={}",
                hash_prefix(video_id),
                urlencoding::encode(&all_cats),
                urlencoding::encode(r#"["skip","mute","chapter"]"#)
            );
            let resp = crate::http::client()
                .get(&url)
                .header(reqwest::header::USER_AGENT, "Spotless/1.0")
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            let body = match resp {
                // 404 = nothing under this prefix.
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => "[]".to_owned(),
                Ok(r) if r.status().is_success() => r.text().await.unwrap_or_else(|_| "[]".into()),
                Ok(r) => {
                    tracing::debug!("sponsorblock: HTTP {}", r.status());
                    return Value::Array(vec![]);
                }
                Err(e) => {
                    tracing::debug!("sponsorblock: {e}");
                    return Value::Array(vec![]);
                }
            };
            // Cache only this video's entry, all categories, so a category change needs no refetch.
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Array(vec![]));
            let mine: Vec<Value> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter(|e| e.get("videoID").and_then(Value::as_str) == Some(video_id))
                .cloned()
                .collect();
            let raw = serde_json::to_string(&mine).unwrap_or_else(|_| "[]".into());
            store::sponsorblock_put(&db.podcasts_conn(), video_id, &raw);
            raw
        }
    };
    serde_json::from_str(&raw).unwrap_or(Value::Array(vec![]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_four_hex_chars_of_sha256() {
        // sha256("dQw4w9WgXcQ") starts with "f5d7" … check length/format and stability.
        let p = hash_prefix("dQw4w9WgXcQ");
        assert_eq!(p.len(), 4);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(p, hash_prefix("dQw4w9WgXcQ"));
    }

    #[test]
    fn parses_only_this_video_and_wanted_categories() {
        let v = serde_json::json!([
            {"videoID": "other", "segments": [{"segment": [1.0, 50.0], "category": "sponsor", "actionType": "skip"}]},
            {"videoID": "vid", "segments": [
                {"segment": [300.0, 360.5], "category": "sponsor", "actionType": "skip"},
                {"segment": [10.0, 40.0], "category": "selfpromo", "actionType": "skip"},
                {"segment": [500.0, 520.0], "category": "intro", "actionType": "skip"},
                {"segment": [600.0, 600.4], "category": "sponsor", "actionType": "skip"},
                {"segment": [700.0, 760.0], "category": "sponsor", "actionType": "mute"}
            ]}
        ]);
        let cats: Vec<String> = DEFAULT_CATEGORIES.iter().map(|s| s.to_string()).collect();
        let s = parse(&v, "vid", &cats);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].category, "selfpromo");
        assert_eq!(s[1].end, 360.5);
    }

    #[test]
    fn chapter_segments_are_chapters_not_skips() {
        let v = serde_json::json!([{"videoID": "vid", "segments": [
            {"segment": [0.0, 300.0], "category": "chapter", "actionType": "chapter", "description": "Intro"},
            {"segment": [300.0, 900.0], "category": "chapter", "actionType": "chapter", "description": "Main topic"},
            {"segment": [400.0, 460.0], "category": "sponsor", "actionType": "skip"}
        ]}]);
        let c = parse_chapters(&v, "vid");
        assert_eq!(c.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Intro", "Main topic"]);
        assert_eq!(c[1].start_secs, 300.0);
        let cats: Vec<String> = vec!["chapter".into(), "sponsor".into()];
        assert_eq!(parse(&v, "vid", &cats).len(), 1, "a chapter is never skipped");
    }

    #[test]
    fn skips_each_segment_once() {
        let segs = vec![
            Segment { start: 10.0, end: 40.0, category: "sponsor".into() },
            Segment { start: 100.0, end: 130.0, category: "sponsor".into() },
        ];
        let mut skipped = vec![false, false];
        assert_eq!(skip_at(&segs, &skipped, 5.0), None);
        assert_eq!(skip_at(&segs, &skipped, 10.2), Some(0));
        skipped[0] = true;
        assert_eq!(skip_at(&segs, &skipped, 20.0), None, "rewinding into it plays it");
        assert_eq!(skip_at(&segs, &skipped, 129.7), None, "at the very end: nothing to gain");
        assert_eq!(skip_at(&segs, &skipped, 129.0), Some(1));
    }
}
