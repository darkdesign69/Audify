//! Filtered search with continuations (stream: search2): one chip of the search page as a full,
//! endlessly scrolling list, the way YouTube Music's own "Songs" / "Albums" / … tabs page.
//!
//! Ported from ytmusicapi 1.12 (sigma67/ytmusicapi, MIT):
//! - the filter params are `parsers/search.py::get_search_params` with `scope=None,
//!   ignore_spelling=False`: `"EgWKAQ" + _get_param2(filter) + "AWoMEA4QChADEAQQCRAF"`, and the
//!   playlist constants spelled out there;
//! - the paging is `continuations.py::get_continuations`: the first answer's
//!   `musicShelfRenderer.continuations[0].nextContinuationData.continuation` is re-sent as
//!   `&ctoken=<t>&continuation=<t>` on the **same** search body, and the answer's
//!   `continuationContents.musicShelfContinuation` holds the next rows and the next token.
//!   ytmusicapi's 2025 form (`continuationItemRenderer.continuationEndpoint.continuationCommand.
//!   token`, sent as `{"continuation": t}` in the body, rows under `onResponseReceivedActions`) is
//!   read as well, so a switch on YouTube's side keeps paging.
//!
//! The rows themselves are parsed by the existing song / card parsers ([`crate::models`]); this
//! module only adds the params, the token and the request.

use serde::Serialize;
use serde_json::Value;

use crate::clients::YouTubeClient;
use crate::models::browse::{self, BrowseItem, SearchResults};
use crate::models::context::Context;
use crate::models::metadata::{self, SongItem};
use crate::transport::{Error, InnerTube};

/// One search chip. `params()` is what ytmusicapi sends for `search(q, filter=<name>)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchFilter {
    Songs,
    Videos,
    Albums,
    Artists,
    /// Featured (YouTube Music's own) and community playlists together, as ytmusicapi's
    /// `filter="playlists"`.
    Playlists,
    CommunityPlaylists,
    FeaturedPlaylists,
    /// User channels (YouTube Music "Profiles").
    Profiles,
    Podcasts,
    Episodes,
}

/// ytmusicapi `_get_param2`.
fn param2(f: SearchFilter) -> &'static str {
    match f {
        SearchFilter::Songs => "II",
        SearchFilter::Videos => "IQ",
        SearchFilter::Albums => "IY",
        SearchFilter::Artists => "Ig",
        SearchFilter::Playlists => "Io",
        SearchFilter::Profiles => "JY",
        SearchFilter::Podcasts => "JQ",
        SearchFilter::Episodes => "JI",
        SearchFilter::CommunityPlaylists => "EA",
        SearchFilter::FeaturedPlaylists => "Dg",
    }
}

impl SearchFilter {
    pub const ALL: [SearchFilter; 10] = [
        SearchFilter::Songs,
        SearchFilter::Videos,
        SearchFilter::Albums,
        SearchFilter::Artists,
        SearchFilter::Playlists,
        SearchFilter::CommunityPlaylists,
        SearchFilter::FeaturedPlaylists,
        SearchFilter::Profiles,
        SearchFilter::Podcasts,
        SearchFilter::Episodes,
    ];

    /// The `params` of the search body (ytmusicapi `get_search_params(filter, None, False)`).
    pub fn params(self) -> String {
        match self {
            // `filter == "playlists"` is spelled out in full there.
            SearchFilter::Playlists => "Eg-KAQwIABAAGAAgACgBMABqChAEEAMQCRAFEAo%3D".to_owned(),
            SearchFilter::CommunityPlaylists | SearchFilter::FeaturedPlaylists => {
                format!("EgeKAQQoA{}BagwQDhAKEAMQBBAJEAU%3D", param2(self))
            }
            _ => format!("EgWKAQ{}AWoMEA4QChADEAQQCRAF", param2(self)),
        }
    }

    /// The chip's name as the UI and ytmusicapi spell it (`"songs"`, `"podcasts"`, …).
    pub fn name(self) -> &'static str {
        match self {
            SearchFilter::Songs => "songs",
            SearchFilter::Videos => "videos",
            SearchFilter::Albums => "albums",
            SearchFilter::Artists => "artists",
            SearchFilter::Playlists => "playlists",
            SearchFilter::CommunityPlaylists => "community_playlists",
            SearchFilter::FeaturedPlaylists => "featured_playlists",
            SearchFilter::Profiles => "profiles",
            SearchFilter::Podcasts => "podcasts",
            SearchFilter::Episodes => "episodes",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == s)
    }

    /// Rows that are tracks (parsed as `SongItem`) rather than cards.
    pub fn is_tracks(self) -> bool {
        matches!(self, SearchFilter::Songs | SearchFilter::Videos)
    }
}

