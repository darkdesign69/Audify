//! Directory search for shows that aren't on YouTube Music.
//!
//! - **iTunes Search API** (default, keyless): `media=podcast`, shows (`entity=podcast`) and
//!   episodes (`entity=podcastEpisode`, which carry the enclosure as `episodeUrl`). Apple allows
//!   ~20 calls/min per IP, so results are cached for 24 h in `podcast_http_cache`.
//! - **Podcast Index** (`search/byterm`): only when the user pasted an API key and secret
//!   (settings `podcastindex_key` / `podcastindex_secret`); auth is
//!   `Authorization: sha1(key + secret + unix_time)` per the Podcast Index docs.

use std::time::Duration;

use serde_json::Value;
use sha1::{Digest, Sha1};

use super::{rss_id, sanitize, store, Episode, Show, ShowSource};

const CACHE_TTL: i64 = 24 * 3600;
const UA: &str = "Spotless/1.0";

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned).filter(|x| !x.is_empty())
}

/// iTunes `entity=podcast` results → shows (only those with a feed URL: nothing else is playable).
pub fn parse_shows(v: &Value) -> Vec<Show> {
    v.get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let feed = s(r, "feedUrl")?;
                    Some(Show {
                        id: feed.clone(),
                        title: s(r, "collectionName")?,
                        author: s(r, "artistName").unwrap_or_default(),
                        thumbnail: s(r, "artworkUrl600").or_else(|| s(r, "artworkUrl100")),
                        feed_url: Some(feed),
                        itunes_id: r.get("collectionId").and_then(Value::as_i64),
                        episode_count: r.get("trackCount").and_then(Value::as_u64).map(|n| n as u32),
                        source: ShowSource::Rss,
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// iTunes `entity=podcastEpisode` results → episodes playing their enclosure.
pub fn parse_episodes(v: &Value) -> Vec<Episode> {
    v.get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|r| r.get("wrapperType").and_then(Value::as_str) == Some("podcastEpisode"))
                .filter_map(|r| {
                    let url = s(r, "episodeUrl")?;
                    Some(Episode {
                        id: rss_id(&url),
                        show_id: s(r, "feedUrl"),
                        show_title: s(r, "collectionName").unwrap_or_default(),
                        title: s(r, "trackName")?,
                        published_at: s(r, "releaseDate")
                            .and_then(|d| chrono::DateTime::parse_from_rfc3339(&d).ok())
                            .map(|d| d.timestamp()),
                        duration_secs: r
                            .get("trackTimeMillis")
                            .and_then(Value::as_i64)
                            .map(|ms| ms / 1000)
                            .filter(|d| *d > 0),
                        description: s(r, "description").map(|d| sanitize::notes_html(&d)),
                        thumbnail: s(r, "artworkUrl600").or_else(|| s(r, "artworkUrl160")),
                        is_video: s(r, "episodeContentType").as_deref() == Some("video"),
                        enclosure_url: Some(url),
                        guid: s(r, "episodeGuid"),
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Podcast Index `search/byterm` → shows.
pub fn parse_podcastindex(v: &Value) -> Vec<Show> {
    v.get("feeds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    let url = s(f, "url")?;
                    Some(Show {
                        id: url.clone(),
                        title: s(f, "title")?,
                        author: s(f, "author").or_else(|| s(f, "ownerName")).unwrap_or_default(),
                        thumbnail: s(f, "artwork").or_else(|| s(f, "image")),
                        description: s(f, "description").map(|d| sanitize::to_text(&d)),
                        feed_url: Some(url),
                        itunes_id: f.get("itunesId").and_then(Value::as_i64),
                        episode_count: f.get("episodeCount").and_then(Value::as_u64).map(|n| n as u32),
                        source: ShowSource::Rss,
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) async fn get_json(db: &crate::db::Db, url: &str, headers: &[(&str, String)]) -> Result<Value, String> {
    let cached = store::cache_get(&db.podcasts_conn(), url, CACHE_TTL);
    if let Some(body) = cached {
        if let Ok(v) = serde_json::from_str(&body) {
            return Ok(v);
        }
    }
    let mut req = crate::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .timeout(Duration::from_secs(15));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    store::cache_put(&db.podcasts_conn(), url, &body);
    Ok(v)
}

pub(crate) fn country(db: &crate::db::Db) -> String {
    db.get_setting("podcast_country")
        .filter(|c| c.len() == 2)
        .unwrap_or_else(|| "US".into())
}

pub async fn search_shows(db: &crate::db::Db, term: &str) -> Result<Vec<Show>, String> {
    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=25&country={}&term={}",
        country(db),
        urlencoding::encode(term)
    );
    Ok(parse_shows(&get_json(db, &url, &[]).await?))
}

pub async fn search_episodes(db: &crate::db::Db, term: &str) -> Result<Vec<Episode>, String> {
    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcastEpisode&limit=25&country={}&term={}",
        country(db),
        urlencoding::encode(term)
    );
    Ok(parse_episodes(&get_json(db, &url, &[]).await?))
}

/// Podcast Index credentials, when the user configured them.
pub fn podcastindex_keys(db: &crate::db::Db) -> Option<(String, String)> {
    let k = db.get_setting("podcastindex_key").filter(|k| !k.trim().is_empty())?;
    let s = db.get_setting("podcastindex_secret").filter(|s| !s.trim().is_empty())?;
    Some((k.trim().to_owned(), s.trim().to_owned()))
}

/// The Podcast Index `Authorization` header: hex sha1 of key + secret + date.
pub fn podcastindex_auth(key: &str, secret: &str, date: i64) -> String {
    let mut h = Sha1::new();
    h.update(format!("{key}{secret}{date}").as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn search_podcastindex(db: &crate::db::Db, term: &str) -> Result<Vec<Show>, String> {
    let Some((key, secret)) = podcastindex_keys(db) else { return Ok(Vec::new()) };
    let date = super::now_secs();
    let url = format!(
        "https://api.podcastindex.org/api/1.0/search/byterm?max=25&q={}",
        urlencoding::encode(term)
    );
    let headers = [
        ("X-Auth-Key", key.clone()),
        ("X-Auth-Date", date.to_string()),
        ("Authorization", podcastindex_auth(&key, &secret, date)),
    ];
    Ok(parse_podcastindex(&get_json(db, &url, &headers).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let p = format!("{}/src/podcasts/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn itunes_shows() {
        let shows = parse_shows(&fixture("itunes_shows.json"));
        let h = &shows[0];
        assert_eq!(h.title, "Huberman Lab");
        assert_eq!(h.author, "Scicomm Media");
        assert_eq!(h.id, "https://feeds.megaphone.fm/hubermanlab");
        assert_eq!(h.itunes_id, Some(1545953110));
        assert_eq!(h.source, ShowSource::Rss);
    }

    #[test]
    fn itunes_episodes() {
        let eps = parse_episodes(&fixture("itunes_episodes.json"));
        assert!(!eps.is_empty());
        let e = &eps[0];
        assert!(e.id.starts_with("PODURL:https://"));
        assert_eq!(e.show_id.as_deref(), Some("https://feeds.megaphone.fm/hubermanlab"));
        assert_eq!(e.duration_secs, Some(2315));
        assert!(e.published_at.is_some());
    }

    #[test]
    fn podcastindex_parse_and_auth() {
        let v = serde_json::json!({"feeds": [
            {"url": "https://f/x.xml", "title": "X", "author": "A", "artwork": "https://a.jpg", "itunesId": 5, "episodeCount": 9},
            {"title": "no url"}
        ]});
        let shows = parse_podcastindex(&v);
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].episode_count, Some(9));
        // sha1("k" + "s" + "1") — fixed vector.
        assert_eq!(podcastindex_auth("k", "s", 1), "4e1e7e5b6f5006b89c667560197dd4bb28a9b362");
    }
}
