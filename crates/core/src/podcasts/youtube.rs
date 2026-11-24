//! YouTube Music podcast JSON → [`Show`] / [`Episode`]. Pure parsers over the raw responses from
//! `innertube::podcasts`, tested against trimmed live fixtures (`fixtures/yt_*.json`).
//!
//! Shapes (2026-11, WEB_REMIX, hl=en):
//! - search, Podcasts filter: `musicResponsiveListItemRenderer` rows whose `navigationEndpoint`
//!   browses an `MPSP…` show; columns are title / author.
//! - search, Episodes filter: the same rows with a `MUSIC_VIDEO_TYPE_PODCAST_EPISODE` watch
//!   endpoint; second column is "5d ago • Show" with the show's `MPSP…` link.
//! - show page: `musicResponsiveHeaderRenderer` (title, `straplineTextOne` author, description
//!   shelf) + a `musicShelfRenderer` of `musicMultiRowListItemRenderer` episodes, paged by
//!   `nextContinuationData`; a chip menu carries "Latest" / "Oldest" reload tokens.
//! - episode page (`MPED<videoId>`): the same header shape plus the full description runs.

use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

use super::{sanitize, Episode, Show, ShowSource};

pub const EPISODE_TYPE: &str = "MUSIC_VIDEO_TYPE_PODCAST_EPISODE";

// --- JSON helpers --------------------------------------------------------------------------------

fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                }
                find_all(x, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find_all(x, key, out)),
        _ => {}
    }
}

fn all<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    find_all(v, key, &mut out);
    out
}

fn first<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(key) {
                return Some(x);
            }
            m.values().find_map(|x| first(x, key))
        }
        Value::Array(a) => a.iter().find_map(|x| first(x, key)),
        _ => None,
    }
}

fn first_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    first(v, key).and_then(Value::as_str)
}

