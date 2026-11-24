//! Explore surfaces (stream: polish): Charts (`FEmusic_charts`), New releases
//! (`FEmusic_new_releases`) and any browse page that is a stack of titled shelves (a mood/genre
//! category page, `FEmusic_moods_and_genres_category` + params).
//!
//! Shapes captured live on 2026-11-15 (fixtures `tests/fixtures/explore_*.json`):
//! - Charts: a country picker (`musicSortFilterButtonRenderer` → `musicMultiSelectMenuRenderer`),
//!   then carousels: "Video charts" (chart playlists: Trending 20, Daily Top Music Videos, Top 100
//!   Music Videos, Top 100 Live Performances), "Genres" (US-style countries only: Top 50 <genre>
//!   playlists), "Top artists" (ranked list rows with a trend arrow) and "Weekly top podcast shows"
//!   (ranked list rows linking `MPSP…`). YouTube no longer ships a "Top songs" shelf; the "Top 100
//!   Music Videos" / "Daily Top" chart playlists are what the songs chart is now.
//!   The country is chosen by sending `formData.selectedValues = ["US"]` (`ZZ` = Global).
//! - New releases: a "New Release Mix" playlist card in a `gridRenderer`, then "Albums & singles"
//!   (More → `FEmusic_new_releases_albums`) and "Music videos" (More → `FEmusic_new_releases_videos`).
//! - A mood/genre category page: a `musicHeaderRenderer` title, then plain carousel shelves.

use serde::Serialize;
use serde_json::Value;

use crate::clients::YouTubeClient;
use crate::models::browse::{
    list_item_to_browse_item, parse_carousel_item, parse_home, BrowseItem, Section,
};
use crate::models::context::Context;
use crate::models::metadata::{find_all, runs_text};
use crate::transport::{Error, InnerTube};

/// A country the charts can be shown for. `code` is ISO 3166 alpha-2, or `ZZ` for Global.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChartCountry {
    pub code: String,
    pub name: String,
}

/// One chart row: the card plus, on ranked shelves (top artists, top podcasts), its position and
/// how it moved since the last chart.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartEntry {
    #[serde(flatten)]
    pub item: BrowseItem,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    /// `up` | `down` | `neutral` | `new`, from the rank column's arrow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trend: Option<&'static str>,
}

/// A titled charts shelf. `kind` is language independent:
/// `video_charts` | `genres` | `top_artists` | `top_podcasts` | `other`.
#[derive(Debug, Clone, Serialize)]
pub struct ChartSection {
    pub kind: &'static str,
    pub title: String,
    pub items: Vec<ChartEntry>,
}

/// The chart playlists Spotify-style Browse leads with, picked out of the shelves.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartHighlights {
    /// The songs chart: "Top 100 Songs" when YouTube has one, else "Top 100 Music Videos".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_songs: Option<BrowseItem>,
    /// "Daily Top Music Videos" (or the weekly top 100 when there is no daily one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_videos: Option<BrowseItem>,
    /// "Trending 20".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trending: Option<BrowseItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartsPage {
    /// The country the chart is for (the picker's current value).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<ChartCountry>,
    /// Every country YouTube offers, Global first, in YouTube's order.
    pub countries: Vec<ChartCountry>,
    pub highlights: ChartHighlights,
    pub sections: Vec<ChartSection>,
}

/// New releases: YouTube's mix of the week's releases plus the albums/singles and videos shelves.
/// `sections[].kind` is `albums` or `videos` (from the shelf's "More" target); `moreBrowseId`
/// opens the full grid through `get_browse_grid`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewReleasesPage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix: Option<BrowseItem>,
    pub sections: Vec<Section>,
}

/// A browse page made of titled shelves (a genre/mood category page).
#[derive(Debug, Clone, Serialize)]
pub struct SectionsPage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub sections: Vec<Section>,
}

