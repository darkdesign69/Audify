//! RSS / Atom / JSON Feed podcasts through `feed-rs` (MIT): one parser for every feed dialect,
//! with the iTunes extensions (author, artwork, duration) folded into its media model.
//!
//! Fetching follows AntennaPod / PinePods: a conditional GET (`If-None-Match` /
//! `If-Modified-Since`) so an unchanged feed costs a 304, a real User-Agent (some hosts refuse
//! reqwest's default), and a size cap so a hostile URL can't fill memory.

use std::time::Duration;

use super::{rss_id, sanitize, Episode, Show, ShowSource};

const UA: &str = "Spotless/1.0 (podcast player; +https://github.com/fjenn/spotless)";
/// Large back catalogues (e.g. 1,500-episode feeds with full notes) run to ~20 MB.
const MAX_FEED_BYTES: usize = 40 * 1024 * 1024;

/// Parse feed bytes into the show and its playable episodes (entries with an audio or video
/// enclosure). `feed_url` becomes the show id.
pub fn parse_feed(feed_url: &str, bytes: &[u8]) -> Result<(Show, Vec<Episode>), String> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(feed_url))
        .build()
        .parse(bytes)
        .map_err(|e| format!("not a podcast feed: {e}"))?;
    let thumbnail = feed
        .logo
        .as_ref()
        .map(|i| i.uri.clone())
        .or_else(|| feed.icon.as_ref().map(|i| i.uri.clone()));
    let title = feed.title.as_ref().map(|t| t.content.trim().to_owned()).unwrap_or_default();
    let author = feed
        .authors
        .first()
        .map(|p| p.name.trim().to_owned())
        .unwrap_or_default();
    let description = feed
        .description
        .as_ref()
        .map(|t| sanitize::to_text(&t.content))
        .filter(|s| !s.is_empty());
    let show = Show {
        id: feed_url.to_owned(),
        title: title.clone(),
        author,
        thumbnail: thumbnail.clone(),
        description,
        feed_url: Some(feed_url.to_owned()),
        source: ShowSource::Rss,
        serial: is_serial_feed(bytes),
        ..Default::default()
    };
    // feed-rs reads an `MM:SS` itunes:duration ("45:10") as 45 seconds, and that form is common,
    // so the raw values are read here and win when the item count lines up.
    let raw_durations = itunes_durations(bytes);
    let raw_durations = (raw_durations.len() == feed.entries.len()).then_some(raw_durations);
    // feed-rs doesn't read the Podcasting 2.0 namespace: the chapters links come from the raw items.
    let chapter_urls = item_chapter_urls(bytes);
    let chapter_urls = (chapter_urls.len() == feed.entries.len()).then_some(chapter_urls);
    let mut episodes = Vec::new();
    for (i, entry) in feed.entries.iter().enumerate() {
        // The enclosure: the first media content that is audio or video (or untyped).
        let mut enclosure: Option<(String, bool)> = None;
        let mut duration = None;
        let mut thumb = None;
        let mut media_desc = None;
        for m in &entry.media {
            duration = duration.or(m.duration.map(|d: Duration| d.as_secs() as i64));
            thumb = thumb.or_else(|| m.thumbnails.first().map(|t| t.image.uri.clone()));
            media_desc = media_desc.or_else(|| m.description.as_ref().map(|d| d.content.clone()));
            for c in &m.content {
                let Some(url) = &c.url else { continue };
                let ty = c.content_type.as_ref().map(|t| t.to_string()).unwrap_or_default();
                let playable = ty.is_empty() || ty.starts_with("audio") || ty.starts_with("video");
                if playable && enclosure.is_none() {
                    enclosure = Some((url.to_string(), ty.starts_with("video")));
                }
                duration = duration.or(c.duration.map(|d| d.as_secs() as i64));
            }
        }
        let Some((url, is_video)) = enclosure else { continue };
        let notes = entry
            .content
            .as_ref()
            .and_then(|c| c.body.clone())
            .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
            .or(media_desc)
            .filter(|s| !s.trim().is_empty());
        episodes.push(Episode {
            id: rss_id(&url),
            show_id: Some(feed_url.to_owned()),
            show_title: title.clone(),
            title: entry.title.as_ref().map(|t| t.content.trim().to_owned()).unwrap_or_default(),
            published_at: entry.published.or(entry.updated).map(|d| d.timestamp()),
            duration_secs: raw_durations
                .as_ref()
                .and_then(|r| r[i])
                .or(duration)
                .filter(|d| *d > 0),
            description: notes.map(|n| sanitize::notes_html(&n)),
            thumbnail: thumb.or_else(|| thumbnail.clone()),
            video_id: None,
            enclosure_url: Some(url),
            guid: Some(entry.id.clone()).filter(|g| !g.is_empty()),
            is_video,
            chapters_url: chapter_urls.as_ref().and_then(|c| c[i].clone()),
            ..Default::default()
        });
    }
    Ok((show, episodes))
}

