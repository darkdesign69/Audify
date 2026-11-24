//! Home's Podcasts chip (stream: home-tiles-pods): Spotify fills it with suggestions, not only
//! your own shows. Shelves, in order (each left out when empty):
//!
//! | id | shelf | source |
//! |---|---|---|
//! | `shortcuts` | episodes in progress, then new ones from your shows (8 tiles) | local |
//! | `pod_new` | New episodes (your shows) | local |
//! | `pod_for_you` | Episodes for you: the latest episode of each suggested show | Apple |
//! | `pod_shows` | Your shows | local |
//! | `pod_like` | Shows you might like: charts of the genres you listen to | Apple |
//! | `pod_because` | Because you listened to <show>: that show's genre chart | Apple |
//! | `pod_top` | Top podcasts | Apple |
//! | `pod_categories` | Podcast categories (Browse-style tiles → [`podcast_category`]) | Apple |
//!
//! Apple's charts are keyless: the marketing-tools feed for the overall top list and the iTunes RSS
//! feed for a genre's. One iTunes lookup turns chart ids into RSS shows (feed URL = show id, the
//! same id search uses) plus each show's newest episode. Episodes still prefer an ad-free YouTube
//! twin at play time (`polish::podmatch`). YouTube Music's own podcasts page
//! (`FEmusic_non_music_audio`) answers 404 to anonymous clients, so it is not a source.
//!
//! The suggested shelves are cached 6 h (`smart_cache`); a stale copy stands in when Apple is slow
//! or offline, and the categories shelf is built from a fixed list, so the chip is never empty.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::home::{episode_item, item, shelf, Shelf, ShelfItem};
use crate::db::Db;
use crate::podcasts::{itunes, search::norm, store, Episode, Show};
use crate::state::AppState;

const CACHE_KEY: &str = "podcast_home";
const CACHE_TTL: i64 = 6 * 3600;
/// How long the chip waits for Apple before painting the cached (or fixed) shelves.
const BUDGET: std::time::Duration = std::time::Duration::from_secs(8);
const SHELF_LEN: usize = 12;

/// Apple's podcast genres shown as category tiles: (genre id, name, tile colour).
pub const CATEGORIES: [(&str, &str, &str); 16] = [
    ("1303", "Comedy", "#be123c"),
    ("1489", "News", "#15803d"),
    ("1488", "True Crime", "#9d2bb0"),
    ("1324", "Society & Culture", "#0e6ba8"),
    ("1545", "Sports", "#a16207"),
    ("1321", "Business", "#3f7d20"),
    ("1512", "Health & Fitness", "#b0279f"),
    ("1487", "History", "#d6246e"),
    ("1533", "Science", "#0b7a75"),
    ("1318", "Technology", "#6a2fd8"),
    ("1309", "TV & Film", "#1f4fa8"),
    ("1304", "Education", "#c2410c"),
    ("1301", "Arts", "#be123c"),
    ("1310", "Music", "#15803d"),
    ("1305", "Kids & Family", "#9d2bb0"),
    ("1483", "Fiction", "#0e6ba8"),
];

/// The Podcasts chip's shelves. Local shelves are always fresh; the suggested ones come from the
/// cache, or are rebuilt (within [`BUDGET`]; a slower build finishes in the background and fills
/// the cache for next time).
pub async fn podcast_home(state: &AppState) -> Vec<Shelf> {
    let db = &state.db;
    let remote = match super::cache_get::<Vec<Shelf>>(db, CACHE_KEY, CACHE_TTL) {
        Some(r) => r,
        None => {
            use std::sync::atomic::{AtomicBool, Ordering};
            let late = std::sync::Arc::new(AtomicBool::new(false));
            let flag = late.clone();
            let task = crate::rt::spawn(async move {
                let st = crate::state::global()?;
                let r = suggested(&st.db).await;
                if r.iter().any(|s| s.id != "pod_categories") {
                    super::cache_put(&st.db, CACHE_KEY, &r);
                    // The chip stopped waiting: tell the UIs to reload it.
                    if flag.load(Ordering::SeqCst) {
                        super::home::emit_changed(&*st.host, "podcasts", None);
                    }
                }
                Some(r)
            });
            match tokio::time::timeout(BUDGET, task).await {
                Ok(Ok(Some(r))) if r.iter().any(|s| s.id != "pod_categories") => r,
                _ => {
                    late.store(true, Ordering::SeqCst);
                    super::cache_get(db, CACHE_KEY, i64::MAX).unwrap_or_else(|| vec![categories_shelf(&HashMap::new())])
                }
            }
        }
    };
    assemble(local(db), remote)
}