/// ytmusicapi `continuations.CONTINUATION_TOKEN` / `COMMAND_EXECUTOR_COMMANDS` on a row list.
fn token_from_items(items: &[Value]) -> Option<String> {
    let last = items.last()?;
    let cir = last.get("continuationItemRenderer")?;
    let ep = cir.get("continuationEndpoint")?;
    if let Some(t) = ep.pointer("/continuationCommand/token").and_then(Value::as_str) {
        return Some(t.to_owned());
    }
    ep.pointer("/commandExecutorCommand/commands")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|c| c.pointer("/continuationCommand/token").and_then(Value::as_str))
        .map(str::to_owned)
}

/// The shelf a filtered search answer holds its rows in: `musicShelfRenderer` on the first page
/// (the only shelf of a filtered search; an "Did you mean" / top card sits in other renderers),
/// `continuationContents.musicShelfContinuation` on a later one.
fn shelf(v: &Value) -> Option<&Value> {
    if let Some(c) = v.pointer("/continuationContents/musicShelfContinuation") {
        return Some(c);
    }
    let sections = v
        .pointer("/contents/tabbedSearchResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)?;
    sections.iter().rev().find_map(|s| s.get("musicShelfRenderer"))
}

/// The token for the next page of a filtered search answer (first page or a continuation), or
/// `None` at the end. ytmusicapi `get_continuation_params` (the shelf's
/// `continuations[0].nextContinuationData.continuation`), else the 2025 `continuationItemRenderer`.
pub fn next_token(v: &Value) -> Option<String> {
    if let Some(s) = shelf(v) {
        if let Some(t) = s
            .pointer("/continuations/0/nextContinuationData/continuation")
            .and_then(Value::as_str)
        {
            return Some(t.to_owned());
        }
        if let Some(t) = s.get("contents").and_then(Value::as_array).and_then(|a| token_from_items(a)) {
            return Some(t);
        }
    }
    v.pointer("/onResponseReceivedActions/0/appendContinuationItemsAction/continuationItems")
        .and_then(Value::as_array)
        .and_then(|a| token_from_items(a))
}

/// The part of a filtered-search answer that holds this page's rows, so a row parser
/// (`find_all(.., "musicResponsiveListItemRenderer")`) sees only them and never a "Top result"
/// card or a "did you mean" suggestion of the first page.
pub fn page_rows(v: &Value) -> &Value {
    if let Some(s) = shelf(v) {
        return s;
    }
    v.pointer("/onResponseReceivedActions/0/appendContinuationItemsAction/continuationItems")
        .unwrap_or(v)
}

/// YouTube Music's own spelling correction on a search answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellFix {
    /// The corrected query.
    pub query: String,
    /// `true` = "Showing results for <query>": the rows answer the corrected query already.
    /// `false` = "Did you mean <query>": the rows still answer what was typed.
    pub applied: bool,
}

/// `showingResultsForRenderer` / `didYouMeanRenderer`, wherever the answer carries them (the
/// first page's `itemSectionRenderer`, above the shelf). The corrected query is the renderer's
/// `correctedQueryEndpoint.searchEndpoint.query`, else its `correctedQuery` runs joined (the shape
/// Metrolist / InnerTune read; ytmusicapi 1.12 skips these sections).
pub fn spell_fix(v: &Value) -> Option<SpellFix> {
    fn walk<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
        match v {
            Value::Object(m) => m.get(key).or_else(|| {
                // Never inside a row: a row has no correction, and the walk stays cheap.
                m.iter()
                    .filter(|(k, _)| k.as_str() != "musicResponsiveListItemRenderer")
                    .find_map(|(_, x)| walk(x, key))
            }),
            Value::Array(a) => a.iter().find_map(|x| walk(x, key)),
            _ => None,
        }
    }
    let contents = v.get("contents")?;
    for (key, applied) in [("showingResultsForRenderer", true), ("didYouMeanRenderer", false)] {
        let Some(r) = walk(contents, key) else { continue };
        let q = r
            .pointer("/correctedQueryEndpoint/searchEndpoint/query")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                let runs = r.pointer("/correctedQuery/runs")?.as_array()?;
                Some(runs.iter().filter_map(|x| x.get("text")?.as_str()).collect::<String>())
            })?;
        let q = q.trim().to_owned();
        if !q.is_empty() {
            return Some(SpellFix { query: q, applied });
        }
    }
    None
}