/// Each `<item>`'s `<itunes:duration>` in seconds, in document order. Accepts seconds, `MM:SS`
/// and `H:MM:SS` (the forms Apple documents and feeds actually use).
fn itunes_durations(bytes: &[u8]) -> Vec<Option<i64>> {
    use regex::bytes::Regex;
    use std::sync::OnceLock;
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static DUR: OnceLock<Regex> = OnceLock::new();
    let item = ITEM.get_or_init(|| Regex::new(r"(?is)<item\b.*?</item>").unwrap());
    let dur = DUR.get_or_init(|| Regex::new(r"(?is)<itunes:duration>\s*([^<]*?)\s*</itunes:duration>").unwrap());
    item.find_iter(bytes)
        .map(|m| {
            let c = dur.captures(m.as_bytes())?;
            let t = std::str::from_utf8(&c[1]).ok()?.trim();
            if t.contains(':') {
                let p: Vec<i64> = t.split(':').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
                match p[..] {
                    [m, s] => Some(m * 60 + s),
                    [h, m, s] => Some(h * 3600 + m * 60 + s),
                    _ => None,
                }
            } else {
                t.parse::<f64>().ok().map(|s| s as i64)
            }
        })
        .collect()
}

/// stream: podcast-upnext. The channel's `<itunes:type>serial</itunes:type>` (feed-rs drops it).
fn is_serial_feed(bytes: &[u8]) -> bool {
    use regex::bytes::Regex;
    use std::sync::OnceLock;
    static TYPE: OnceLock<Regex> = OnceLock::new();
    let re = TYPE.get_or_init(|| Regex::new(r"(?is)<itunes:type>\s*serial\s*</itunes:type>").unwrap());
    // Only the channel's: the part before the first item.
    let head = Regex::new(r"(?i)<item\b").ok().and_then(|r| r.find(bytes)).map_or(bytes.len(), |m| m.start());
    re.is_match(&bytes[..head])
}