/// Local shelves and the suggested ones in Spotify's order, suggestions minus your own shows.
fn assemble(local: Local, remote: Vec<Shelf>) -> Vec<Shelf> {
    let followed: HashSet<String> = local.follows.iter().map(|s| norm(&s.title)).collect();
    let mut remote: HashMap<String, Shelf> = remote.into_iter().map(|s| (s.id.clone(), s)).collect();
    let mut take = |id: &str| {
        remote.remove(id).map(|mut s| {
            // Your own shows (and their episodes) aren't suggestions.
            s.items.retain(|i| match i.kind.as_str() {
                "podcast" => !followed.contains(&norm(&i.title)),
                "episode" => !followed.contains(&norm(i.subtitle.as_deref().unwrap_or_default())),
                _ => true,
            });
            s
        })
    };
    let shows: Vec<ShelfItem> = local.follows.iter().map(show_item).collect();
    let out = [
        Some(shelf("shortcuts", "", "shortcuts", "local", local.shortcuts)),
        Some(shelf("pod_new", "New episodes", "cards", "local", local.new_eps)),
        take("pod_for_you"),
        Some(shelf("pod_shows", "Your shows", "cards", "local", shows)),
        take("pod_like"),
        take("pod_because"),
        take("pod_top"),
        take("pod_categories"),
    ];
    out.into_iter().flatten().filter(|s| !s.items.is_empty()).collect()
}

struct Local {
    shortcuts: Vec<ShelfItem>,
    new_eps: Vec<ShelfItem>,
    follows: Vec<Show>,
}

/// New episodes of your shows count this far back.
const NEW_WINDOW: i64 = 30 * 86_400;

fn local(db: &Db) -> Local {
    let conn = db.podcasts_conn();
    let now = crate::podcasts::now_secs();
    let started = store::continue_listening(&conn, 8).unwrap_or_default();
    let fresh = store::new_episodes(&conn, now - NEW_WINDOW, 20).unwrap_or_default();
    let mut seen = HashSet::new();
    let shortcuts = started
        .iter()
        .map(|e| ShelfItem { progress: Some(e.progress.fraction.clamp(0.0, 1.0)), ..episode_item(e) })
        .chain(fresh.iter().map(|e| ShelfItem { new_episode: e.progress.position_secs <= 0.0, ..episode_item(e) }))
        .filter(|i| seen.insert(i.id.clone()))
        .take(8)
        .collect();
    Local {
        shortcuts,
        new_eps: fresh.iter().map(episode_item).collect(),
        follows: store::follows(&conn).unwrap_or_default(),
    }
}

fn show_item(s: &Show) -> ShelfItem {
    ShelfItem {
        subtitle: Some(s.author.clone()).filter(|a| !a.is_empty()),
        thumbnail: s.thumbnail.clone(),
        ..item("podcast", &s.id, &s.title)
    }
}

// --- suggestions -------------------------------------------------------------------------------

/// Titles of the shows you listened to lately (then followed ones), at most 3.
fn seeds(db: &Db) -> Vec<String> {
    let conn = db.podcasts_conn();
    let mut out: Vec<String> = conn
        .prepare(
            "SELECT e.show_title, MAX(p.updated_at) AS m FROM podcast_progress p
             JOIN podcast_episodes e ON e.id = p.episode_id
             WHERE e.show_title != '' GROUP BY e.show_title ORDER BY m DESC LIMIT 3",
        )
        .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect())
        .unwrap_or_default();
    for s in store::follows(&conn).unwrap_or_default() {
        if out.len() < 3 && !out.iter().any(|t| norm(t) == norm(&s.title)) {
            out.push(s.title);
        }
    }
    out
}