/// One page of a chip: its rows, the token for the next page, YouTube's correction if any.
#[derive(Debug, Clone, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub continuation: Option<String>,
    pub spell_fix: Option<SpellFix>,
}

impl InnerTube {
    /// One page of a filtered search, raw. `continuation` = `None` for the first page, else the
    /// [`next_token`] of the previous answer. Anonymous, like every search that records no history
    /// (see `search_raw`): only the search page's first, unfiltered query is ever written to it.
    pub async fn search_page_json(
        &self,
        client: &YouTubeClient,
        query: &str,
        filter: SearchFilter,
        continuation: Option<&str>,
    ) -> Result<Value, Error> {
        #[derive(Serialize)]
        struct Body {
            context: Context,
            query: String,
            params: String,
        }
        let body = Body {
            context: self.context_anonymous(client),
            query: query.to_owned(),
            params: filter.params(),
        };
        let path = match continuation {
            None => "search".to_owned(),
            Some(t) => {
                let enc = urlencoding::encode(t);
                format!("search?ctoken={enc}&continuation={enc}&type=next")
            }
        };
        self.post(&path, client, &body, false).await
    }

    /// A tracks chip (`Songs` / `Videos`) page as rows. "Hide music videos" applies as everywhere.
    pub async fn search_tracks_page(
        &self,
        client: &YouTubeClient,
        query: &str,
        filter: SearchFilter,
        continuation: Option<&str>,
    ) -> Result<Page<SongItem>, Error> {
        if filter == SearchFilter::Videos && self.hide_videos() {
            return Ok(Page::default());
        }
        let v = self.search_page_json(client, query, filter, continuation).await?;
        let mut items = metadata::parse_search(page_rows(&v)).items;
        self.drop_video_songs(&mut items);
        Ok(Page { items, continuation: next_token(&v), spell_fix: spell_fix(&v) })
    }

    /// A card chip (albums, artists, playlists, profiles) page. Profiles keep the user channels
    /// every other surface drops; the other chips drop them as `search_cards` does.
    pub async fn search_cards_page(
        &self,
        client: &YouTubeClient,
        query: &str,
        filter: SearchFilter,
        continuation: Option<&str>,
    ) -> Result<Page<BrowseItem>, Error> {
        let v = self.search_page_json(client, query, filter, continuation).await?;
        let profiles = filter == SearchFilter::Profiles;
        let mut items: Vec<BrowseItem> = metadata::find_all(page_rows(&v), "musicResponsiveListItemRenderer")
            .into_iter()
            .filter(|li| profiles == browse::is_user_channel(li))
            .filter_map(browse::list_item_to_browse_item)
            .collect();
        self.drop_video_cards(&mut items);
        Ok(Page { items, continuation: next_token(&v), spell_fix: spell_fix(&v) })
    }

