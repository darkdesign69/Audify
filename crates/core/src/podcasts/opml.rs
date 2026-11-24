//! OPML import / export of follows (the `opml` crate, MIT/Apache).
//!
//! A YouTube-only show has no RSS feed of its own, but YouTube publishes one per playlist
//! (`/feeds/videos.xml?playlist_id=PL…`), and an `MPSP…` show id is `MPSP` + that playlist id.
//! Export writes that URL so other podcast apps can still subscribe, and import maps it back to
//! the show page, so a round trip loses nothing.

use std::sync::OnceLock;

use regex::Regex;

use super::Show;

const YT_FEED: &str = "https://www.youtube.com/feeds/videos.xml?playlist_id=";

/// A follow read from OPML.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    /// `MPSP…` for YouTube playlist feeds, the feed URL otherwise.
    pub id: String,
    pub feed_url: Option<String>,
}

impl Entry {
    pub fn show(&self) -> Show {
        let mut s = if self.id.starts_with("MPSP") { Show::youtube(&self.id) } else { Show::rss(&self.id) };
        s.title = self.title.clone();
        s
    }
}

/// The OPML `xmlUrl` for a show.
pub fn feed_url_for(show: &Show) -> Option<String> {
    show.feed_url.clone().or_else(|| {
        show.yt_browse_id
            .as_deref()
            .or(Some(show.id.as_str()))
            .and_then(|id| id.strip_prefix("MPSP"))
            .map(|pl| format!("{YT_FEED}{pl}"))
    })
}

/// Map a feed URL back to a show id (`MPSP…` for a YouTube playlist feed).
pub fn id_for_feed(url: &str) -> String {
    match url.strip_prefix(YT_FEED) {
        Some(pl) if !pl.is_empty() => format!("MPSP{}", pl.split('&').next().unwrap_or(pl)),
        _ => url.to_owned(),
    }
}

pub fn export(shows: &[Show]) -> Result<String, String> {
    let mut doc = opml::OPML {
        version: "2.0".into(),
        head: Some(opml::Head { title: Some("Spotless podcast subscriptions".into()), ..Default::default() }),
        ..Default::default()
    };
    for s in shows {
        let Some(url) = feed_url_for(s) else { continue };
        doc.body.outlines.push(opml::Outline {
            text: s.title.clone(),
            title: Some(s.title.clone()),
            r#type: Some("rss".into()),
            xml_url: Some(url),
            ..Default::default()
        });
    }
    if doc.body.outlines.is_empty() {
        // The format requires at least one outline.
        doc.body.outlines.push(opml::Outline { text: "No subscriptions".into(), ..Default::default() });
    }
    doc.to_string().map_err(|e| e.to_string())
}

/// Every feed in an OPML file, nested folders included, deduplicated. Falls back to a plain
/// attribute scan when the file isn't strict OPML (several apps export slightly broken XML).
pub fn import(xml: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    match opml::OPML::from_str(xml) {
        Ok(doc) => walk(&doc.body.outlines, &mut out),
        Err(e) => {
            tracing::info!("opml: strict parse failed ({e}), scanning attributes");
            scan(xml, &mut out);
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e.id.clone()));
    out
}

fn push(out: &mut Vec<Entry>, title: &str, url: &str) {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let id = id_for_feed(url);
    let feed_url = (!id.starts_with("MPSP")).then(|| url.to_owned());
    out.push(Entry { title: title.trim().to_owned(), id, feed_url });
}

fn walk(outlines: &[opml::Outline], out: &mut Vec<Entry>) {
    for o in outlines {
        if let Some(url) = &o.xml_url {
            push(out, o.title.as_deref().unwrap_or(&o.text), url);
        }
        walk(&o.outlines, out);
    }
}

fn scan(xml: &str, out: &mut Vec<Entry>) {
    static OUTLINE: OnceLock<Regex> = OnceLock::new();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    let outline = OUTLINE.get_or_init(|| Regex::new(r"(?is)<outline\b[^>]*>").unwrap());
    let attr = ATTR.get_or_init(|| Regex::new(r#"(?i)\b(xmlUrl|text|title)\s*=\s*"([^"]*)""#).unwrap());
    for o in outline.find_iter(xml) {
        let (mut url, mut text) = (None, String::new());
        for c in attr.captures_iter(o.as_str()) {
            let v = unescape(&c[2]);
            match c[1].to_ascii_lowercase().as_str() {
                "xmlurl" => url = Some(v),
                _ if text.is_empty() => text = v,
                _ => {}
            }
        }
        if let Some(u) = url {
            push(out, &text, &u);
        }
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string(format!(
            "{}/src/podcasts/fixtures/subscriptions.opml",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn imports_nested_feeds_and_youtube_playlists() {
        let e = import(&fixture());
        assert_eq!(e.len(), 3, "{e:?}");
        assert_eq!(e[0].title, "The Fixture Show");
        assert_eq!(e[0].id, "https://example.com/feed.xml");
        assert_eq!(e[1].title, "Second & Show");
        assert_eq!(e[2].id, "MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW");
        assert_eq!(e[2].feed_url, None);
    }

    #[test]
    fn round_trip() {
        let mut a = Show::rss("https://example.com/feed.xml");
        a.title = "A & B <show>".into();
        let mut y = Show::youtube("MPSPPLabc");
        y.title = "Yt".into();
        let xml = export(&[a.clone(), y.clone()]).unwrap();
        assert!(xml.contains("playlist_id=PLabc"), "{xml}");
        let back = import(&xml);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].show().id, a.id);
        assert_eq!(back[0].title, a.title);
        assert_eq!(back[1].show().id, y.id);
        assert_eq!(back[1].show().title, "Yt");
    }

    #[test]
    fn broken_xml_still_imports() {
        let xml = r#"<opml version="9"><body><outline text="X" xmlUrl="https://x/f?a=1&amp;b=2"><outline text="bad"></body>"#;
        let e = import(xml);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, "https://x/f?a=1&b=2");
        assert!(import("not xml").is_empty());
        // An empty export is still valid OPML.
        assert!(import(&export(&[]).unwrap()).is_empty());
    }
}
