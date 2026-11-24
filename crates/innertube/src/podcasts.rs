//! Podcast requests (stream: podcasts). Raw JSON only: the app's `podcasts` module owns the
//! parsing, so this crate gains four thin calls and two filter constants, nothing else.
//!
//! Every call here is anonymous (no cookie, no `onBehalfOfUser`): podcasts need no account, and a
//! search for a show should not land in anyone's YouTube history. The language is pinned to
//! English so the relative dates ("5d ago", "Oct 1") and durations ("1 hr 4 min") on episode rows
//! parse the same whatever the UI language is.

use serde::Serialize;

use crate::clients::YouTubeClient;
use crate::models::context::Context;
use crate::transport::{Error, InnerTube};

/// YouTube Music search filter: podcast shows (ytmusicapi `_get_param2("podcasts")`).
pub const FILTER_PODCASTS: &str = "EgWKAQJQAWoMEA4QChADEAQQCRAF";
/// YouTube Music search filter: podcast episodes (ytmusicapi `_get_param2("episodes")`).
pub const FILTER_EPISODES: &str = "EgWKAQJIAWoMEA4QChADEAQQCRAF";

impl InnerTube {
    fn podcast_context(&self, client: &YouTubeClient) -> Context {
        let mut c = self.context_anonymous(client);
        c.client.hl = "en".into();
        c
    }

    /// `search` with one of [`FILTER_PODCASTS`] / [`FILTER_EPISODES`] (or any params).
    pub async fn podcast_search_json(
        &self,
        client: &YouTubeClient,
        query: &str,
        params: &str,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        struct Body {
            context: Context,
            query: String,
            params: String,
        }
        let body = Body {
            context: self.podcast_context(client),
            query: query.to_owned(),
            params: params.to_owned(),
        };
        self.post("search", client, &body, false).await
    }

    /// `browse` a show page (`MPSP…`) or an episode page (`MPED…`).
    pub async fn podcast_browse_json(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            context: Context,
            browse_id: String,
        }
        let body = Body { context: self.podcast_context(client), browse_id: browse_id.to_owned() };
        self.post("browse", client, &body, false).await
    }

    /// A browse continuation: the next page of a show's episodes, or the "Oldest" sort reload.
    pub async fn podcast_continuation_json(
        &self,
        client: &YouTubeClient,
        token: &str,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        struct Body {
            context: Context,
            continuation: String,
        }
        let body =
            Body { context: self.podcast_context(client), continuation: token.to_owned() };
        self.post("browse", client, &body, false).await
    }

    /// stream: chapters. YouTube's (not YouTube Music's) watch page data for a video, from `next`
    /// with the WEB client: only that shape carries the chapter markers
    /// (`multiMarkersPlayerBarRenderer`) and the full description. YouTube Music's `next` has
    /// neither.
    pub async fn podcast_watch_json(
        &self,
        web_client: &YouTubeClient,
        video_id: &str,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            context: Context,
            video_id: String,
        }
        let body = Body { context: self.podcast_context(web_client), video_id: video_id.to_owned() };
        self.post("next", web_client, &body, false).await
    }
}
