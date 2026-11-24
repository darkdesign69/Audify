//! RSS → YouTube episode matching (research 20 §match; podcasts.md "Known gaps").
//!
//! An RSS-only episode plays its enclosure, which often carries dynamically inserted ads. Many
//! shows also publish every episode on YouTube, where the app plays it ad-free. So before an RSS
//! episode is resolved, look for the same episode on YouTube Music:
//!
//! 1. search Episodes for "<show> <episode title>";
//! 2. keep rows from the same show whose title is the same episode ([`same_title`], identity.rs
//!    normalisation, YouTube's "| Show name" / "#123" dressing tolerated) and, when both dates are
//!    known, published within 3 days ([`shortlist`]);
//! 3. read the best candidates' episode pages for their length and accept the first within
//!    ±2 % of the RSS duration (at least ±5 s) ([`duration_ok`]).
//!
//! Matches *and* misses are cached in `polish_pod_twins` (a miss is retried after 3 days: the
//! YouTube upload can come later). On by default; setting `podcast_youtube_match` = `false` turns
//! it off. The queue row keeps its RSS id, so progress, resume and "played" stay on the episode;
//! only the stream comes from YouTube.

use std::collections::HashSet;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};

use crate::db::Db;
use crate::identity;
use crate::podcasts::{self, Episode};
use crate::state::AppState;

pub const SETTING: &str = "podcast_youtube_match";
const MISS_RETRY_SECS: i64 = 3 * 86_400;
const DATE_SLACK_SECS: i64 = 3 * 86_400;

pub fn enabled(db: &Db) -> bool {
    db.get_setting(SETTING).as_deref() != Some("false")
}

/// Settings › Podcasts "Play ad-free YouTube versions"; answers the new value.
pub fn set_enabled(db: &Db, on: bool) -> bool {
    db.set_setting(SETTING, if on { "true" } else { "false" });
    on
}

fn tokens(s: &str) -> Vec<String> {
    identity::norm(s).split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

/// The same episode title, allowing for YouTube's extra dressing: every word of the RSS title
/// (in order-insensitive form) must be in the candidate, and the candidate may add at most a few
/// words (the show name, an episode number, a guest tag) beyond those.
pub fn same_title(rss: &str, cand: &str, show: &str) -> bool {
    // YouTube titles often carry a "| Guest Name" or "| Show" tail: try the parts on their own too.
    std::iter::once(cand)
        .chain(cand.split('|').filter(|_| cand.contains('|')))
        .any(|c| same_title_one(rss, c, show))
}

fn same_title_one(rss: &str, cand: &str, show: &str) -> bool {
    let a = tokens(rss);
    if a.is_empty() {
        return false;
    }
    let show_words: HashSet<String> = tokens(show).into_iter().collect();
    let b: Vec<String> = tokens(cand);
    if identity::norm(rss) == identity::norm(cand) {
        return true;
    }
    let bs: HashSet<&str> = b.iter().map(String::as_str).collect();
    let found = a.iter().filter(|w| bs.contains(w.as_str())).count();
    let recall = found as f64 / a.len() as f64;
    let a_set: HashSet<&str> = a.iter().map(String::as_str).collect();
    // Extra words in the candidate, not counting the show's name and bare numbers.
    let extra = b
        .iter()
        .filter(|w| !a_set.contains(w.as_str()) && !show_words.contains(*w))
        .filter(|w| {
            !w.chars().all(|c| c.is_ascii_digit()) && w.as_str() != "ep" && w.as_str() != "episode"
        })
        .count();
    recall >= 0.9 && extra <= (a.len() / 3).max(2)
}

fn same_show(rss_show: &str, cand_show: &str) -> bool {
    let (a, b) = (podcasts::search::norm(rss_show), podcasts::search::norm(cand_show));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

/// Search rows that can be the RSS episode, best first (YouTube's order).
pub fn shortlist<'a>(rss: &Episode, cands: &'a [Episode]) -> Vec<&'a Episode> {
    cands
        .iter()
        .filter(|c| c.video_id.is_some())
        .filter(|c| c.show_title.is_empty() || same_show(&rss.show_title, &c.show_title))
        .filter(|c| same_title(&rss.title, &c.title, &rss.show_title))
        .filter(|c| match (rss.published_at, c.published_at) {
            (Some(a), Some(b)) => (a - b).abs() <= DATE_SLACK_SECS,
            _ => true,
        })
        .take(3)
        .collect()
}