/// A show's Apple genre id, from a directory search for its title (cached a day).
pub(crate) async fn genre_of(db: &Db, title: &str) -> Option<String> {
    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=5&country={}&term={}",
        itunes::country(db),
        urlencoding::encode(title)
    );
    let v = itunes::get_json(db, &url, &[]).await.ok()?;
    let results = v.get("results")?.as_array()?;
    let hit = results
        .iter()
        .find(|r| r.get("collectionName").and_then(Value::as_str).is_some_and(|n| norm(n) == norm(title)))
        .or(results.first())?;
    hit.get("genreIds")?.as_array()?.iter().filter_map(Value::as_str).find(|g| *g != "26").map(str::to_owned)
}

/// One chart entry: the iTunes collection id and its artwork.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartEntry {
    pub id: String,
    pub art: Option<String>,
}

/// Bigger Apple artwork: the chart feeds hand out 100 / 170 px thumbnails.
fn big_art(url: &str) -> String {
    url.replace("/100x100bb.", "/600x600bb.").replace("/170x170bb.", "/600x600bb.")
}

/// Either chart feed: the marketing-tools JSON (`feed.results[]`) or the iTunes RSS JSON
/// (`feed.entry[]`).
pub fn parse_chart(v: &Value) -> Vec<ChartEntry> {
    let feed = &v["feed"];
    if let Some(rows) = feed["results"].as_array() {
        return rows
            .iter()
            .filter_map(|r| {
                Some(ChartEntry {
                    id: r["id"].as_str()?.to_owned(),
                    art: r["artworkUrl100"].as_str().map(big_art),
                })
            })
            .collect();
    }
    let entries = match &feed["entry"] {
        Value::Array(a) => a.clone(),
        e @ Value::Object(_) => vec![e.clone()],
        _ => Vec::new(),
    };
    entries
        .iter()
        .filter_map(|e| {
            Some(ChartEntry {
                id: e["id"]["attributes"]["im:id"].as_str()?.to_owned(),
                art: e["im:image"].as_array().and_then(|a| a.last()).and_then(|i| i["label"].as_str()).map(big_art),
            })
        })
        .collect()
}

/// Apple's top shows, overall (`genre` None: the marketing-tools feed, or the iTunes RSS feed when
/// that one is down) or in one genre.
pub(crate) async fn chart(db: &Db, genre: Option<&str>) -> Vec<ChartEntry> {
    let cc = itunes::country(db).to_lowercase();
    let get = |url: String| async move { itunes::get_json(db, &url, &[]).await.map(|v| parse_chart(&v)).unwrap_or_default() };
    match genre {
        Some(g) => get(format!("https://itunes.apple.com/{cc}/rss/toppodcasts/limit=25/genre={g}/json")).await,
        None => {
            let top = get(format!("https://rss.marketingtools.apple.com/api/v2/{cc}/podcasts/top/25/podcasts.json")).await;
            if !top.is_empty() {
                return top;
            }
            get(format!("https://itunes.apple.com/{cc}/rss/toppodcasts/limit=25/json")).await
        }
    }
}

/// iTunes lookup of chart ids: the playable (RSS) shows by collection id, and each show's newest
/// episode by show id (feed URL).
pub fn parse_lookup(v: &Value) -> (HashMap<String, Show>, HashMap<String, Episode>) {
    let rows = v["results"].as_array().cloned().unwrap_or_default();
    let shows_only: Vec<Value> = rows.iter().filter(|r| r["kind"] == "podcast").cloned().collect();
    let shows = itunes::parse_shows(&serde_json::json!({ "results": shows_only }))
        .into_iter()
        .filter_map(|s| Some((s.itunes_id?.to_string(), s)))
        .collect();
    let mut eps: HashMap<String, Episode> = HashMap::new();
    for e in itunes::parse_episodes(v) {
        if let Some(show) = e.show_id.clone() {
            let newer = eps.get(&show).is_none_or(|o| e.published_at > o.published_at);
            if newer {
                eps.insert(show, e);
            }
        }
    }
    (shows, eps)
}

