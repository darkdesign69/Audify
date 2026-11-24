//! What's New (stream smart2): the bell in the top bar. New releases from followed artists (top
//! artists while nothing is followed) and new episodes of followed podcasts, with a "New" dot for
//! anything that arrived since the page was last opened.
//!
//! YouTube artist pages only give a release's year, so a registry (`smart_releases`) remembers
//! when each release was **first seen**. The first scan of an artist files what is already there
//! as "earlier" (`first_seen = 0`), except releases YouTube Music's own New releases page lists
//! right now (they are new by definition); every later scan stamps what appeared since. The same
//! registry feeds New Release Mix. A background refresh runs every 6 hours (`mfy::spawn`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::home::{artist_info, ShelfItem};
use crate::db::Db;
use crate::state::AppState;

const SEEN_KEY: &str = "whatsnew_seen_at";
const CHECKED_KEY: &str = "whatsnew_checked_at";
const REFRESH_SECS: i64 = 6 * 3600;
const ARTISTS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// Album browse id (`MPRE…`).
    pub id: String,
    pub title: String,
    /// "Single • 2026" / "Album • 2026" as YouTube words it.
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub artist: String,
    pub artist_id: String,
    pub year: i32,
    /// Unix secs when first seen; 0 = was already out at the first scan.
    pub first_seen: i64,
    pub followed: bool,
    /// Listed on YouTube Music's New releases page at the last scan.
    pub yt_new: bool,
    /// Position on the artist page (newest first).
    pub position: i64,
}