    /// [`InnerTube::search_all`] plus YouTube's spelling correction, which that drops.
    pub async fn search_all_fixed(
        &self,
        client: &YouTubeClient,
        query: &str,
        record_history: bool,
    ) -> Result<(SearchResults, Option<SpellFix>), Error> {
        let value = self.search_raw(client, query, None, record_history).await?;
        let mut r = browse::parse_search_all(&value);
        self.drop_video_cards(&mut r.top);
        self.drop_video_cards(&mut r.songs);
        Ok((r, spell_fix(&value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_match_ytmusicapi() {
        // ytmusicapi 1.12.3 `get_search_params(f, None, False)`, verbatim.
        assert_eq!(SearchFilter::Songs.params(), "EgWKAQIIAWoMEA4QChADEAQQCRAF");
        assert_eq!(SearchFilter::Videos.params(), "EgWKAQIQAWoMEA4QChADEAQQCRAF");
        assert_eq!(SearchFilter::Albums.params(), "EgWKAQIYAWoMEA4QChADEAQQCRAF");
        assert_eq!(SearchFilter::Artists.params(), "EgWKAQIgAWoMEA4QChADEAQQCRAF");
        assert_eq!(SearchFilter::Profiles.params(), "EgWKAQJYAWoMEA4QChADEAQQCRAF");
        assert_eq!(SearchFilter::Podcasts.params(), crate::podcasts::FILTER_PODCASTS);
        assert_eq!(SearchFilter::Episodes.params(), crate::podcasts::FILTER_EPISODES);
        assert_eq!(SearchFilter::Playlists.params(), "Eg-KAQwIABAAGAAgACgBMABqChAEEAMQCRAFEAo%3D");
        assert_eq!(SearchFilter::CommunityPlaylists.params(), "EgeKAQQoAEABagwQDhAKEAMQBBAJEAU%3D");
        assert_eq!(SearchFilter::FeaturedPlaylists.params(), "EgeKAQQoADgBagwQDhAKEAMQBBAJEAU%3D");
        for f in SearchFilter::ALL {
            assert_eq!(SearchFilter::from_name(f.name()), Some(f));
        }
        assert_eq!(SearchFilter::from_name("nope"), None);
    }

    fn first_page(token: Option<&str>) -> Value {
        let mut shelf = json!({ "contents": [{ "musicResponsiveListItemRenderer": { "id": 1 } }] });
        if let Some(t) = token {
            shelf["continuations"] = json!([{ "nextContinuationData": { "continuation": t } }]);
        }
        json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
            "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [{ "showingResultsForRenderer": {} }] } },
                { "musicShelfRenderer": shelf }
            ] } } } }] } } })
    }

    #[test]
    fn reads_the_first_page_token() {
        assert_eq!(next_token(&first_page(Some("tok1"))).as_deref(), Some("tok1"));
        assert_eq!(next_token(&first_page(None)), None);
        let page = first_page(Some("tok1"));
        let rows = page_rows(&page);
        assert!(rows.get("contents").is_some());
    }

    #[test]
    fn reads_a_continuation_token_and_the_end() {
        let more = json!({ "continuationContents": { "musicShelfContinuation": {
            "contents": [{ "musicResponsiveListItemRenderer": {} }],
            "continuations": [{ "nextContinuationData": { "continuation": "tok2" } }] } } });
        assert_eq!(next_token(&more).as_deref(), Some("tok2"));
        let last = json!({ "continuationContents": { "musicShelfContinuation": { "contents": [] } } });
        assert_eq!(next_token(&last), None);
    }

    #[test]
    fn reads_the_2025_continuation_item_form() {
        let v = json!({ "onResponseReceivedActions": [{ "appendContinuationItemsAction": { "continuationItems": [
            { "musicResponsiveListItemRenderer": {} },
            { "continuationItemRenderer": { "continuationEndpoint": { "continuationCommand": { "token": "tok3" } } } }
        ] } }] });
        assert_eq!(next_token(&v).as_deref(), Some("tok3"));
        let nested = json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
            "sectionListRenderer": { "contents": [{ "musicShelfRenderer": { "contents": [
                { "musicResponsiveListItemRenderer": {} },
                { "continuationItemRenderer": { "continuationEndpoint": { "commandExecutorCommand": { "commands": [
                    { "playlistVotingRefreshPopupCommand": {} },
                    { "continuationCommand": { "token": "tok4", "request": "CONTINUATION_REQUEST_TYPE_BROWSE" } }
                ] } } } }
            ] } }] } } } }] } } });
        assert_eq!(next_token(&nested).as_deref(), Some("tok4"));
    }

    #[test]
    fn reads_youtube_spelling_corrections() {
        let showing = json!({ "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
            "sectionListRenderer": { "contents": [{ "itemSectionRenderer": { "contents": [{ "showingResultsForRenderer": {
                "correctedQuery": { "runs": [{ "text": "bohemian" }, { "text": " rhapsody" }] },
                "correctedQueryEndpoint": { "searchEndpoint": { "query": "bohemian rhapsody", "params": "x" } },
                "originalQuery": { "runs": [{ "text": "bohemain rapsodyy" }] } } }] } }] } } } }] } } });
        assert_eq!(spell_fix(&showing), Some(SpellFix { query: "bohemian rhapsody".into(), applied: true }));
        let dym = json!({ "contents": { "sectionListRenderer": { "contents": [{ "itemSectionRenderer": { "contents": [
            { "didYouMeanRenderer": { "correctedQuery": { "runs": [{ "text": "taylor" }, { "text": " swift" }] } } }] } }] } } });
        assert_eq!(spell_fix(&dym), Some(SpellFix { query: "taylor swift".into(), applied: false }));
        assert_eq!(spell_fix(&first_page(None)), None);
    }
}
