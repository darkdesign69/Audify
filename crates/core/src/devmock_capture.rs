//! ── stream: devmock ── Captures REAL backend responses as JSON fixtures for the UI's browser-only
//! dev mock (`ui/src/lib/devmock/`, docs/streams/devmock.md). Calls the same functions the Tauri
//! commands call and writes what they return, so the JSON shape is exactly the command's. Live
//! network, so never in the default run:
//!   cd src-tauri && LIBRARY_PATH=/opt/homebrew/lib cargo test --lib devmock_capture -- --ignored --nocapture
#![cfg(test)]

use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

const MOODS_CATEGORY_ID: &str = "FEmusic_moods_and_genres_category";

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/devmock/fixtures");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write<T: Serialize>(name: &str, v: &T) {
    let path = out_dir().join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string(v).unwrap()).unwrap();
    println!("wrote {}", path.display());
}

/// Run a capture step; a failure is logged and skipped, so one dead endpoint doesn't lose the rest.
macro_rules! cap {
    ($name:expr, $fut:expr) => {
        match $fut.await {
            Ok(v) => {
                write($name, &v);
                Some(v)
            }
            Err(e) => {
                eprintln!("FAILED {}: {e}", $name);
                None
            }
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn capture_fixtures() {
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();

    // Home + a chip page.
    // Signed out, the first page is only a couple of shelves: follow the continuation a few
    // times and fold the pages together, the way the Home page's infinite scroll would.
    if let Ok(mut home) = it.home(c, None).await {
        let mut token = home.continuation.take();
        for _ in 0..5 {
            let Some(t) = token.take() else { break };
            match it.home_continuation(c, &t).await {
                Ok(more) => {
                    home.sections.extend(more.sections);
                    token = more.continuation;
                }
                Err(e) => eprintln!("FAILED home more: {e}"),
            }
        }
        write("home", &home);
        if let Some(chip) = home.chips.first() {
            cap!("home_chip", it.home(c, Some(&chip.params)));
        }
    }

    // Search "daft punk", every filter, + typeahead.
    let dp = cap!("search_all_daft_punk", it.search_all(c, "daft punk", false));
    cap!("search_songs_daft_punk", async {
        it.search_songs(c, "daft punk", false).await.map(|r| r.items)
    });
    cap!("search_videos_daft_punk", async {
        it.search_videos(c, "daft punk").await.map(|r| r.items)
    });
    for cat in ["albums", "artists", "playlists"] {
        cap!(&format!("search_cards_daft_punk_{cat}"), it.search_cards(c, "daft punk", cat));
    }
    cap!("search_suggestions_daft_p", it.search_suggestions(c, "daft p"));
    let rh = cap!("search_all_radiohead", it.search_all(c, "radiohead", false));

    // Artists.
    // By exact name: the unfiltered search's artist rows lead with whoever YouTube relates.
    let mut artist_ids = Vec::new();
    for (slug, name, res) in [("daft_punk", "Daft Punk", &dp), ("radiohead", "Radiohead", &rh)] {
        let from_all = res.as_ref().and_then(|r| {
            r.top.iter().chain(r.artists.iter()).find(|a| a.kind == "artist" && a.title.eq_ignore_ascii_case(name)).map(|a| a.id.clone())
        });
        let id = match from_all {
            Some(id) => Some(id),
            None => it.search_cards(c, name, "artists").await.ok().and_then(|v| {
                v.into_iter().find(|a| a.title.eq_ignore_ascii_case(name)).map(|a| a.id)
            }),
        };
        match id {
            Some(id) => artist_ids.push((slug, id)),
            None => eprintln!("FAILED artist id for {name}"),
        }
    }
    write("artist_index", &artist_ids.iter().map(|(s, id)| (id.clone(), json!(s))).collect::<serde_json::Map<_, _>>());
    for (slug, id) in &artist_ids {
        if let Some(page) = cap!(&format!("artist_{slug}"), it.artist(c, id)) {
            if let Some(songs_id) = page.top_songs_id.as_deref() {
                cap!(&format!("artist_appears_on_{slug}"), it.artist_appears_on(c, id, songs_id));
            }
        }
    }

    // Albums: find each by search, keep the browseId → slug map for the mock.
    let mut albums = serde_json::Map::new();
    for (slug, q) in [
        ("discovery", "daft punk discovery"),
        ("ram", "daft punk random access memories"),
        ("ok_computer", "radiohead ok computer"),
    ] {
        let Ok(cards) = it.search_cards(c, q, "albums").await else { continue };
        let Some(first) = cards.first() else { continue };
        if cap!(&format!("album_{slug}"), it.album(c, &first.id)).is_some() {
            albums.insert(first.id.clone(), json!(slug));
        }
    }
    write("album_index", &albums);

    // Explore: charts, new releases, moods grid, one genre page + its grid.
    let charts = cap!("charts", it.charts(c, Some("US")));
    cap!("new_releases", it.new_releases(c));
    if let Some(moods) = cap!("moods", it.moods(c)) {
        let tiles: Vec<_> = moods.iter().flat_map(|s| s.items.iter()).collect();
        if let Some(t) = tiles.first() {
            write("genre_meta", &json!({ "title": t.title, "params": t.params }));
            cap!("genre_sections", it.browse_sections(c, MOODS_CATEGORY_ID, Some(&t.params)));
            cap!("genre_grid", it.browse_grid(c, MOODS_CATEGORY_ID, Some(&t.params)));
        }
        // Mood art the way `get_mood_art` builds it: the first playlist of each category.
        let mut art = serde_json::Map::new();
        for t in tiles.iter().take(24) {
            if let Ok(grid) = it.browse_grid(c, MOODS_CATEGORY_ID, Some(&t.params)).await {
                if let Some(thumb) = grid.iter().find_map(|i| i.thumbnail.clone()) {
                    art.insert(t.params.clone(), json!(thumb));
                }
            }
        }
        write("mood_art", &art);
    }

    // Playlists: the chart's top songs + the first playlist search hit.
    let mut playlists = serde_json::Map::new();
    let mut pl_ids: Vec<(String, String)> = Vec::new();
    if let Some(top) = charts.as_ref().and_then(|ch| ch.highlights.top_songs.as_ref()) {
        pl_ids.push(("chart_top".into(), top.id.clone()));
    }
    if let Ok(cards) = it.search_cards(c, "daft punk", "playlists").await {
        if let Some(p) = cards.first() {
            pl_ids.push(("search_daft_punk".into(), p.id.clone()));
        }
    }
    if let Ok(cards) = it.search_cards(c, "90s rock anthems", "playlists").await {
        if let Some(p) = cards.first() {
            pl_ids.push(("rock".into(), p.id.clone()));
        }
    }
    for (slug, id) in &pl_ids {
        if cap!(&format!("playlist_{slug}"), it.playlist(c, id, None)).is_some() {
            playlists.insert(id.clone(), json!(slug));
        }
    }
    write("playlist_index", &playlists);

    // Podcasts (Huberman Lab): YouTube search + show page + an episode page.
    use crate::podcasts::youtube;
    let shows = youtube::search_shows(&it, c, "huberman lab").await;
    let episodes = youtube::search_episodes(&it, c, "huberman lab").await;
    write(
        "podcast_search_huberman",
        &json!({
            "shows": shows.as_ref().ok(),
            "episodes": episodes.as_ref().ok(),
        }),
    );
    if let Some(show) = shows.as_ref().ok().and_then(|s| s.first()) {
        match youtube::fetch_show(&it, c, &show.id, crate::podcasts::Sort::Newest).await {
            Ok(p) => {
                let first_ep = p.episodes.first().map(|e| e.id.clone());
                write(
                    "podcast_show_huberman",
                    &crate::podcasts::ShowPage {
                        show: p.show,
                        episodes: p.episodes,
                        continuation: None,
                        sort: crate::podcasts::Sort::Newest,
                    },
                );
                if let Some(id) = first_ep {
                    cap!("podcast_episode_huberman", youtube::fetch_episode(&it, c, &id));
                }
            }
            Err(e) => eprintln!("FAILED podcast show: {e}"),
        }
    }

    // Lyrics: synced LRCLIB, parsed by the app's own LRC parser.
    let lrc: Value = crate::http::client()
        .get("https://lrclib.net/api/get?artist_name=Daft%20Punk&track_name=One%20More%20Time&album_name=Discovery&duration=320")
        .header("User-Agent", "Encore devmock capture")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap_or(Value::Null);
    if let Some(synced) = lrc.get("syncedLyrics").and_then(Value::as_str) {
        let lines = crate::lyrics::parse_file(synced).unwrap_or_default();
        write(
            "lyrics_one_more_time",
            &json!({
                "source": "LRCLIB", "provider": "lrclib", "synced": true, "instrumental": false,
                "lines": lines, "pinned": false, "offset_ms": 0
            }),
        );
    } else {
        eprintln!("FAILED lyrics: {lrc}");
    }
}
