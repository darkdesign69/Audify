//! Show notes as safe HTML. Feeds ship arbitrary HTML (scripts, iframes, tracking pixels, inline
//! styles); ammonia (the html5ever-based sanitiser Lemmy, crates.io and feed-rs itself use) keeps
//! the structural tags and links and drops everything else. Timestamps ("12:34", "(01:02:03)")
//! become `<a data-seek="754">` links the episode page turns into seeks, like Spotify's.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

fn cleaner() -> &'static ammonia::Builder<'static> {
    static B: OnceLock<ammonia::Builder<'static>> = OnceLock::new();
    B.get_or_init(|| {
        let tags: HashSet<&str> = [
            "a", "b", "strong", "i", "em", "u", "p", "br", "ul", "ol", "li", "blockquote", "h1",
            "h2", "h3", "h4", "h5", "h6", "code", "pre", "hr", "span", "div",
        ]
        .into_iter()
        .collect();
        let mut attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
        attrs.insert("a", ["href", "data-seek"].into_iter().collect());
        let mut b = ammonia::Builder::default();
        b.tags(tags)
            .tag_attributes(attrs)
            .generic_attributes(HashSet::new())
            .url_schemes(["http", "https", "mailto"].into_iter().collect())
            .link_rel(Some("noopener noreferrer nofollow"))
            .set_tag_attribute_value("a", "target", "_blank");
        b
    })
}

/// Sanitise feed HTML. Plain text (no tags at all) is escaped and its line breaks kept.
pub fn notes_html(input: &str) -> String {
    let input = input.trim();
    let html = if looks_like_html(input) { input.to_owned() } else { text_to_html(input) };
    cleaner().clean(&link_timestamps(&html)).to_string()
}

fn looks_like_html(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)</?(p|br|a|div|ul|ol|li|b|strong|i|em|span)\b").unwrap())
        .is_match(s)
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Plain text → HTML: escaped, bare URLs linked, newlines as `<br>`.
pub fn text_to_html(s: &str) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    let url = URL.get_or_init(|| Regex::new(r#"https?://[^\s<>"]+"#).unwrap());
    let mut out = String::new();
    let mut last = 0;
    for m in url.find_iter(s) {
        out.push_str(&escape(&s[last..m.start()]));
        let u = escape(m.as_str());
        out.push_str(&format!("<a href=\"{u}\">{u}</a>"));
        last = m.end();
    }
    out.push_str(&escape(&s[last..]));
    out.replace("\r\n", "\n").replace('\n', "<br>")
}

/// YouTube description runs (text + `urlEndpoint` links) → HTML.
pub fn runs_to_html(runs: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for r in runs {
        let text = r.get("text").and_then(|t| t.as_str()).unwrap_or_default();
        let url = r.pointer("/navigationEndpoint/urlEndpoint/url").and_then(|u| u.as_str());
        match url {
            Some(u) => out.push_str(&format!("<a href=\"{}\">{}</a>", escape(u), escape(text))),
            None => out.push_str(&escape(text)),
        }
    }
    notes_html(&out.replace('\n', "<br>"))
}

/// HTML → plain text, for descriptions shown as one line (show cards).
pub fn to_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"(?is)<br\s*/?>|</p>|<[^>]+>").unwrap());
    let s = tag.replace_all(html, |c: &regex::Captures| {
        let t = c[0].to_ascii_lowercase();
        if t.starts_with("<br") || t == "</p>" {
            "\n"
        } else {
            ""
        }
    });
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_owned()
}

/// `h:mm:ss` / `mm:ss` → seconds.
pub fn parse_clock(s: &str) -> Option<i64> {
    let parts: Vec<i64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts[..] {
        [m, s] if s < 60 => Some(m * 60 + s),
        [h, m, s] if m < 60 && s < 60 => Some(h * 3600 + m * 60 + s),
        _ => None,
    }
}

/// Wrap timestamps outside tags/links in seek links. Skips text inside an existing `<a>`.
fn link_timestamps(html: &str) -> String {
    static TS: OnceLock<Regex> = OnceLock::new();
    let ts = TS.get_or_init(|| Regex::new(r"\b(\d{1,2}:)?\d{1,2}:\d{2}\b").unwrap());
    let mut out = String::with_capacity(html.len());
    let mut in_link = 0i32;
    // Walk tag by tag; only rewrite text segments.
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(lt) = rest.find('<') {
            let (text, tail) = rest.split_at(lt);
            out.push_str(&rewrite(text, in_link > 0, ts));
            let end = tail.find('>').map(|i| i + 1).unwrap_or(tail.len());
            let tag = &tail[..end];
            let low = tag.to_ascii_lowercase();
            if low.starts_with("<a ") || low == "<a>" {
                in_link += 1;
            } else if low.starts_with("</a") {
                in_link -= 1;
            }
            out.push_str(tag);
            rest = &tail[end..];
        } else {
            out.push_str(&rewrite(rest, in_link > 0, ts));
            break;
        }
    }
    out
}

fn rewrite(text: &str, in_link: bool, ts: &Regex) -> String {
    if in_link {
        return text.to_owned();
    }
    ts.replace_all(text, |c: &regex::Captures| match parse_clock(&c[0]) {
        Some(secs) => format!("<a href=\"#t={secs}\" data-seek=\"{secs}\">{}</a>", &c[0]),
        None => c[0].to_owned(),
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_and_keeps_links() {
        let out = notes_html(
            r#"<p>Hi <script>alert(1)</script><a href="https://x.com" onclick="evil()">x</a><img src="https://t/pixel.gif"><iframe src="https://e"></iframe></p>"#,
        );
        assert!(!out.contains("script"), "{out}");
        assert!(!out.contains("onclick"), "{out}");
        assert!(!out.contains("<img"), "{out}");
        assert!(!out.contains("iframe"), "{out}");
        assert!(out.contains(r#"href="https://x.com""#), "{out}");
        assert!(out.contains("noopener"), "{out}");
    }

    #[test]
    fn javascript_urls_are_dropped() {
        let out = notes_html(r#"<p><a href="javascript:alert(1)">x</a></p>"#);
        assert!(!out.contains("javascript"), "{out}");
    }

    #[test]
    fn plain_text_is_escaped_linked_and_timestamped() {
        let out = notes_html("Intro <3\n(00:02:19) Brain & body\nhttps://example.com/a?b=1\n12:05 end");
        assert!(out.contains("Intro &lt;3<br>"), "{out}");
        assert!(out.contains(r#"data-seek="139""#), "{out}");
        assert!(out.contains(r#"data-seek="725""#), "{out}");
        assert!(out.contains(r#"<a href="https://example.com/a?b=1""#), "{out}");
    }

    #[test]
    fn timestamps_inside_links_are_left_alone() {
        let out = notes_html(r#"<p><a href="https://x">at 1:00</a> and 2:00</p>"#);
        assert_eq!(out.matches("data-seek").count(), 1, "{out}");
        assert!(out.contains(r#"data-seek="120""#), "{out}");
    }

    #[test]
    fn to_text_flattens() {
        assert_eq!(to_text("<p>a &amp; b</p><p>c<br/>d</p>"), "a & b\nc\nd");
        assert_eq!(parse_clock("1:02:03"), Some(3723));
        assert_eq!(parse_clock("75:00"), Some(4500));
        assert_eq!(parse_clock("1:75"), None);
    }
}