/// ±2 % of the RSS length, at least ±5 s. With no RSS length only an exact title match counts,
/// which the caller decides; here an unknown length on either side is not a match.
pub fn duration_ok(rss_secs: Option<i64>, yt_secs: Option<i64>) -> bool {
    match (rss_secs.filter(|s| *s > 0), yt_secs.filter(|s| *s > 0)) {
        (Some(a), Some(b)) => (a - b).abs() as f64 <= (a as f64 * 0.02).max(5.0),
        _ => false,
    }
}

/// Cached answer: `Some(Some(id))` a twin, `Some(None)` a recent miss, `None` never asked / stale.
fn cached(db: &Db, id: &str, now: i64) -> Option<Option<String>> {
    let row: Option<(Option<String>, i64)> = db.with_conn(|c| {
        c.query_row("SELECT video_id, checked_at FROM polish_pod_twins WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .ok()
        .flatten()
    });
    match row? {
        (Some(v), _) => Some(Some(v)),
        (None, at) if now - at < MISS_RETRY_SECS => Some(None),
        _ => None,
    }
}

pub fn remember(db: &Db, id: &str, twin: Option<&str>, now: i64) {
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO polish_pod_twins(id, video_id, checked_at) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET video_id = excluded.video_id, checked_at = excluded.checked_at",
            params![id, twin, now],
        );
    });
}

/// The cached twin of an RSS episode, without any network. For the UI's "from YouTube" badge.
pub fn known_twin(db: &Db, id: &str) -> Option<String> {
    cached(db, id, super::now_secs()).flatten()
}

async fn search(
    it: &innertube::InnerTube,
    client: &innertube::YouTubeClient,
    rss: &Episode,
) -> Option<String> {
    let q = format!("{} {}", rss.show_title, rss.title);
    let cands = podcasts::youtube::search_episodes(it, client, &q).await.ok()?;
    let exact = |c: &Episode| identity::norm(&c.title) == identity::norm(&rss.title);
    for c in shortlist(rss, &cands) {
        let vid = c.video_id.as_deref()?;
        if rss.duration_secs.is_none() {
            // No length to check against: only a word-for-word title from the same show.
            if exact(c) && !c.show_title.is_empty() {
                return Some(vid.to_owned());
            }
            continue;
        }
        let page = podcasts::youtube::fetch_episode(it, client, vid).await.ok();
        if duration_ok(rss.duration_secs, page.and_then(|p| p.duration_secs)) {
            return Some(vid.to_owned());
        }
    }
    None
}