impl InnerTube {
    /// `FEmusic_charts`, for `country` (ISO alpha-2, `ZZ` = Global) or YouTube's guess from the
    /// IP when `None`. Anonymous: charts are the same for everyone.
    pub async fn charts(
        &self,
        client: &YouTubeClient,
        country: Option<&str>,
    ) -> Result<ChartsPage, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct FormData {
            selected_values: Vec<String>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            context: Context,
            browse_id: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            form_data: Option<FormData>,
        }
        let body = Body {
            context: self.context_anonymous(client),
            browse_id: "FEmusic_charts",
            form_data: country
                .filter(|c| !c.is_empty())
                .map(|c| FormData { selected_values: vec![c.to_ascii_uppercase()] }),
        };
        let value = self.post("browse", client, &body, false).await?;
        let mut page = parse_charts(&value);
        let blocked = self.blocked();
        for s in &mut page.sections {
            let mut cards: Vec<BrowseItem> = s.items.iter().map(|e| e.item.clone()).collect();
            crate::blocklist::retain_cards(&mut cards, &blocked);
            let keep: std::collections::HashSet<String> = cards.into_iter().map(|c| c.id).collect();
            s.items.retain(|e| keep.contains(&e.item.id));
        }
        page.sections.retain(|s| !s.items.is_empty());
        Ok(page)
    }

    /// `FEmusic_new_releases`. Anonymous.
    pub async fn new_releases(&self, client: &YouTubeClient) -> Result<NewReleasesPage, Error> {
        let value = self.browse_sections_raw(client, "FEmusic_new_releases", None).await?;
        let mut page = parse_new_releases(&value);
        self.filter_sections(&mut page.sections);
        Ok(page)
    }

    /// Any browse page made of carousel shelves, kept as shelves (a genre/mood category page:
    /// `FEmusic_moods_and_genres_category` + the tile's params). `browse_grid` flattens the same
    /// page into one grid; this keeps Spotify's shelf-per-row layout.
    pub async fn browse_sections(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
        params: Option<&str>,
    ) -> Result<SectionsPage, Error> {
        let value = self.browse_sections_raw(client, browse_id, params).await?;
        let mut page = parse_sections_page(&value);
        self.filter_sections(&mut page.sections);
        Ok(page)
    }

    async fn browse_sections_raw(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
        params: Option<&str>,
    ) -> Result<Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            context: Context,
            browse_id: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            params: Option<String>,
        }
        let body = Body {
            context: self.context_anonymous(client),
            browse_id: browse_id.to_owned(),
            params: params.map(str::to_owned),
        };
        self.post("browse", client, &body, false).await
    }

    fn filter_sections(&self, sections: &mut Vec<Section>) {
        let hide = self.hide_videos();
        let blocked = self.blocked();
        for s in sections.iter_mut() {
            if hide {
                s.items.retain(|i| !i.is_video);
            }
            crate::blocklist::retain_cards(&mut s.items, &blocked);
            let mut seen = std::collections::HashSet::new();
            s.items.retain(|i| seen.insert(i.id.clone()));
        }
        sections.retain(|s| !s.items.is_empty());
    }
}

// ── parsers (pure, fixture-tested) ──────────────────────────────────────────────────────────────

/// The 2-letter code inside a country option's `formItemEntityKey`: a base64 protobuf whose
/// string field reads `explore_charts_country_menu_<digits><CC>`.
fn country_code(entity_key: &str) -> Option<String> {
    use base64::Engine;
    let raw = urlencoding::decode(entity_key).ok()?;
    let raw = raw.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(raw))
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let at = text.find("country_menu_")? + "country_menu_".len();
    let rest: String = text[at..].chars().skip_while(|c| c.is_ascii_digit()).take(2).collect();
    (rest.len() == 2 && rest.chars().all(|c| c.is_ascii_uppercase())).then_some(rest)
}

fn parse_countries(root: &Value) -> (Option<ChartCountry>, Vec<ChartCountry>) {
    let Some(button) = find_all(root, "musicSortFilterButtonRenderer").into_iter().next() else {
        return (None, Vec::new());
    };
    let current_name = runs_text(button.get("title"));
    let mut countries: Vec<ChartCountry> = Vec::new();
    let mut current = None;
    for opt in find_all(button, "musicMultiSelectMenuItemRenderer") {
        let (Some(name), Some(code)) = (
            runs_text(opt.get("title")),
            opt.get("formItemEntityKey").and_then(Value::as_str).and_then(country_code),
        ) else {
            continue;
        };
        let c = ChartCountry { code, name };
        // The selected option has no `selectedCommand` (nothing to switch to) and is also repeated
        // at the top of the menu, above the divider.
        if opt.get("selectedCommand").is_none() && current.is_none() {
            current = Some(c.clone());
        }
        if !countries.iter().any(|x| x.code == c.code) {
            countries.push(c);
        }
    }
    if current.is_none() {
        current = current_name.and_then(|n| countries.iter().find(|c| c.name == n).cloned());
    }
    // Global first, then YouTube's (alphabetical) order.
    if let Some(i) = countries.iter().position(|c| c.code == "ZZ") {
        let g = countries.remove(i);
        countries.insert(0, g);
    }
    (current, countries)
}