pub(crate) async fn lookup(db: &Db, ids: &[String]) -> (HashMap<String, Show>, HashMap<String, Episode>) {
    if ids.is_empty() {
        return Default::default();
    }
    let url = format!(
        "https://itunes.apple.com/lookup?id={}&entity=podcastEpisode&limit=1&country={}",
        ids.join(","),
        itunes::country(db)
    );
    itunes::get_json(db, &url, &[]).await.map(|v| parse_lookup(&v)).unwrap_or_default()
}

/// The category tiles; `art` is each genre's chart (its first show is the tile's cover).
fn categories_shelf(art: &HashMap<String, Vec<ChartEntry>>) -> Shelf {
    let tiles = CATEGORIES
        .iter()
        .map(|(id, name, color)| ShelfItem {
            params: Some((*id).to_owned()),
            color: Some((*color).to_owned()),
            thumbnail: art.get(*id).and_then(|c| c.first()).and_then(|e| e.art.clone()),
            ..item("podcast_category", &format!("podcat:{id}"), name)
        })
        .collect();
    shelf("pod_categories", "Podcast categories", "moods", "apple", tiles)
}

/// Pick up to `n` chart entries from `lists` in turn (round robin), skipping `taken` ids.
pub fn round_robin(lists: &[&[ChartEntry]], taken: &mut HashSet<String>, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let longest = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    'outer: for i in 0..longest {
        for l in lists {
            if out.len() == n {
                break 'outer;
            }
            if let Some(e) = l.get(i) {
                if taken.insert(e.id.clone()) {
                    out.push(e.id.clone());
                }
            }
        }
    }
    out
}

/// The suggested shelves (network): charts, genre charts for your shows, one lookup.
async fn suggested(db: &Db) -> Vec<Shelf> {
    use futures_util::future::join_all;
    let seeds = seeds(db);
    let genres: Vec<Option<String>> = join_all(seeds.iter().map(|t| genre_of(db, t))).await;
    // Every category's chart (its cover, and the seeds' genres among them), plus seed genres
    // outside the fixed list, alongside the overall top chart.
    let mut wanted: Vec<String> = CATEGORIES.iter().map(|c| c.0.to_owned()).collect();
    wanted.extend(genres.iter().flatten().filter(|g| !wanted.contains(g)).cloned().collect::<Vec<_>>());
    let (top, charts) = futures_util::join!(
        chart(db, None),
        join_all(wanted.iter().map(|g| async move { (g.clone(), chart(db, Some(g)).await) }))
    );
    let charts: HashMap<String, Vec<ChartEntry>> = charts.into_iter().collect();
    let empty = Vec::new();
    let chart_of = |g: &Option<String>| g.as_ref().and_then(|g| charts.get(g)).unwrap_or(&empty);

    let mut taken = HashSet::new();
    let because = match genres.first() {
        Some(g @ Some(_)) => round_robin(&[chart_of(g)], &mut taken, SHELF_LEN),
        _ => Vec::new(),
    };
    // Your other shows' genres first, then the rest of the first one's chart.
    let rest: Vec<&[ChartEntry]> =
        genres.iter().skip(1).chain(genres.first()).map(|g| chart_of(g).as_slice()).collect();
    let like = round_robin(&rest, &mut taken, SHELF_LEN);
    let top_ids: Vec<String> = top.iter().map(|e| e.id.clone()).take(20).collect();

    let mut seen = HashSet::new();
    let ids: Vec<String> =
        because.iter().chain(&like).chain(&top_ids).filter(|i| seen.insert(*i)).take(100).cloned().collect();
    let (shows, eps) = lookup(db, &ids).await;
    let cards = |ids: &[String]| -> Vec<ShelfItem> { ids.iter().filter_map(|i| shows.get(i)).map(show_item).collect() };

    // Episodes for you: the newest episode of each suggested show (top shows when there's no
    // listening yet), newest first, ones you've played left out.
    let source: Vec<&String> = if because.is_empty() && like.is_empty() { top_ids.iter().collect() } else { because.iter().chain(&like).collect() };
    let mut for_you: Vec<Episode> =
        source.iter().filter_map(|i| shows.get(*i)).filter_map(|s| eps.get(&s.id).cloned()).collect();
    {
        let conn = db.podcasts_conn();
        // Stored so the episode page opens them like any other RSS episode.
        let _ = store::upsert_episodes(&conn, &for_you);
        store::decorate(&conn, &mut for_you);
    }
    for_you.retain(|e| !e.progress.played);
    for_you.sort_by_key(|e| std::cmp::Reverse(e.published_at));
    for_you.truncate(SHELF_LEN);

    let mut out = vec![shelf("pod_for_you", "Episodes for you", "cards", "apple", for_you.iter().map(episode_item).collect())];
    out.push(shelf("pod_like", "Shows you might like", "cards", "apple", cards(&like)));
    if let Some(seed) = seeds.first() {
        out.push(shelf("pod_because", &format!("Because you listened to {seed}"), "cards", "apple", cards(&because)));
    }
    out.push(shelf("pod_top", "Top podcasts", "cards", "apple", cards(&top_ids)));
    out.push(categories_shelf(&charts));
    out.retain(|s| !s.items.is_empty());
    out
}