/// The YouTube video to stream for RSS episode `id` instead of its enclosure, if any. Bounded to a
/// few seconds; the enclosure is the fallback on any failure. `duration_secs` is the queue row's
/// length, used when the stored episode has none.
pub async fn twin_for(st: &AppState, id: &str, duration_secs: i64) -> Option<String> {
    if !podcasts::is_rss_id(id) || !enabled(&st.db) {
        return None;
    }
    let now = super::now_secs();
    if let Some(hit) = cached(&st.db, id, now) {
        return hit;
    }
    let mut rss = podcasts::store::get_episode(&st.db.podcasts_conn(), id)?;
    if rss.title.trim().is_empty() || rss.show_title.trim().is_empty() {
        return None;
    }
    let client = st.clients.get(innertube::METADATA_CLIENT)?;
    if rss.duration_secs.is_none() && duration_secs > 0 {
        rss.duration_secs = Some(duration_secs);
    }
    match tokio::time::timeout(Duration::from_secs(6), search(&st.it, client, &rss)).await {
        Ok(found) => {
            remember(&st.db, id, found.as_deref(), now);
            if let Some(v) = &found {
                tracing::info!(id, twin = %v, "RSS episode plays its YouTube twin");
            }
            found
        }
        // Timed out: say nothing to the cache, try again next time.
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_episodes() -> Vec<Episode> {
        let p =
            format!("{}/src/podcasts/fixtures/yt_search_episodes.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        podcasts::youtube::parse_episode_search(&v, 1_794_744_000)
    }

    fn rss(title: &str, show: &str) -> Episode {
        Episode {
            id: "PODURL:https://example.com/ep.mp3".into(),
            title: title.into(),
            show_title: show.into(),
            ..Default::default()
        }
    }

    #[test]
    fn titles_tolerate_youtube_dressing_but_not_other_episodes() {
        let show = "Huberman Lab";
        assert!(same_title(
            "Overcome Addictions and Obsessions of All Kinds",
            "Overcome Addictions and Obsessions of All Kinds | Dr. Anna Lembke",
            show
        ));
        assert!(same_title(
            "Tools to Improve Your Focus",
            "Tools to Improve Your Focus | Huberman Lab #212",
            show
        ));
        assert!(same_title("Don't Stop", "Dont stop", show));
        assert!(!same_title(
            "Tools to Improve Your Focus & Concentration",
            "Maximize Productivity, Physical & Mental Health With Daily Tools",
            show
        ));
        assert!(!same_title(
            "Sleep",
            "Sleep Toolkit: Optimize Sleep and Wake Up Feeling Better Than Ever",
            show
        ));
    }

    #[test]
    fn shortlist_picks_the_same_episode_from_a_live_search() {
        let cands = fixture_episodes();
        assert!(cands.len() >= 3);
        let want = rss("Overcome Addictions and Obsessions of All Kinds", "Huberman Lab");
        let got = shortlist(&want, &cands);
        assert_eq!(got.len(), 1);
        assert!(got[0].title.starts_with("Overcome Addictions"));
        // Another show with the same words is not it.
        let other = rss("Overcome Addictions and Obsessions of All Kinds", "The Daily");
        assert!(shortlist(&other, &cands).is_empty());
        // Published weeks apart: not it.
        let mut late = want.clone();
        late.published_at = got[0].published_at.map(|d| d + 30 * 86_400);
        assert!(shortlist(&late, &cands).is_empty());
    }

    #[test]
    fn duration_within_two_percent() {
        assert!(duration_ok(Some(3600), Some(3640)));
        assert!(!duration_ok(Some(3600), Some(3800)), "inserted ads or another cut");
        assert!(duration_ok(Some(120), Some(124)), "5 s floor");
        assert!(!duration_ok(None, Some(100)));
        assert!(!duration_ok(Some(100), None));
    }

    #[test]
    fn misses_expire_matches_stay() {
        let db = crate::polish::test_db();
        remember(&db, "PODURL:a", Some("vidA"), 100);
        remember(&db, "PODURL:b", None, 100);
        assert_eq!(cached(&db, "PODURL:a", 100 + 10 * MISS_RETRY_SECS), Some(Some("vidA".into())));
        assert_eq!(cached(&db, "PODURL:b", 200), Some(None));
        assert_eq!(cached(&db, "PODURL:b", 100 + MISS_RETRY_SECS + 1), None);
        assert_eq!(cached(&db, "PODURL:c", 0), None);
    }

    /// `cargo test -p limusic-app --lib polish::podmatch -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "hits YouTube and a real feed"]
    async fn live_rss_episodes_find_their_youtube_twins() {
        let it = innertube::InnerTube::new(innertube::Session::default(), None).unwrap();
        let clients = innertube::Clients::bundled();
        let client = clients.get(innertube::METADATA_CLIENT).unwrap();
        let (show, eps, _, _) =
            podcasts::rss::fetch_show("https://feeds.megaphone.fm/hubermanlab").await.unwrap();
        let mut found = 0;
        for e in eps.iter().skip(1).take(4) {
            let mut e = e.clone();
            e.show_title = show.title.clone();
            let twin = search(&it, client, &e).await;
            println!("{:?} ({:?}s) -> {twin:?}", e.title, e.duration_secs);
            found += twin.is_some() as usize;
        }
        assert!(found >= 1);
    }
}