fn rank_of(row: &Value) -> (Option<u32>, Option<&'static str>) {
    let Some(col) =
        row.get("customIndexColumn").and_then(|c| c.get("musicCustomIndexColumnRenderer"))
    else {
        return (None, None);
    };
    let rank = runs_text(col.get("text")).and_then(|t| t.trim().parse().ok());
    let trend = col.get("icon").and_then(|i| i.get("iconType")).and_then(Value::as_str).map(|i| {
        if i.contains("UP") {
            "up"
        } else if i.contains("DOWN") {
            "down"
        } else if i.contains("NEW") {
            "new"
        } else {
            "neutral"
        }
    });
    (rank, trend)
}

fn chart_entry(node: &Value) -> Option<ChartEntry> {
    if let Some(row) = node.get("musicResponsiveListItemRenderer") {
        let item = list_item_to_browse_item(row)?;
        let (rank, trend) = rank_of(row);
        return Some(ChartEntry { item, rank, trend });
    }
    let item = parse_carousel_item(node)?;
    Some(ChartEntry { item, rank: None, trend: None })
}

pub fn parse_charts(root: &Value) -> ChartsPage {
    let (country, countries) = parse_countries(root);
    let mut sections = Vec::new();
    let mut playlist_shelves = 0;
    for shelf in find_all(root, "musicCarouselShelfRenderer") {
        let title = find_all(shelf, "musicCarouselShelfBasicHeaderRenderer")
            .into_iter()
            .find_map(|h| runs_text(h.get("title")))
            .unwrap_or_default();
        let items: Vec<ChartEntry> = shelf
            .get("contents")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(chart_entry).collect())
            .unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        let all = |k: &str| items.iter().all(|e| e.item.kind == k);
        let kind = if all("artist") {
            "top_artists"
        } else if all("podcast") {
            "top_podcasts"
        } else if all("playlist") {
            playlist_shelves += 1;
            if playlist_shelves == 1 {
                "video_charts"
            } else {
                "genres"
            }
        } else {
            "other"
        };
        sections.push(ChartSection { kind, title, items });
    }
    let highlights = highlights(&sections);
    ChartsPage { country, countries, highlights, sections }
}

fn highlights(sections: &[ChartSection]) -> ChartHighlights {
    let charts: Vec<&BrowseItem> = sections
        .iter()
        .filter(|s| s.kind == "video_charts")
        .flat_map(|s| s.items.iter().map(|e| &e.item))
        .collect();
    let find = |pred: &dyn Fn(&str) -> bool| {
        charts.iter().find(|i| pred(&i.title.to_lowercase())).map(|i| (*i).clone())
    };
    ChartHighlights {
        top_songs: find(&|t| t.contains("top") && t.contains("songs"))
            .or_else(|| find(&|t| t.contains("top 100 music videos"))),
        top_videos: find(&|t| t.contains("daily top"))
            .or_else(|| find(&|t| t.contains("top 100 music videos"))),
        trending: find(&|t| t.contains("trending")),
    }
}

fn page_title(root: &Value) -> Option<String> {
    let header = root.get("header")?;
    find_all(header, "musicHeaderRenderer").into_iter().find_map(|h| runs_text(h.get("title")))
}

pub fn parse_sections_page(root: &Value) -> SectionsPage {
    let sections = parse_home(root).sections;
    SectionsPage { title: page_title(root), sections }
}