fn runs(v: Option<&Value>) -> Vec<Value> {
    v.and_then(|t| t.get("runs")).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn runs_text(v: Option<&Value>) -> String {
    let r = runs(v);
    if r.is_empty() {
        return v
            .and_then(|t| t.get("simpleText"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
    }
    r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect()
}

fn last_thumb(v: &Value) -> Option<String> {
    first(v, "thumbnails")?
        .as_array()?
        .last()?
        .get("url")?
        .as_str()
        .map(str::to_owned)
}

fn column_runs(row: &Value, i: usize) -> Option<&Value> {
    row.get("flexColumns")?
        .get(i)?
        .get("musicResponsiveListItemFlexColumnRenderer")?
        .get("text")
}

/// "78K views • 2d ago" → "2d ago": the run that parses as a date.
fn date_from_runs(text: &str, now: i64) -> Option<i64> {
    text.split('•').map(str::trim).find_map(|p| parse_date(p, now))
}

// --- dates and durations -------------------------------------------------------------------------

/// YouTube's episode dates (English): "5d ago", "3 hours ago", "Yesterday", "Oct 1" (this year,
/// or last year when that would be in the future), "May 22, 2025". Unix seconds at day precision.
pub fn parse_date(text: &str, now: i64) -> Option<i64> {
    static REL: OnceLock<Regex> = OnceLock::new();
    let t = text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    if t == "today" || t == "just now" {
        return Some(now);
    }
    if t == "yesterday" {
        return Some(now - 86_400);
    }
    let rel = REL.get_or_init(|| {
        Regex::new(r"^(\d+)\s*(seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h|days?|d|weeks?|wks?|w|months?|mos?|years?|yrs?|y)\s+ago$").unwrap()
    });
    if let Some(c) = rel.captures(&t) {
        let n: i64 = c[1].parse().ok()?;
        let unit = &c[2];
        let secs = match unit.chars().next()? {
            's' => 1,
            'h' => 3600,
            'd' => 86_400,
            'w' => 7 * 86_400,
            'y' => 365 * 86_400,
            'm' if unit.starts_with("mo") => 30 * 86_400,
            'm' => 60,
            _ => return None,
        };
        return Some(now - n * secs);
    }
    let today = Utc.timestamp_opt(now, 0).single()?.date_naive();
    let clean = text.trim().replace(',', "");
    if let Ok(d) = NaiveDate::parse_from_str(&clean, "%b %d %Y") {
        return Some(midday(d));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{clean} {}", today.year()), "%b %d %Y") {
        let d = if d > today + Duration::days(1) {
            NaiveDate::from_ymd_opt(d.year() - 1, d.month(), d.day())?
        } else {
            d
        };
        return Some(midday(d));
    }
    None
}

fn midday(d: NaiveDate) -> i64 {
    d.and_hms_opt(12, 0, 0).map(|t| t.and_utc().timestamp()).unwrap_or(0)
}

/// "1 hr 4 min" → 3840, "38 min" → 2280, "45 sec" → 45, "2 hr" → 7200, "1:02:03" → 3723.
pub fn parse_duration_text(text: &str) -> Option<i64> {
    let t = text.trim().trim_start_matches('•').trim();
    if t.contains(':') {
        return sanitize::parse_clock(t);
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(\d+)\s*(hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b").unwrap());
    let mut total = 0;
    let mut any = false;
    for c in re.captures_iter(&t.to_ascii_lowercase()) {
        let n: i64 = c[1].parse().ok()?;
        total += n * match c[2].chars().next()? {
            'h' => 3600,
            'm' => 60,
            _ => 1,
        };
        any = true;
    }
    any.then_some(total)
}

// --- search --------------------------------------------------------------------------------------

/// Show rows of a Podcasts-filter search.
pub fn parse_show_search(v: &Value) -> Vec<Show> {
    all(v, "musicResponsiveListItemRenderer")
        .into_iter()
        .filter_map(|row| {
            let id = row.pointer("/navigationEndpoint/browseEndpoint/browseId")?.as_str()?;
            if !id.starts_with("MPSP") {
                return None;
            }
            Some(Show {
                id: id.to_owned(),
                title: runs_text(column_runs(row, 0)),
                author: runs_text(column_runs(row, 1)),
                thumbnail: row.get("thumbnail").and_then(last_thumb),
                yt_browse_id: Some(id.to_owned()),
                source: ShowSource::Youtube,
                ..Default::default()
            })
        })
        .collect()
}

/// Episode rows of an Episodes-filter search.
pub fn parse_episode_search(v: &Value, now: i64) -> Vec<Episode> {
    all(v, "musicResponsiveListItemRenderer")
        .into_iter()
        .filter_map(|row| {
            let watch = first(row.get("overlay")?, "watchEndpoint")?;
            if first_str(watch, "musicVideoType") != Some(EPISODE_TYPE) {
                return None;
            }
            let video_id = watch.get("videoId")?.as_str()?.to_owned();
            let meta = column_runs(row, 1);
            let meta_runs = runs(meta);
            let show_run = meta_runs.iter().find(|r| {
                r.pointer("/navigationEndpoint/browseEndpoint/browseId")
                    .and_then(Value::as_str)
                    .is_some_and(|b| b.starts_with("MPSP"))
            });
            let show_id = show_run
                .and_then(|r| r.pointer("/navigationEndpoint/browseEndpoint/browseId"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let show_title = show_run
                .and_then(|r| r.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(Episode {
                id: video_id.clone(),
                video_id: Some(video_id),
                show_id,
                show_title,
                title: runs_text(column_runs(row, 0)),
                published_at: date_from_runs(&runs_text(meta), now),
                thumbnail: row.get("thumbnail").and_then(last_thumb),
                ..Default::default()
            })
        })
        .collect()
}

// --- show page -----------------------------------------------------------------------------------

/// A parsed show page or continuation.
#[derive(Debug, Default)]
pub struct ShowParse {
    pub show: Show,
    pub episodes: Vec<Episode>,
    pub continuation: Option<String>,
    /// The "Oldest" sort reload token (first page only).
    pub oldest_token: Option<String>,
}

/// One `musicMultiRowListItemRenderer` episode row.
fn parse_multi_row(row: &Value, show_id: Option<&str>, show_title: &str, now: i64) -> Option<Episode> {
    let watch = first(row.get("onTap").or_else(|| row.get("overlay"))?, "watchEndpoint")?;
    let video_id = watch.get("videoId")?.as_str()?.to_owned();
    let desc = runs_text(row.get("description"));
    let duration = row
        .pointer("/playbackProgress/musicPlaybackProgressRenderer/durationText")
        .map(|d| runs_text(Some(d)))
        .and_then(|d| parse_duration_text(&d));
    // A home-feed row names its show; a show-page row doesn't.
    let second = runs(row.get("secondTitle"));
    let (sid, stitle) = match second.first() {
        Some(r) => (
            r.pointer("/navigationEndpoint/browseEndpoint/browseId")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(show_id.map(str::to_owned)),
            r.get("text").and_then(Value::as_str).unwrap_or(show_title).to_owned(),
        ),
        None => (show_id.map(str::to_owned), show_title.to_owned()),
    };
    Some(Episode {
        id: video_id.clone(),
        video_id: Some(video_id),
        show_id: sid,
        show_title: stitle,
        title: runs_text(row.get("title")),
        published_at: date_from_runs(&runs_text(row.get("subtitle")), now),
        duration_secs: duration,
        description: (!desc.is_empty()).then(|| sanitize::notes_html(&desc)),
        thumbnail: row.get("thumbnail").and_then(last_thumb),
        ..Default::default()
    })
}

fn episodes_in(v: &Value, show_id: Option<&str>, show_title: &str, now: i64) -> Vec<Episode> {
    all(v, "musicMultiRowListItemRenderer")
        .into_iter()
        .filter_map(|r| parse_multi_row(r, show_id, show_title, now))
        .collect()
}

fn next_token(v: &Value) -> Option<String> {
    all(v, "nextContinuationData")
        .into_iter()
        .find_map(|c| c.get("continuation").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The reload token of the sort chip labelled `label` ("Oldest" / "Latest").
fn sort_token(v: &Value, label: &str) -> Option<String> {
    all(v, "menuNavigationItemRenderer").into_iter().find_map(|m| {
        if runs_text(m.get("text")) != label {
            return None;
        }
        let t = first(m, "reloadContinuationData")?.get("continuation")?.as_str()?;
        Some(urlencoding::decode(t).map(|c| c.into_owned()).unwrap_or_else(|_| t.to_owned()))
    })
}

/// A show page (`MPSP…`).
pub fn parse_show_page(browse_id: &str, v: &Value, now: i64) -> ShowParse {
    let header = first(v, "musicResponsiveHeaderRenderer");
    let mut show = Show::youtube(browse_id);
    if let Some(h) = header {
        show.title = runs_text(h.get("title"));
        show.author = runs_text(h.get("straplineTextOne"));
        show.thumbnail = h.get("thumbnail").and_then(last_thumb);
        let desc = first(h, "musicDescriptionShelfRenderer")
            .map(|d| runs_text(d.get("description")))
            .unwrap_or_default();
        show.description = (!desc.is_empty()).then_some(desc);
    }
    let episodes = episodes_in(v, Some(browse_id), &show.title, now);
    ShowParse {
        continuation: next_token(v),
        oldest_token: sort_token(v, "Oldest"),
        show,
        episodes,
    }
}

/// A continuation page (next page, or the "Oldest" reload) of a show's episodes.
pub fn parse_show_continuation(v: &Value, show_id: &str, show_title: &str, now: i64) -> (Vec<Episode>, Option<String>) {
    (episodes_in(v, Some(show_id), show_title, now), next_token(v))
}

/// An episode page (`MPED<videoId>`): the full show notes.
pub fn parse_episode_page(video_id: &str, v: &Value, now: i64) -> Option<Episode> {
    let h = first(v, "musicResponsiveHeaderRenderer")?;
    let strap = runs(h.get("straplineTextOne"));
    let show_id = strap
        .first()
        .and_then(|r| r.pointer("/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let desc_runs = first(v, "musicDescriptionShelfRenderer")
        .map(|d| runs(d.get("description")))
        .unwrap_or_default();
    let duration = first(h, "durationText").map(|d| runs_text(Some(d))).and_then(|d| parse_duration_text(&d));
    Some(Episode {
        id: video_id.to_owned(),
        video_id: Some(video_id.to_owned()),
        show_id,
        show_title: runs_text(h.get("straplineTextOne")),
        title: runs_text(h.get("title")),
        published_at: date_from_runs(&runs_text(h.get("subtitle")), now),
        duration_secs: duration,
        description: (!desc_runs.is_empty()).then(|| sanitize::runs_to_html(&desc_runs)),
        thumbnail: h.get("thumbnail").and_then(last_thumb),
        ..Default::default()
    })
}

// --- network -------------------------------------------------------------------------------------

use innertube::{InnerTube, YouTubeClient};

use super::Sort;

fn err(e: innertube::Error) -> String {
    e.to_string()
}

pub async fn search_shows(it: &InnerTube, client: &YouTubeClient, q: &str) -> Result<Vec<Show>, String> {
    let v = it.podcast_search_json(client, q, innertube::podcasts::FILTER_PODCASTS).await.map_err(err)?;
    Ok(parse_show_search(&v))
}

pub async fn search_episodes(it: &InnerTube, client: &YouTubeClient, q: &str) -> Result<Vec<Episode>, String> {
    let v = it.podcast_search_json(client, q, innertube::podcasts::FILTER_EPISODES).await.map_err(err)?;
    Ok(parse_episode_search(&v, super::now_secs()))
}

/// A show's header and first page of episodes in `sort` order.
pub async fn fetch_show(
    it: &InnerTube,
    client: &YouTubeClient,
    browse_id: &str,
    sort: Sort,
) -> Result<ShowParse, String> {
    let now = super::now_secs();
    let v = it.podcast_browse_json(client, browse_id).await.map_err(err)?;
    let mut p = parse_show_page(browse_id, &v, now);
    if p.show.title.is_empty() && p.episodes.is_empty() {
        return Err("This show isn't available.".into());
    }
    if sort == Sort::Oldest {
        if let Some(tok) = p.oldest_token.clone() {
            let v = it.podcast_continuation_json(client, &tok).await.map_err(err)?;
            let (eps, next) = parse_show_continuation(&v, browse_id, &p.show.title, now);
            p.episodes = eps;
            p.continuation = next;
        } else {
            // No sort menu (a short show): everything is on this page.
            p.episodes.reverse();
        }
    }
    Ok(p)
}

pub async fn fetch_more(
    it: &InnerTube,
    client: &YouTubeClient,
    token: &str,
    show_id: &str,
    show_title: &str,
) -> Result<(Vec<Episode>, Option<String>), String> {
    let v = it.podcast_continuation_json(client, token).await.map_err(err)?;
    Ok(parse_show_continuation(&v, show_id, show_title, super::now_secs()))
}

pub async fn fetch_episode(it: &InnerTube, client: &YouTubeClient, video_id: &str) -> Result<Episode, String> {
    let v = it.podcast_browse_json(client, &format!("MPED{video_id}")).await.map_err(err)?;
    parse_episode_page(video_id, &v, super::now_secs()).ok_or_else(|| "episode not found".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let p = format!("{}/src/podcasts/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    // 2026-11-15 12:00 UTC, the day the fixtures were captured.
    const NOW: i64 = 1_794_744_000;

    #[test]
    fn dates() {
        let now = NOW;
        assert_eq!(parse_date("5d ago", now), Some(now - 5 * 86_400));
        assert_eq!(parse_date("3 hours ago", now), Some(now - 3 * 3600));
        assert_eq!(parse_date("2 weeks ago", now), Some(now - 14 * 86_400));
        assert_eq!(parse_date("1 month ago", now), Some(now - 30 * 86_400));
        assert_eq!(parse_date("Yesterday", now), Some(now - 86_400));
        let may = parse_date("May 22, 2025", now).unwrap();
        assert_eq!(Utc.timestamp_opt(may, 0).unwrap().date_naive().to_string(), "2025-05-22");
        let oct = parse_date("Oct 1", now).unwrap();
        assert_eq!(Utc.timestamp_opt(oct, 0).unwrap().date_naive().to_string(), "2026-10-01");
        // A month later than today belongs to last year.
        let dec = parse_date("Dec 20", now).unwrap();
        assert_eq!(Utc.timestamp_opt(dec, 0).unwrap().date_naive().to_string(), "2025-12-20");
        assert_eq!(parse_date("78K views", now), None);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_text("38 min"), Some(2280));
        assert_eq!(parse_duration_text(" • 1 hr 4 min"), Some(3840));
        assert_eq!(parse_duration_text("2 hr"), Some(7200));
        assert_eq!(parse_duration_text("45 sec"), Some(45));
        assert_eq!(parse_duration_text("1:02:03"), Some(3723));
        assert_eq!(parse_duration_text("soon"), None);
    }

    #[test]
    fn show_search() {
        let shows = parse_show_search(&fixture("yt_search_podcasts.json"));
        assert!(shows.len() >= 3, "{shows:?}");
        let h = &shows[0];
        assert_eq!(h.title, "Huberman Lab");
        assert_eq!(h.author, "Andrew Huberman");
        assert_eq!(h.id, "MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW");
        assert!(h.thumbnail.as_deref().unwrap().starts_with("https://"));
    }

    #[test]
    fn episode_search() {
        let eps = parse_episode_search(&fixture("yt_search_episodes.json"), NOW);
        assert!(eps.len() >= 3);
        let e = &eps[0];
        assert_eq!(e.id, "wbJ_Q1xsDHg");
        assert_eq!(e.show_title, "Huberman Lab");
        assert_eq!(e.show_id.as_deref(), Some("MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW"));
        assert_eq!(e.published_at, Some(NOW - 5 * 86_400));
        assert!(e.title.contains("Anna Lembke"));
    }

    #[test]
    fn show_page() {
        let p = parse_show_page("MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW", &fixture("yt_show.json"), NOW);
        assert_eq!(p.show.title, "Huberman Lab");
        assert_eq!(p.show.author, "Andrew Huberman");
        assert!(p.show.description.as_deref().unwrap().contains("neuroscientist"));
        assert!(!p.episodes.is_empty());
        let e = &p.episodes[0];
        assert_eq!(e.id, "1rn_Clc-MM0");
        assert_eq!(e.duration_secs, Some(38 * 60));
        assert_eq!(e.published_at, Some(NOW - 2 * 86_400));
        assert_eq!(e.show_title, "Huberman Lab");
        assert!(e.description.is_some());
        assert!(p.continuation.is_some());
        let oldest = p.oldest_token.unwrap();
        assert!(!oldest.contains('%'), "token must be url-decoded: {oldest}");
    }

    #[test]
    fn continuations() {
        let (eps, next) = parse_show_continuation(&fixture("yt_show_cont.json"), "MPSPx", "Huberman Lab", NOW);
        assert!(!eps.is_empty());
        assert!(next.is_some());
        let (old, _) = parse_show_continuation(&fixture("yt_show_oldest.json"), "MPSPx", "Huberman Lab", NOW);
        assert_eq!(old[0].title, "Welcome to the Huberman Lab Podcast");
        let d = Utc.timestamp_opt(old[0].published_at.unwrap(), 0).unwrap().date_naive();
        assert_eq!(d.to_string(), "2020-12-23");
    }

    #[test]
    fn episode_page() {
        let e = parse_episode_page("1rn_Clc-MM0", &fixture("yt_episode.json"), NOW).unwrap();
        assert!(e.title.starts_with("Essentials: How Emotions"));
        assert_eq!(e.show_id.as_deref(), Some("MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW"));
        assert_eq!(e.show_title, "Huberman Lab");
        assert_eq!(e.duration_secs, Some(38 * 60));
        let d = e.description.unwrap();
        assert!(d.contains(r#"href="https://drinkag1.com/huberman""#), "{d}");
    }
}