/// A podcast category's page: Apple's top shows in that genre, playable.
pub async fn podcast_category(state: &AppState, genre: String) -> Vec<ShelfItem> {
    let db = &state.db;
    let ids: Vec<String> = chart(db, Some(&genre)).await.into_iter().map(|e| e.id).collect();
    let (shows, _) = lookup(db, &ids).await;
    ids.iter().filter_map(|i| shows.get(i)).map(show_item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_chart_feeds() {
        let marketing = serde_json::json!({"feed": {"results": [
            {"id": "1200361736", "name": "The Daily", "artworkUrl100": "https://x/a.jpg/100x100bb.png"}
        ]}});
        assert_eq!(
            parse_chart(&marketing),
            vec![ChartEntry { id: "1200361736".into(), art: Some("https://x/a.jpg/600x600bb.png".into()) }]
        );
        let rss = serde_json::json!({"feed": {"entry": [
            {"id": {"attributes": {"im:id": "360084272"}}, "im:image": [
                {"label": "https://x/b.jpg/55x55bb.png"}, {"label": "https://x/b.jpg/170x170bb.png"}]}
        ]}});
        assert_eq!(parse_chart(&rss)[0].id, "360084272");
        assert_eq!(parse_chart(&rss)[0].art.as_deref(), Some("https://x/b.jpg/600x600bb.png"));
        // A one-show genre chart is an object, not an array.
        let one = serde_json::json!({"feed": {"entry": {"id": {"attributes": {"im:id": "7"}}}}});
        assert_eq!(parse_chart(&one)[0].id, "7");
        assert!(parse_chart(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn lookup_splits_shows_and_newest_episodes() {
        let v = serde_json::json!({"results": [
            {"wrapperType": "track", "kind": "podcast", "collectionId": 1, "collectionName": "Show",
             "artistName": "Host", "feedUrl": "https://f/1", "artworkUrl600": "https://a/1"},
            {"wrapperType": "podcastEpisode", "kind": "podcast-episode", "collectionId": 1, "collectionName": "Show",
             "feedUrl": "https://f/1", "trackName": "Old", "episodeUrl": "https://e/old.mp3", "releaseDate": "2026-11-01T00:00:00Z"},
            {"wrapperType": "podcastEpisode", "kind": "podcast-episode", "collectionId": 1, "collectionName": "Show",
             "feedUrl": "https://f/1", "trackName": "New", "episodeUrl": "https://e/new.mp3", "releaseDate": "2026-11-10T00:00:00Z"},
            {"wrapperType": "track", "kind": "podcast", "collectionId": 2, "collectionName": "No feed"}
        ]});
        let (shows, eps) = parse_lookup(&v);
        assert_eq!(shows.len(), 1, "episodes and feedless shows are not shows");
        assert_eq!(shows["1"].id, "https://f/1");
        assert_eq!(eps["https://f/1"].title, "New");
    }

    #[test]
    fn round_robin_interleaves_without_repeats() {
        let e = |id: &str| ChartEntry { id: id.into(), art: None };
        let a = [e("1"), e("2"), e("3")];
        let b = [e("2"), e("4")];
        let mut taken: HashSet<String> = ["3".to_string()].into();
        assert_eq!(round_robin(&[&a, &b], &mut taken, 10), vec!["1", "2", "4"]);
        assert_eq!(round_robin(&[&a], &mut HashSet::new(), 2), vec!["1", "2"]);
    }

    #[test]
    fn chip_is_never_empty_and_keeps_spotify_order() {
        let db = super::super::test_db();
        store::init(&db.podcasts_conn()).unwrap();
        // Nothing local, nothing fetched: the fixed category tiles still fill the chip.
        let shelves = assemble(local(&db), vec![categories_shelf(&HashMap::new())]);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].layout, "moods");
        assert_eq!(shelves[0].items.len(), CATEGORIES.len());
        assert_eq!(shelves[0].items[0].id, "podcat:1303");

        // A followed show is left out of the suggestions; shelves come in Spotify's order.
        store::follow(&db.podcasts_conn(), &Show { title: "Mine".into(), ..Show::rss("https://f/mine") }).unwrap();
        let sug = |id: &str, items: Vec<ShelfItem>| shelf(id, id, "cards", "apple", items);
        let remote = vec![
            categories_shelf(&HashMap::new()),
            sug("pod_top", vec![item("podcast", "https://f/mine", "Mine"), item("podcast", "https://f/x", "X")]),
            sug("pod_for_you", vec![item("episode", "e", "E")]),
        ];
        let ids: Vec<String> = assemble(local(&db), remote).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["pod_for_you", "pod_shows", "pod_top", "pod_categories"]);
        let top = assemble(local(&db), vec![sug("pod_top", vec![item("podcast", "https://f/mine", "Mine"), item("podcast", "https://f/x", "X")])]);
        assert_eq!(top.iter().find(|s| s.id == "pod_top").unwrap().items.len(), 1);
    }

    #[test]
    fn category_tiles_wear_their_charts_first_cover() {
        let art: HashMap<String, Vec<ChartEntry>> =
            [("1489".to_string(), vec![ChartEntry { id: "1".into(), art: Some("https://a/n".into()) }])].into();
        let s = categories_shelf(&art);
        let news = s.items.iter().find(|i| i.title == "News").unwrap();
        assert_eq!(news.thumbnail.as_deref(), Some("https://a/n"));
        assert_eq!(news.params.as_deref(), Some("1489"));
        assert!(s.items.iter().find(|i| i.title == "Comedy").unwrap().thumbnail.is_none());
    }

    /// Live: `cargo test -p encore-core --lib podhome::tests::live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_podcast_chip_fills_with_real_shows() {
        let db = super::super::test_db();
        store::init(&db.podcasts_conn()).unwrap();
        store::follow(&db.podcasts_conn(), &Show { title: "Huberman Lab".into(), author: "Scicomm Media".into(), ..Show::rss("https://feeds.megaphone.fm/hubermanlab") }).unwrap();
        let t = std::time::Instant::now();
        let remote = suggested(&db).await;
        println!("cold build: {:?}", t.elapsed());
        let shelves = assemble(local(&db), remote);
        for s in &shelves {
            let names: Vec<String> = s.items.iter().take(5).map(|i| format!("{} [{}]", i.title, i.thumbnail.is_some())).collect();
            println!("{} ({}): {} items: {}", s.title, s.id, s.items.len(), names.join(" | "));
        }
        let t = std::time::Instant::now();
        let _ = suggested(&db).await;
        println!("warm build (HTTP cache): {:?}", t.elapsed());
        for id in ["pod_for_you", "pod_like", "pod_because", "pod_top", "pod_categories"] {
            assert!(shelves.iter().any(|s| s.id == id && !s.items.is_empty()), "{id} empty");
        }
    }
}
