//! Browse pages both apps open from Search: Charts, New releases and a genre/mood page as
//! titled shelves. The desktop's `polish::commands` and the iOS bridge (`invoke.rs`) both call
//! these, so the hour cache and the remembered chart country exist once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use innertube::explore::{ChartsPage, NewReleasesPage, SectionsPage};

use crate::db::Db;
use crate::state::AppState;

const TTL: Duration = Duration::from_secs(60 * 60);
/// The chart country last picked (ISO alpha-2, `ZZ` = Global).
pub const CHARTS_COUNTRY: &str = "charts_country";

fn client(st: &AppState) -> Result<&innertube::YouTubeClient, String> {
    st.clients.get(innertube::METADATA_CLIENT).ok_or_else(|| "metadata client missing".into())
}

/// A tiny per-key hour cache: charts and new releases change daily at most.
struct Cache<T>(Mutex<HashMap<String, (Instant, T)>>);

impl<T: Clone> Cache<T> {
    fn get(&self, k: &str) -> Option<T> {
        let m = self.0.lock().unwrap();
        m.get(k).filter(|(at, _)| at.elapsed() < TTL).map(|(_, v)| v.clone())
    }
    fn put(&self, k: &str, v: &T) {
        self.0.lock().unwrap().insert(k.to_owned(), (Instant::now(), v.clone()));
    }
}

fn charts_cache() -> &'static Cache<ChartsPage> {
    static C: OnceLock<Cache<ChartsPage>> = OnceLock::new();
    C.get_or_init(|| Cache(Mutex::new(HashMap::new())))
}

fn releases_cache() -> &'static Cache<NewReleasesPage> {
    static C: OnceLock<Cache<NewReleasesPage>> = OnceLock::new();
    C.get_or_init(|| Cache(Mutex::new(HashMap::new())))
}

/// The country a charts request is for: a valid pick (remembered from now on), else the last one
/// picked, else `None` (YouTube guesses from the IP).
pub fn chart_country(db: &Db, picked: Option<String>) -> Option<String> {
    match picked.map(|c| c.trim().to_ascii_uppercase()).filter(|c| c.len() == 2) {
        Some(c) => {
            db.set_setting(CHARTS_COUNTRY, &c);
            Some(c)
        }
        None => db.get_setting(CHARTS_COUNTRY),
    }
}

/// Charts for `country`: `{country, countries[], highlights {topSongs, topVideos, trending},
/// sections[]}` where `sections[].kind` is `video_charts | genres | top_artists | top_podcasts |
/// other` and each item is a card plus `rank` / `trend` on ranked shelves.
pub async fn charts(st: &Arc<AppState>, country: Option<String>) -> Result<ChartsPage, String> {
    let country = chart_country(&st.db, country);
    let key = country.clone().unwrap_or_default();
    if let Some(hit) = charts_cache().get(&key) {
        return Ok(hit);
    }
    let page = st.it.charts(client(st)?, country.as_deref()).await.map_err(|e| e.to_string())?;
    charts_cache().put(&key, &page);
    Ok(page)
}

/// New releases: `{mix?, sections[]}`; `sections[].kind` is `albums` | `videos`, and
/// `moreBrowseId` opens the full grid with `get_browse_grid`.
pub async fn new_releases(st: &Arc<AppState>) -> Result<NewReleasesPage, String> {
    if let Some(hit) = releases_cache().get("") {
        return Ok(hit);
    }
    let page = st.it.new_releases(client(st)?).await.map_err(|e| e.to_string())?;
    releases_cache().put("", &page);
    Ok(page)
}

/// A browse page as titled shelves: a genre/mood page is
/// `browse_sections("FEmusic_moods_and_genres_category", tile.params)`.
pub async fn browse_sections(
    st: &Arc<AppState>,
    browse_id: String,
    params: Option<String>,
) -> Result<SectionsPage, String> {
    st.it.browse_sections(client(st)?, &browse_id, params.as_deref()).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_country_is_remembered_and_a_bad_one_ignored() {
        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        assert_eq!(chart_country(&db, None), None);
        assert_eq!(chart_country(&db, Some(" de ".into())).as_deref(), Some("DE"));
        assert_eq!(chart_country(&db, None).as_deref(), Some("DE"));
        assert_eq!(chart_country(&db, Some("Germany".into())).as_deref(), Some("DE"));
        assert_eq!(chart_country(&db, Some("zz".into())).as_deref(), Some("ZZ"));
    }
}