impl Release {
    /// "Single" / "Album" / "EP" from YouTube's subtitle.
    pub fn release_type(&self) -> String {
        let s = self.subtitle.as_deref().unwrap_or("");
        ["Single", "EP", "Album"]
            .into_iter()
            .find(|t| s.contains(t))
            .unwrap_or("Album")
            .to_owned()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNewRelease {
    #[serde(flatten)]
    pub release: Release,
    pub release_type: String,
    pub is_new: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNewEpisode {
    #[serde(flatten)]
    pub episode: crate::podcasts::Episode,
    pub is_new: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNew {
    pub music: Vec<WhatsNewRelease>,
    pub podcasts: Vec<WhatsNewEpisode>,
    /// When the page was last opened (the "New" cut-off).
    pub seen_at: i64,
    pub new_count: usize,
}

pub fn migrate(db: &Db) {
    db.with_conn(|c| {
        let _ = c.execute_batch(
            "CREATE TABLE IF NOT EXISTS smart_releases (
                id         TEXT PRIMARY KEY,
                artist_id  TEXT NOT NULL,
                json       TEXT NOT NULL,
                first_seen INTEGER NOT NULL,
                year       INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS smart_releases_artist ON smart_releases(artist_id);",
        );
    });
}

// --- registry ------------------------------------------------------------------------------------

pub fn load_releases(db: &Db) -> Vec<Release> {
    db.with_conn(|c| {
        let Ok(mut st) = c.prepare("SELECT json FROM smart_releases") else { return Vec::new() };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|j| serde_json::from_str(&j).ok()).collect())
            .unwrap_or_default()
    })
}

/// File one scan's releases (pure over the db). `scanned` = (artist id, name, followed, its
/// releases with years in page order); `yt_new` = album ids on YouTube's New releases page.
pub fn record_scan(
    db: &Db,
    scanned: &[(String, String, bool, Vec<(ShelfItem, i32)>)],
    yt_new: &HashSet<String>,
    now: i64,
    min_year: i32,
) -> usize {
    let mut added = 0;
    db.with_conn(|c| {
        for (artist_id, artist, followed, releases) in scanned {
            let known: bool = c
                .query_row("SELECT 1 FROM smart_releases WHERE artist_id = ?1 LIMIT 1", [artist_id], |_| Ok(()))
                .is_ok();
            for (pos, (item, year)) in releases.iter().enumerate() {
                if *year < min_year {
                    continue;
                }
                let prev: Option<Release> = c
                    .query_row("SELECT json FROM smart_releases WHERE id = ?1", [&item.id], |r| {
                        r.get::<_, String>(0)
                    })
                    .ok()
                    .and_then(|j| serde_json::from_str(&j).ok());
                let is_yt_new = yt_new.contains(&item.id);
                let first_seen = match &prev {
                    Some(p) => p.first_seen,
                    None if known || is_yt_new => now,
                    None => 0,
                };
                if prev.is_none() && first_seen > 0 {
                    added += 1;
                }
                let r = Release {
                    id: item.id.clone(),
                    title: item.title.clone(),
                    subtitle: item.subtitle.clone(),
                    thumbnail: item.thumbnail.clone(),
                    artist: artist.clone(),
                    artist_id: artist_id.clone(),
                    year: *year,
                    first_seen,
                    followed: *followed,
                    yt_new: is_yt_new,
                    position: pos as i64,
                };
                if let Ok(json) = serde_json::to_string(&r) {
                    let _ = c.execute(
                        "INSERT INTO smart_releases(id, artist_id, json, first_seen, year)
                         VALUES(?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                        rusqlite::params![r.id, r.artist_id, json, r.first_seen, r.year],
                    );
                }
            }
        }
    });
    added
}

/// Followed artists with a channel id (library), then top taste artists, at most `n`.
fn artists_to_watch(db: &Db, n: usize) -> Vec<(String, bool)> {
    let followed: Vec<String> = db.with_conn(|c| {
        let Ok(mut st) = c.prepare(
            "SELECT id FROM library_items WHERE kind = 'artist' ORDER BY added_at DESC",
        ) else {
            return Vec::new();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter(|id| id.starts_with("UC")).collect())
            .unwrap_or_default()
    });
    let mut out: Vec<(String, bool)> = followed.into_iter().map(|id| (id, true)).collect();
    let mut seen: HashSet<String> = out.iter().map(|(id, _)| id.clone()).collect();
    let taste = super::taste::profile(db);
    for (k, _) in taste.top_artists(40) {
        if k.starts_with("UC") && seen.insert(k.clone()) {
            out.push((k, false));
        }
    }
    // Keep room for a few top artists even with a long follow list.
    if out.len() > n {
        let top: Vec<(String, bool)> = out.iter().filter(|(_, f)| !f).take(4).cloned().collect();
        let mut fol: Vec<(String, bool)> = out.into_iter().filter(|(_, f)| *f).take(n - top.len()).collect();
        fol.extend(top);
        out = fol;
    }
    out.truncate(n);
    out
}

/// Scan the watched artists (throttled to every 6 h unless `force`).
pub async fn refresh(state: &AppState, force: bool) -> usize {
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let db = &state.db;
    let now = super::now_secs();
    let checked: i64 = db.get_setting(CHECKED_KEY).and_then(|v| v.parse().ok()).unwrap_or(0);
    if !force && now - checked < REFRESH_SECS {
        return 0;
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let artists = artists_to_watch(db, ARTISTS);
    // ── stream: reliability ── one artist page per followed artist: background priority.
    let infos = innertube::background(futures_util::future::join_all(
        artists.iter().map(|(id, _)| artist_info(state, id)),
    ))
    .await;
    let mut scanned = Vec::new();
    for ((id, followed), info) in artists.iter().zip(infos) {
        if let Some(a) = info {
            scanned.push((id.clone(), a.name.clone(), *followed, a.releases.clone()));
        }
    }
    let mut yt_new = HashSet::new();
    if let Some(client) = state.clients.get(innertube::METADATA_CLIENT) {
        if let Ok(page) = state.it.new_releases(client).await {
            for s in &page.sections {
                for c in &s.items {
                    yt_new.insert(c.id.clone());
                }
            }
        }
    }
    let year = chrono::Datelike::year(&chrono::Local::now());
    let added = record_scan(db, &scanned, &yt_new, now, year - 1);
    db.set_setting(CHECKED_KEY, &now.to_string());
    RUNNING.store(false, Ordering::SeqCst);
    if added > 0 {
        use crate::host::HostExt;
        let _ = state.host.emit("whats-new-changed", serde_json::json!({ "newCount": new_count(db) }));
    }
    added
}

/// Newest first: first seen, then YouTube's "new", then year, then page order.
fn sort_newest(v: &mut [Release]) {
    v.sort_by(|a, b| {
        b.first_seen
            .cmp(&a.first_seen)
            .then(b.yt_new.cmp(&a.yt_new))
            .then(b.year.cmp(&a.year))
            .then(a.position.cmp(&b.position))
    });
}

/// New Release Mix's releases: recent ones of watched artists, one per artist first.
pub async fn radar_releases(state: &AppState, n: usize) -> Vec<Release> {
    refresh(state, false).await;
    radar_pick(load_releases(&state.db), super::now_secs(), n)
}

pub fn radar_pick(all: Vec<Release>, now: i64, n: usize) -> Vec<Release> {
    let year = chrono::Datelike::year(&chrono::Local::now());
    let mut v: Vec<Release> = all
        .into_iter()
        .filter(|r| r.yt_new || (r.first_seen > 0 && now - r.first_seen < 28 * 86_400) || r.year >= year)
        .collect();
    sort_newest(&mut v);
    let mut seen = HashSet::new();
    let (mut first, mut rest): (Vec<Release>, Vec<Release>) = (Vec::new(), Vec::new());
    for r in v {
        if seen.insert(r.artist_id.clone()) {
            first.push(r);
        } else {
            rest.push(r);
        }
    }
    first.extend(rest);
    first.truncate(n);
    first
}

fn seen_at(db: &Db) -> i64 {
    db.get_setting(SEEN_KEY).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// The feed's music part (pure): followed artists' releases (all watched ones when nothing is
/// followed) from this year and last, newest first.
pub fn music_feed(all: Vec<Release>, seen: i64, min_year: i32) -> Vec<WhatsNewRelease> {
    let any_followed = all.iter().any(|r| r.followed);
    let mut v: Vec<Release> =
        all.into_iter().filter(|r| (r.followed || !any_followed) && r.year >= min_year).collect();
    sort_newest(&mut v);
    v.into_iter()
        .take(60)
        .map(|r| {
            let first = if r.first_seen > 0 { r.first_seen } else if r.yt_new { 1 } else { 0 };
            WhatsNewRelease {
                release_type: r.release_type(),
                is_new: r.first_seen > seen || (r.yt_new && seen == 0 && first > 0),
                release: r,
            }
        })
        .collect()
}

pub fn whats_new(db: &Db) -> WhatsNew {
    let seen = seen_at(db);
    let year = chrono::Datelike::year(&chrono::Local::now());
    let music = music_feed(load_releases(db), seen, year - 1);
    let since = super::now_secs() - 30 * 86_400;
    let eps = crate::podcasts::store::new_episodes(&db.podcasts_conn(), since, 50).unwrap_or_default();
    let podcasts: Vec<WhatsNewEpisode> = eps
        .into_iter()
        .map(|e| WhatsNewEpisode { is_new: e.published_at.unwrap_or(0) > seen, episode: e })
        .collect();
    let new_count =
        music.iter().filter(|m| m.is_new).count() + podcasts.iter().filter(|p| p.is_new).count();
    WhatsNew { music, podcasts, seen_at: seen, new_count }
}

pub fn new_count(db: &Db) -> usize {
    whats_new(db).new_count
}

pub fn mark_seen(db: &Db) {
    db.set_setting(SEEN_KEY, &super::now_secs().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(id: &str, year: i32) -> (ShelfItem, i32) {
        (
            ShelfItem {
                kind: "album".into(),
                id: id.into(),
                title: id.into(),
                subtitle: Some(format!("Single \u{2022} {year}")),
                thumbnail: None,
                explicit: false,
                song: None,
                params: None,
                color: None,
                progress: None,
                new_episode: false,
            },
            year,
        )
    }

    #[test]
    fn first_scan_is_earlier_and_later_scans_are_new() {
        let db = super::super::test_db();
        let yt: HashSet<String> = ["hot".to_string()].into_iter().collect();
        let scan1 = vec![("UCa".to_string(), "A".to_string(), true, vec![rel("hot", 2026), rel("old", 2026), rel("ancient", 2019)])];
        assert_eq!(record_scan(&db, &scan1, &yt, 1000, 2025), 1);
        let all = load_releases(&db);
        assert_eq!(all.len(), 2, "years before the cut-off are not filed");
        let get = |id: &str| load_releases(&db).into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(get("old").first_seen, 0);
        assert_eq!(get("hot").first_seen, 1000);
        // Next scan: a brand-new single shows up.
        let scan2 = vec![("UCa".to_string(), "A".to_string(), true, vec![rel("fresh", 2026), rel("hot", 2026), rel("old", 2026)])];
        assert_eq!(record_scan(&db, &scan2, &HashSet::new(), 5000, 2025), 1);
        assert_eq!(get("fresh").first_seen, 5000);
        assert_eq!(get("hot").first_seen, 1000, "first sighting is kept");
        let feed = music_feed(load_releases(&db), 2000, 2025);
        let ids: Vec<&str> = feed.iter().map(|f| f.release.id.as_str()).collect();
        assert_eq!(ids, ["fresh", "hot", "old"]);
        assert!(feed[0].is_new && !feed[1].is_new && !feed[2].is_new);
        assert_eq!(feed[0].release_type, "Single");
    }

    #[test]
    fn feed_keeps_to_followed_artists_when_there_are_some() {
        let db = super::super::test_db();
        let scan = vec![
            ("UCf".to_string(), "Followed".to_string(), true, vec![rel("f1", 2026)]),
            ("UCt".to_string(), "Top".to_string(), false, vec![rel("t1", 2026)]),
        ];
        record_scan(&db, &scan, &HashSet::new(), 10, 2025);
        let feed = music_feed(load_releases(&db), 0, 2025);
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].release.artist, "Followed");
        // New Release Mix takes both, one per artist first.
        let radar = radar_pick(load_releases(&db), 10, 10);
        assert_eq!(radar.len(), 2);
    }

    #[test]
    fn seen_cut_off() {
        let db = super::super::test_db();
        let scan = vec![("UCa".to_string(), "A".to_string(), true, vec![rel("x", 2026)])];
        record_scan(&db, &scan, &HashSet::new(), 10, 2025); // first scan: earlier
        let scan = vec![("UCa".to_string(), "A".to_string(), true, vec![rel("y", 2026), rel("x", 2026)])];
        record_scan(&db, &scan, &HashSet::new(), super::super::now_secs(), 2025);
        assert_eq!(whats_new(&db).new_count, 1);
        mark_seen(&db);
        assert_eq!(whats_new(&db).new_count, 0);
    }
}