pub fn parse_new_releases(root: &Value) -> NewReleasesPage {
    let mix = find_all(root, "gridRenderer")
        .into_iter()
        .filter_map(|g| g.get("items")?.as_array())
        .flatten()
        .find_map(parse_carousel_item);
    let mut sections = parse_home(root).sections;
    for s in &mut sections {
        s.kind = match s.more_browse_id.as_deref() {
            Some("FEmusic_new_releases_albums") => Some("albums"),
            Some("FEmusic_new_releases_videos") => Some("videos"),
            _ if s.items.iter().all(|i| i.kind == "album") => Some("albums"),
            _ if s.items.iter().any(|i| i.is_video) => Some("videos"),
            _ => None,
        };
    }
    NewReleasesPage { mix, sections }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn country_code_decodes_the_entity_key() {
        assert_eq!(
            country_code("EidleHBsb3JlX2NoYXJ0c19jb3VudHJ5X21lbnVfMzE2NzY2NTY3QVIgkQEoAQ%3D%3D")
                .as_deref(),
            Some("AR")
        );
        assert_eq!(
            country_code("EidleHBsb3JlX2NoYXJ0c19jb3VudHJ5X21lbnVfMzE2NzY2NTY3WlogkQEoAQ%3D%3D")
                .as_deref(),
            Some("ZZ")
        );
        assert_eq!(country_code("garbage!"), None);
    }

    #[test]
    fn us_charts_parse_into_kinded_shelves() {
        let page = parse_charts(&fixture("explore_charts_us.json"));
        assert_eq!(
            page.country,
            Some(ChartCountry { code: "US".into(), name: "United States".into() })
        );
        assert!(page.countries.len() > 50, "{}", page.countries.len());
        assert_eq!(page.countries[0].code, "ZZ");
        assert!(page.countries.iter().any(|c| c.code == "DE" && c.name == "Germany"));
        let kinds: Vec<&str> = page.sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, ["video_charts", "genres", "top_artists", "top_podcasts"]);

        let artists = &page.sections[2];
        assert_eq!(artists.items[0].rank, Some(1));
        assert!(artists.items[0].trend.is_some());
        assert!(artists.items.iter().all(|e| e.item.id.starts_with("UC")));
        assert_eq!(artists.items[1].item.title, "Drake");

        let pods = &page.sections[3];
        assert!(pods.items.iter().all(|e| e.item.id.starts_with("MPSP")));
        assert_eq!(pods.items[0].rank, Some(1));

        let charts = &page.sections[0];
        assert!(charts.items.iter().all(|e| e.item.id.starts_with("VL")));
        let h = &page.highlights;
        assert!(h.trending.as_ref().unwrap().title.starts_with("Trending 20"));
        assert!(h.top_videos.as_ref().unwrap().title.starts_with("Daily Top Music Videos"));
        assert!(h.top_songs.as_ref().unwrap().title.starts_with("Top 100 Music Videos"));
    }

    #[test]
    fn global_charts_have_no_genre_shelf() {
        let page = parse_charts(&fixture("explore_charts_global.json"));
        assert_eq!(page.country.as_ref().map(|c| c.code.as_str()), Some("ZZ"));
        let kinds: Vec<&str> = page.sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, ["video_charts", "top_artists"]);
        assert!(page.highlights.trending.is_none());
        assert!(page.highlights.top_songs.is_some());
    }

    #[test]
    fn new_releases_parse_mix_and_shelves() {
        let page = parse_new_releases(&fixture("explore_new_releases.json"));
        let mix = page.mix.unwrap();
        assert_eq!(mix.kind, "playlist");
        assert_eq!(mix.title, "New Release Mix");
        assert_eq!(page.sections.len(), 2);
        assert_eq!(page.sections[0].kind, Some("albums"));
        assert_eq!(page.sections[0].more_browse_id.as_deref(), Some("FEmusic_new_releases_albums"));
        assert!(page.sections[0].items.iter().all(|i| i.kind == "album"));
        assert_eq!(page.sections[1].kind, Some("videos"));
        let videos = &page.sections[1].items;
        assert!(videos.iter().filter(|i| i.kind == "song" && i.is_video).count() >= 6);
        // YouTube slips podcast episodes into the videos shelf; they come back kinded.
        assert!(videos.iter().all(|i| i.kind == "song" || i.kind == "episode"));
    }

    #[test]
    fn mood_category_page_keeps_its_shelves() {
        let page = parse_sections_page(&fixture("explore_mood_category.json"));
        assert_eq!(page.title.as_deref(), Some("Chill"));
        assert!(page.sections.len() >= 6);
        assert_eq!(page.sections[0].title, "Coffee shop blends");
        assert!(page.sections.iter().all(|s| !s.items.is_empty()));
    }
}