/// Each `<item>`'s `<podcast:chapters url=…>` (JSON chapters), in document order.
fn item_chapter_urls(bytes: &[u8]) -> Vec<Option<String>> {
    use regex::bytes::Regex;
    use std::sync::OnceLock;
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static URL: OnceLock<Regex> = OnceLock::new();
    let item = ITEM.get_or_init(|| Regex::new(r"(?is)<item\b.*?</item>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"(?is)<podcast:chapters\b[^>]*>").unwrap());
    let url = URL.get_or_init(|| Regex::new(r#"(?is)\burl\s*=\s*["']([^"']+)["']"#).unwrap());
    item.find_iter(bytes)
        .map(|m| {
            let t = tag.find(m.as_bytes())?;
            let c = url.captures(t.as_bytes())?;
            let u = std::str::from_utf8(&c[1]).ok()?.trim().replace("&amp;", "&");
            u.starts_with("http").then_some(u)
        })
        .collect()
}

/// The result of a conditional fetch.
pub enum Fetched {
    NotModified,
    Body { bytes: Vec<u8>, etag: Option<String>, last_modified: Option<String> },
}

/// GET a feed, conditionally when validators are given.
pub async fn fetch(
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<Fetched, String> {
    let mut req = crate::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .header(reqwest::header::ACCEPT, "application/rss+xml, application/atom+xml, application/xml;q=0.9, */*;q=0.8")
        .timeout(Duration::from_secs(30));
    if let Some(e) = etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, e);
    }
    if let Some(l) = last_modified {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, l);
    }
    let resp = req.send().await.map_err(|e| format!("feed fetch failed: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !resp.status().is_success() {
        return Err(format!("feed fetch failed: HTTP {}", resp.status()));
    }
    let header = |name: reqwest::header::HeaderName| {
        resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
    };
    let etag = header(reqwest::header::ETAG);
    let last_modified = header(reqwest::header::LAST_MODIFIED);
    if resp.content_length().is_some_and(|n| n as usize > MAX_FEED_BYTES) {
        return Err("feed too large".into());
    }
    let bytes = resp.bytes().await.map_err(|e| format!("feed read failed: {e}"))?;
    if bytes.len() > MAX_FEED_BYTES {
        return Err("feed too large".into());
    }
    Ok(Fetched::Body { bytes: bytes.to_vec(), etag, last_modified })
}

/// Fetch and parse unconditionally (show page, add-by-URL).
pub async fn fetch_show(url: &str) -> Result<(Show, Vec<Episode>, Option<String>, Option<String>), String> {
    match fetch(url, None, None).await? {
        Fetched::Body { bytes, etag, last_modified } => {
            let (s, e) = parse_feed(url, &bytes)?;
            Ok((s, e, etag, last_modified))
        }
        Fetched::NotModified => Err("feed fetch failed: unexpected 304".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed() -> (Show, Vec<Episode>) {
        let p = format!("{}/src/podcasts/fixtures/feed.xml", env!("CARGO_MANIFEST_DIR"));
        parse_feed("https://example.com/feed.xml", &std::fs::read(p).unwrap()).unwrap()
    }

    #[test]
    fn serial_type_is_the_channels() {
        let serial = br#"<rss><channel><itunes:type> serial </itunes:type><item><title>a</title></item></channel></rss>"#;
        assert!(is_serial_feed(serial));
        let item_only = br#"<rss><channel><itunes:type>episodic</itunes:type><item><itunes:type>serial</itunes:type></item></channel></rss>"#;
        assert!(!is_serial_feed(item_only));
        assert!(!feed().0.serial);
    }

    #[test]
    fn show_fields() {
        let (s, _) = feed();
        assert_eq!(s.id, "https://example.com/feed.xml");
        assert_eq!(s.title, "The Fixture Show");
        assert_eq!(s.author, "Fixture Media");
        assert_eq!(s.thumbnail.as_deref(), Some("https://example.com/art.jpg"));
        assert_eq!(s.description.as_deref(), Some("A show about fixtures & tests."));
        assert_eq!(s.source, ShowSource::Rss);
    }

    #[test]
    fn episodes_with_enclosures_only() {
        let (_, eps) = feed();
        assert_eq!(eps.len(), 3, "the text-only post is not an episode");
        let e3 = &eps[0];
        assert_eq!(e3.title, "Episode 3: Video");
        assert_eq!(e3.id, "PODURL:https://cdn.example.com/ep3.mp4");
        assert!(e3.is_video);
        assert_eq!(e3.duration_secs, Some(3725));
        assert_eq!(e3.guid.as_deref(), Some("ep-3"));
        assert_eq!(e3.thumbnail.as_deref(), Some("https://example.com/art.jpg"));
        assert!(e3.description.as_deref().unwrap().contains(r#"data-seek="30""#));

        let e2 = &eps[1];
        assert_eq!(
            e2.enclosure_url.as_deref(),
            Some("https://dts.podtrac.com/redirect.mp3/cdn.example.com/ep2.mp3?x=1&y=2")
        );
        assert_eq!(e2.duration_secs, Some(3723));
        assert_eq!(e2.thumbnail.as_deref(), Some("https://example.com/ep2.jpg"));
        let notes = e2.description.as_deref().unwrap();
        assert!(notes.contains("Full notes"), "content:encoded wins over description: {notes}");
        assert!(!notes.contains("script"));
        assert!(notes.contains(r#"data-seek="725""#));
        assert_eq!(e2.published_at, Some(1_793_610_000));
        assert_eq!(e2.chapters_url.as_deref(), Some("https://example.com/ep2/chapters.json?a=1&b=2"));
        assert_eq!(e3.chapters_url, None);

        assert_eq!(eps[2].duration_secs, Some(2710));
        assert_eq!(eps[2].description, None);
        assert!(!eps[2].is_video);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_feed("u", b"<html><body>nope</body></html>").is_err());
    }
}
