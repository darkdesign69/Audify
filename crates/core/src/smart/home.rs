//! The personal Home (research 07 §4(d)): Spotify-like shelves computed on this machine from the
//! listening history and taste profile, topped up from YouTube Music (artist pages for related
//! artists and new releases, song radios for mixes, the anonymous home feed and moods).
//!
//! Shelves, in order (each left out when it has nothing to show):
//!
//! | id | shelf | source |
//! |---|---|---|
//! | `shortcuts` | the 8 most recent contexts (Liked Songs, albums, artists) | local |
//! | `recently_played` | the last songs heard | local |
//! | `top_mixes` | "<Artist> Mix" for the top artists (an artist radio) | taste |
//! | `made_for_you` | up to 6 Your Mixes: top artists clustered by related/co-listen edges | taste + YT |
//! | `more_like:<id>` | "More like <Artist>": the artist page's related artists | YT |
//! | `jump_back_in` | contexts played a lot, but not lately | local |
//! | `new_releases` | this and last year's releases by the top artists | YT |
//! | `yt:<n>` | YouTube Music's own home shelves (personal when signed in) | YT |
//! | `moods` | Moods & genres tiles | YT |
//!
//! Everything goes through the shared filter (blocked songs/artists, explicit).
//!
//! Offline (no network or offline mode) Home is [`offline_home`] instead: an `offline` notice
//! shelf (layout `notice`, no items), the playable shortcuts, `downloaded` and the downloaded
//! recently played songs.

use std::collections::{HashMap, HashSet};

use innertube::{BrowseItem, SongItem};
use serde::{Deserialize, Serialize};

use super::library::Filter;
use super::taste::Taste;
use crate::db::Db;
use crate::state::AppState;

const ARTIST_TTL: i64 = 24 * 3600;
/// Artist pages fetched per Home build (cached a day, so mostly free).
const ARTIST_PAGES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfItem {
    /// `song` | `album` | `artist` | `playlist` | `liked` | `mix` (artist radio: `start_radio`
    /// with kind `artist`) | `radio` (song radio) | `daily_mix` (`smart_mix(id)`) | `mood`.
    pub kind: String,
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub explicit: bool,
    /// The playable row, for `song` items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song: Option<SongItem>,
    /// Mood tiles: the `params` for `get_browse_grid(MOODS_CATEGORY_ID, params)`, and the colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// stream: magic-d4 — `episode` shortcut tiles: how far in (0..1) for a started episode (the
    /// tile's progress bar), and whether it's a new, unplayed episode of a followed show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new_episode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shelf {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// `shortcuts` (the 2x4 grid at the top) | `cards` | `songs` | `moods`.
    pub layout: String,
    /// `local` | `youtube`.
    pub source: String,
    pub items: Vec<ShelfItem>,
}

pub(crate) fn shelf(id: &str, title: &str, layout: &str, source: &str, items: Vec<ShelfItem>) -> Shelf {
    Shelf {
        id: id.into(),
        title: title.into(),
        subtitle: None,
        layout: layout.into(),
        source: source.into(),
        items,
    }
}

pub(crate) fn item(kind: &str, id: &str, title: &str) -> ShelfItem {
    ShelfItem {
        kind: kind.into(),
        id: id.into(),
        title: title.into(),
        subtitle: None,
        thumbnail: None,
        explicit: false,
        song: None,
        params: None,
        color: None,
        progress: None,
        new_episode: false,
    }
}

pub fn song_item(s: &SongItem) -> ShelfItem {
    ShelfItem {
        subtitle: Some(s.artists.clone()),
        thumbnail: s.thumbnail.clone(),
        explicit: s.explicit,
        song: Some(s.clone()),
        ..item("song", &s.video_id, &s.title)
    }
}

pub fn card_item(c: &BrowseItem) -> ShelfItem {
    ShelfItem {
        subtitle: c.subtitle.clone(),
        thumbnail: c.thumbnail.clone(),
        explicit: c.explicit,
        ..item(c.kind, &c.id, &c.title)
    }
}

// --- local history -------------------------------------------------------------------------------

/// A song heard: the row, when it was last heard, how often.
#[derive(Debug, Clone)]
pub struct Heard {
    pub song: SongItem,
    pub last: i64,
    pub plays: i64,
}

pub fn recent_songs(db: &Db, limit: usize) -> Vec<Heard> {
    db.with_conn(|c| {
        let Ok(mut st) = c.prepare(
            "SELECT s.song_json, MAX(l.played_at) AS last, COUNT(*)
             FROM listens l JOIN listen_songs s ON s.video_id = l.video_id
             GROUP BY l.video_id ORDER BY last DESC LIMIT ?1",
        ) else {
            return Vec::new();
        };
        st.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map(|rows| {
            rows.flatten()
                .filter_map(|(j, last, plays)| {
                    serde_json::from_str::<SongItem>(&j).ok().map(|song| Heard {
                        song,
                        last,
                        plays,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
    })
}

/// The contexts behind recent songs, most recent first: the album when the row links one, else
/// the artist. With how many plays and when last heard.
pub fn contexts(recent: &[Heard]) -> Vec<(ShelfItem, i64, i64)> {
    let mut out: Vec<(ShelfItem, i64, i64)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for h in recent {
        let s = &h.song;
        if crate::local::is_local_song(&s.video_id) {
            continue;
        }
        let album = s.album.as_ref().filter(|t| !t.trim().is_empty());
        let artist = super::taste::display_artist(s);
        let ctx = if let (Some(id), Some(title)) = (&s.album_id, album) {
            ShelfItem {
                subtitle: Some(artist.clone()),
                thumbnail: s.thumbnail.clone(),
                ..item("album", id, title)
            }
        } else if let Some(id) = s
            .artist_id
            .clone()
            .or_else(|| s.artist_runs.iter().find_map(|r| r.id.clone()))
            .filter(|_| !artist.trim().is_empty())
        {
            ShelfItem { thumbnail: s.thumbnail.clone(), ..item("artist", &id, &artist) }
        } else {
            continue;
        };
        let key = format!("{}:{}", ctx.kind, ctx.id);
        match at.get(&key) {
            Some(&i) => {
                out[i].1 += h.plays;
                out[i].2 = out[i].2.max(h.last);
            }
            None => {
                at.insert(key, out.len());
                out.push((ctx, h.plays, h.last));
            }
        }
    }
    out
}

// --- clustering top artists into mixes -----------------------------------------------------------

/// Union-find clusters of `artists` (best first) joined by `edges`, best cluster first (by total
/// score), at most `max`. Research 07 "Your mixes": connected components over similar-artist
/// edges.
pub fn cluster(
    artists: &[(String, f64)],
    edges: &[(String, String)],
    max: usize,
) -> Vec<Vec<String>> {
    let index: HashMap<&str, usize> =
        artists.iter().enumerate().map(|(i, (k, _))| (k.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..artists.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut i = i;
        while p[i] != r {
            let next = p[i];
            p[i] = r;
            i = next;
        }
        r
    }
    for (a, b) in edges {
        if let (Some(&i), Some(&j)) = (index.get(a.as_str()), index.get(b.as_str())) {
            let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
            if ri != rj {
                // The better artist's root wins, so a cluster is named after its best artist.
                let (keep, child) = if ri < rj { (ri, rj) } else { (rj, ri) };
                parent[child] = keep;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..artists.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut v: Vec<(f64, Vec<String>)> = groups
        .into_values()
        .map(|mut members| {
            members.sort();
            let score = members.iter().map(|&i| artists[i].1).sum();
            (score, members.into_iter().map(|i| artists[i].0.clone()).collect())
        })
        .collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    v.into_iter().take(max).map(|(_, m)| m).collect()
}

/// Artists heard within 30 minutes of each other, twice or more: a local "these go together".
pub fn colisten_edges(recent: &[(String, i64)]) -> Vec<(String, String)> {
    let mut sorted: Vec<&(String, i64)> = recent.iter().collect();
    sorted.sort_by_key(|(_, at)| *at);
    let mut count: HashMap<(String, String), usize> = HashMap::new();
    for w in sorted.windows(2) {
        let ((a, ta), (b, tb)) = (w[0], w[1]);
        if a != b && tb - ta < 1800 {
            let k = if a < b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
            *count.entry(k).or_default() += 1;
        }
    }
    count.into_iter().filter(|(_, n)| *n >= 2).map(|(k, _)| k).collect()
}

// --- artist pages (cached) -----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtistInfo {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
    pub radio_playlist_id: Option<String>,
    pub top_songs: Vec<SongItem>,
    pub related: Vec<ShelfItem>,
    /// Album/single cards with the year parsed off the subtitle.
    pub releases: Vec<(ShelfItem, i32)>,
}

fn year_of(s: &str) -> Option<i32> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|w| w.len() == 4)
        .filter_map(|w| w.parse::<i32>().ok())
        .find(|y| (1900..=2100).contains(y))
}

pub async fn artist_info(state: &AppState, id: &str) -> Option<ArtistInfo> {
    let key = format!("artist:{id}");
    if let Some(a) = super::cache_get::<ArtistInfo>(&state.db, &key, ARTIST_TTL) {
        return Some(a);
    }
    let client = state.clients.get(innertube::METADATA_CLIENT)?;
    let page = match state.it.artist(client, id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "smart: artist page failed");
            return None;
        }
    };
    let mut info = ArtistInfo {
        id: id.to_owned(),
        name: page.name.clone().unwrap_or_default(),
        thumbnail: page.avatar.clone().or(page.thumbnail.clone()),
        radio_playlist_id: page.radio_playlist_id.clone(),
        top_songs: page.top_songs.clone(),
        ..Default::default()
    };
    for sec in &page.sections {
        if sec.items.iter().all(|c| c.kind == "artist") && !sec.items.is_empty() {
            info.related.extend(sec.items.iter().map(card_item));
        } else {
            for c in sec.items.iter().filter(|c| c.kind == "album") {
                if let Some(y) = c.subtitle.as_deref().and_then(year_of) {
                    let mut it = card_item(c);
                    if it.subtitle.is_none() {
                        it.subtitle = Some(info.name.clone());
                    }
                    info.releases.push((it, y));
                }
            }
        }
    }
    super::cache_put(&state.db, &key, &info);
    Some(info)
}

fn current_year(now: i64) -> i32 {
    // Days since 1970 to a civil year (Howard Hinnant's algorithm).
    let z = now.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let y = yoe + era * 400 + if mp >= 10 { 1 } else { 0 };
    y as i32
}

// --- the Home ------------------------------------------------------------------------------------

/// stream: magic-d4 — tell every client that Home changed. The `home-changed` payload:
///
/// - `{ "reason": "played", "context": <PlayContext> }`: a context (playlist, album, artist,
///   Liked Songs, show, radio, mix) was just started. Clients move that card to the front of their
///   `shortcuts` (and Recents) right away, then revalidate (`smart_home_local` is cheap).
/// - `{ "reason": "youtube" }`: YouTube Music's shelves arrived after Home stopped waiting.
///
/// Older clients ignore the payload and just reload.
pub fn emit_changed(
    host: &dyn crate::host::Host,
    reason: &str,
    context: Option<&crate::polish::context::PlayContext>,
) {
    use crate::host::HostExt;
    tracing::info!(
        reason,
        context = context.map(|c| c.key()).unwrap_or_default(),
        "home: home-changed"
    );
    let payload = match context {
        Some(c) => serde_json::json!({ "reason": reason, "context": c }),
        None => serde_json::json!({ "reason": reason }),
    };
    let _ = host.emit("home-changed", payload);
}

/// How many of the 8 shortcut tiles podcast episodes may take, so music keeps the grid.
const EPISODE_TILES: usize = 3;
/// A started episode stays a shortcut this long after it was last listened to.
const EPISODE_RESUME_WINDOW: i64 = 14 * 86_400;
/// An unplayed episode of a followed show counts as new for this long after it came out.
const EPISODE_NEW_WINDOW: i64 = 7 * 86_400;

/// An episode as a Home tile or card: playable (`song`), with its show's id in `params` so playing
/// it records the show as its context.
pub fn episode_item(e: &crate::podcasts::Episode) -> ShelfItem {
    ShelfItem {
        subtitle: Some(e.show_title.clone()).filter(|t| !t.is_empty()),
        thumbnail: e.thumbnail.clone(),
        song: Some(e.song()),
        params: e.show_id.clone(),
        ..item("episode", &e.id, &e.title)
    }
}

/// stream: magic-d4 — episode shortcut tiles (Spotify mixes them into the grid): episodes started
/// and not finished, listened to lately (with their progress), then new unplayed episodes of
/// followed shows. Each with the time it was last touched (listened, or published), newest first,
/// at most one per show and [`EPISODE_TILES`] in all.
pub fn episode_shortcuts(db: &Db, now: i64) -> Vec<(ShelfItem, i64)> {
    use crate::podcasts::store;
    let conn = db.podcasts_conn();
    let mut out: Vec<(ShelfItem, i64)> = Vec::new();
    let tile = episode_item;
    for e in store::continue_listening(&conn, 20).unwrap_or_default() {
        let at = e.progress.updated_at;
        if now - at > EPISODE_RESUME_WINDOW {
            break;
        }
        let fraction = e.progress.fraction.clamp(0.0, 1.0);
        out.push((ShelfItem { progress: Some(fraction), ..tile(&e) }, at));
    }
    for e in store::new_episodes(&conn, now - EPISODE_NEW_WINDOW, 20).unwrap_or_default() {
        if e.progress.position_secs > 0.0 || out.iter().any(|(t, _)| t.id == e.id) {
            continue;
        }
        let at = e.published_at.unwrap_or(now).min(now);
        out.push((ShelfItem { new_episode: true, ..tile(&e) }, at));
    }
    drop(conn);
    out.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    let mut shows = HashSet::new();
    out.retain(|(t, _)| {
        let show = t.subtitle.clone().unwrap_or_default();
        show.is_empty() || shows.insert(show)
    });
    out.truncate(EPISODE_TILES);
    out
}

/// The shortcut grid: Liked Songs first (when there are likes), then contexts and episode tiles by
/// their last touch. A show whose episode has a tile isn't also a tile itself.
pub fn shortcut_tiles(
    has_liked: bool,
    ctx: &[(ShelfItem, i64, i64)],
    episodes: &[(ShelfItem, i64)],
) -> Vec<ShelfItem> {
    let mut shortcuts: Vec<ShelfItem> = Vec::new();
    if has_liked {
        shortcuts.push(item("liked", "liked", "Liked Songs"));
    }
    let ep_shows: HashSet<&str> =
        episodes.iter().filter_map(|(t, _)| t.subtitle.as_deref()).collect();
    let mut rest: Vec<(&ShelfItem, i64)> = ctx
        .iter()
        .filter(|(i, _, _)| !(i.kind == "podcast" && ep_shows.contains(i.title.as_str())))
        .map(|(i, _, last)| (i, *last))
        .chain(episodes.iter().map(|(t, at)| (t, *at)))
        .collect();
    // Stable: equal times keep contexts ahead of episodes.
    rest.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    shortcuts.extend(rest.into_iter().take(8 - shortcuts.len()).map(|(i, _)| i.clone()));
    shortcuts
}

/// Kinds a shortcut tile can be (a `lib_played` row of any other kind, a folder say, is not).
// stream: audiobooks — a book played from its page is a shortcut too.
const SHORTCUT_KINDS: [&str; 8] = ["playlist", "album", "artist", "podcast", "radio", "mix", "daily_mix", "audiobook"];

/// stream: magic-d4 — fold in the contexts *started* (`lib_played`, stamped the moment a queue
/// starts) on top of the ones *listened* (`polish_listen_contexts`, only after the listen
/// threshold). Without this, a playlist you just pressed play on waited ~30 s to reach the first
/// shortcut tile; Spotify moves it at once. Most recent first.
pub fn with_started(db: &Db, mut ctx: Vec<(ShelfItem, i64, i64)>) -> Vec<(ShelfItem, i64, i64)> {
    let started: Vec<(String, String, Option<String>, i64)> = db.with_conn(|c| {
        let Ok(mut st) = c.prepare(
            "SELECT item_id, kind, card_json, played_at FROM lib_played ORDER BY played_at DESC LIMIT 50",
        ) else {
            return Vec::new();
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    });
    let mut at: HashMap<String, usize> =
        ctx.iter().enumerate().map(|(i, (s, _, _))| (format!("{}:{}", s.kind, s.id), i)).collect();
    for (id, kind, card, played_at) in started {
        if !SHORTCUT_KINDS.contains(&kind.as_str()) {
            continue;
        }
        let key = format!("{kind}:{id}");
        if let Some(&i) = at.get(&key) {
            ctx[i].2 = ctx[i].2.max(played_at);
            continue;
        }
        // Not listened from yet: the card stamped at start (a `PlayContext` or a `BrowseItem`).
        let Some(mut c) = card
            .and_then(|j| serde_json::from_str::<crate::polish::context::PlayContext>(&j).ok())
        else {
            continue;
        };
        c.kind = kind.clone();
        c.id = id;
        if c.title.trim().is_empty() {
            continue;
        }
        db.with_conn(|conn| c.live_art(conn));
        at.insert(key, ctx.len());
        ctx.push((c.shelf_item(), 0, played_at));
    }
    ctx.sort_by_key(|(_, _, last)| std::cmp::Reverse(*last));
    ctx
}

/// Local shelves only (no network): shortcuts, recently played, jump back in, top mixes.
pub fn local_shelves(db: &Db, taste: &Taste, filter: &Filter) -> Vec<Shelf> {
    let recent = recent_songs(db, 300);
    let mut shelves = Vec::new();
    // ── stream: polish ── recorded playback contexts (playlists, radios, shows) + derived ones.
    let ctx = crate::polish::context::home_contexts(db, &recent);
    let ctx = with_started(db, ctx);
    let episodes = episode_shortcuts(db, super::now_secs());
    let shortcuts =
        shortcut_tiles(!super::library::liked_ids(db).is_empty(), &ctx, &episodes);
    if shortcuts.len() >= 2 {
        shelves.push(shelf("shortcuts", "", "shortcuts", "local", shortcuts));
    }
    let played: Vec<ShelfItem> = recent
        .iter()
        .filter(|h| !filter.rejects(&h.song))
        .take(20)
        .map(|h| song_item(&h.song))
        .collect();
    if !played.is_empty() {
        shelves.push(shelf("recently_played", "Recently played", "cards", "local", played));
    }
    // Jump back in: contexts heard a lot, not in the last week, most played first.
    let now = super::now_secs();
    let mut back: Vec<&(ShelfItem, i64, i64)> = ctx
        .iter()
        .skip(8)
        .filter(|(_, plays, last)| *plays >= 3 && now - last > 7 * 86_400)
        .collect();
    back.sort_by_key(|(_, plays, _)| std::cmp::Reverse(*plays));
    let back: Vec<ShelfItem> = back.into_iter().take(12).map(|(i, _, _)| i.clone()).collect();
    if back.len() >= 3 {
        shelves.push(shelf("jump_back_in", "Jump back in", "cards", "local", back));
    }
    // Top mixes: one artist radio per top artist.
    let thumbs: HashMap<String, Option<String>> = recent
        .iter()
        .rev()
        .map(|h| (super::shuffle::artist_key(&h.song), h.song.thumbnail.clone()))
        .collect();
    let mixes: Vec<ShelfItem> = taste
        .top_artists(6)
        .into_iter()
        .filter_map(|(key, _)| {
            let name = taste.artist_names.get(&key).cloned().unwrap_or_default();
            if name.is_empty() {
                return None;
            }
            let (kind, id) = if key.starts_with("UC") {
                ("mix", key.clone())
            } else {
                // No channel: a radio on their most liked track.
                let best = taste
                    .track_artist
                    .iter()
                    .filter(|(_, a)| **a == key)
                    .max_by(|a, b| taste.track(a.0).total_cmp(&taste.track(b.0)))?;
                ("radio", best.0.clone())
            };
            Some(ShelfItem {
                subtitle: Some(format!("{name} and more")),
                thumbnail: thumbs.get(&key).cloned().flatten(),
                ..item(kind, &id, &format!("{name} Mix"))
            })
        })
        .collect();
    if !mixes.is_empty() {
        shelves.push(shelf("top_mixes", "Your top mixes", "cards", "local", mixes));
    }
    shelves
}

/// stream: magic-d4 — just the local shelves (shortcuts, recently played, jump back in, top
/// mixes): no network, a few ms. Clients revalidate with this right after `home-changed`
/// `played`, instead of rebuilding the whole Home (artist pages, YouTube's feed) on every play.
pub fn local_home(db: &Db) -> Vec<Shelf> {
    local_shelves(db, &super::taste::profile(db), &Filter::load(db))
}

/// Home offline (Spotify), for both apps: a "You're offline" notice, the shortcuts you can still
/// play, a "Downloaded" shelf (newest first) and the recently played songs that are on this
/// device. Nothing that would need YouTube.
pub fn offline_home(db: &Db) -> Vec<Shelf> {
    let downloads = db.with_conn(crate::downloads::store::statuses);
    let done: HashSet<String> =
        db.with_conn(crate::downloads::store::done_index).into_iter().map(|(id, ..)| id).collect();
    offline_shelves(local_home(db), &downloads, &done)
}

/// Liked Songs is `liked` in Home's tiles and `VLLM` in downloads: one key for both.
fn offline_key(id: &str) -> &str {
    if id == "liked" { crate::library::LIKED_ID } else { id }
}

/// [`offline_home`]'s rule, given the local shelves, the downloaded collections and the finished
/// track ids.
pub fn offline_shelves(
    shelves: Vec<Shelf>,
    downloads: &[crate::downloads::store::CollectionStatus],
    done: &HashSet<String>,
) -> Vec<Shelf> {
    let have: HashSet<&str> = downloads.iter().map(|d| offline_key(&d.id)).collect();
    // Home's own tile for a download, when it has one, carries the better title and art.
    let mut known: HashMap<&str, &ShelfItem> = HashMap::new();
    for it in shelves.iter().flat_map(|s| &s.items) {
        known.entry(offline_key(&it.id)).or_insert(it);
    }
    let mut newest: Vec<&crate::downloads::store::CollectionStatus> = downloads.iter().collect();
    newest.sort_by_key(|d| std::cmp::Reverse(d.added_at));
    let cards: Vec<ShelfItem> = newest
        .into_iter()
        .map(|d| {
            if d.kind == "liked" || offline_key(&d.id) == crate::library::LIKED_ID {
                return ShelfItem {
                    subtitle: Some(format!("{} songs", d.total)),
                    ..item("liked", "liked", "Liked Songs")
                };
            }
            let k = known.get(offline_key(&d.id)).copied();
            let title = if d.title.is_empty() || d.title == d.id {
                k.map(|k| k.title.clone()).unwrap_or_else(|| d.title.clone())
            } else {
                d.title.clone()
            };
            ShelfItem {
                subtitle: d.subtitle.clone().or_else(|| k.and_then(|k| k.subtitle.clone())),
                thumbnail: d.thumbnail.clone().or_else(|| k.and_then(|k| k.thumbnail.clone())),
                ..item(k.map_or(d.kind.as_str(), |k| k.kind.as_str()), &d.id, &title)
            }
        })
        .collect();
    let subtitle = if cards.is_empty() {
        "Download music to listen without a connection. It shows up here."
    } else {
        "Only music on this device plays until you're back online."
    };
    let mut out = vec![Shelf {
        subtitle: Some(subtitle.into()),
        ..shelf("offline", "You're offline", "notice", "local", vec![])
    }];
    let kept = |s: &Shelf, f: &dyn Fn(&ShelfItem) -> bool| {
        let items: Vec<ShelfItem> = s.items.iter().filter(|i| f(i)).cloned().collect();
        (!items.is_empty()).then(|| Shelf { items, ..s.clone() })
    };
    out.extend(
        shelves
            .iter()
            .find(|s| s.layout == "shortcuts")
            .and_then(|s| kept(s, &|i| have.contains(offline_key(&i.id)))),
    );
    if !cards.is_empty() {
        out.push(shelf("downloaded", "Downloaded", "cards", "local", cards));
    }
    out.extend(shelves.iter().find(|s| s.id == "recently_played").and_then(|s| {
        kept(s, &|i| if i.kind == "song" { done.contains(&i.id) } else { have.contains(offline_key(&i.id)) })
    }));
    out
}

/// Turn YouTube's own home sections into shelves.
fn youtube_shelves(page: &innertube::HomePage, filter: &Filter) -> Vec<Shelf> {
    page.sections
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.items.is_empty())
        .map(|(i, s)| {
            let items: Vec<ShelfItem> =
                s.items.iter().filter(|c| !filter.rejects_card(c)).map(card_item).collect();
            let layout = if s.items.iter().all(|c| c.kind == "song") { "songs" } else { "cards" };
            let id = format!("yt:{}", s.kind.map(str::to_owned).unwrap_or_else(|| i.to_string()));
            shelf(&id, &s.title, layout, "youtube", items)
        })
        .filter(|s| !s.items.is_empty())
        .collect()
}

/// Artist pages of the top artists with a channel, followed artists included (cached a day).
pub async fn top_artist_infos(state: &AppState, taste: &Taste) -> Vec<ArtistInfo> {
    let top = taste.top_artists(30);
    let channel_ids: Vec<String> = top
        .iter()
        .map(|(k, _)| k.clone())
        .filter(|k| k.starts_with("UC"))
        .take(ARTIST_PAGES)
        .collect();
    // ── stream: reliability ── the artist pages behind "More like" and "New releases" enrich
    // Home (cached for days); they give way to anything the user clicks.
    innertube::background(futures_util::future::join_all(
        channel_ids.iter().map(|id| artist_info(state, id)),
    ))
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// Your Mix clusters: top artists joined by related-artist and co-listen edges, at most 6.
pub fn mix_clusters(db: &Db, taste: &Taste, infos: &[ArtistInfo]) -> Vec<Vec<String>> {
    let top = taste.top_artists(30);
    let mut edges: Vec<(String, String)> = Vec::new();
    for a in infos {
        for r in &a.related {
            edges.push((a.id.clone(), r.id.clone()));
        }
    }
    let heard_artists: Vec<(String, i64)> = recent_songs(db, 300)
        .into_iter()
        .map(|h| (super::shuffle::artist_key(&h.song), h.last))
        .collect();
    edges.extend(colisten_edges(&heard_artists));
    cluster(&top, &edges, 6)
}

/// How long Home waits for YouTube Music's home and moods pages (stream: perf).
const YOUTUBE_HOME_BUDGET: std::time::Duration = std::time::Duration::from_secs(4);

type YoutubeHome = (Result<innertube::HomePage, innertube::Error>, Result<Vec<innertube::MoodSection>, innertube::Error>);

/// A YouTube home fetch that finished after its Home load gave up on it, kept for the next load.
static LATE_YOUTUBE_HOME: std::sync::Mutex<Option<(std::time::Instant, YoutubeHome)>> =
    std::sync::Mutex::new(None);

/// YouTube Music's home and moods pages within [`YOUTUBE_HOME_BUDGET`] (stream: perf). The fetch
/// runs as its own task, so going over budget doesn't cancel it: it parks the answer in
/// [`LATE_YOUTUBE_HOME`] and emits `home-changed` for Home to reload.
async fn youtube_home(state: &AppState) -> Option<YoutubeHome> {
    if let Some((at, page)) = LATE_YOUTUBE_HOME.lock().unwrap().take() {
        if at.elapsed() < std::time::Duration::from_secs(300) {
            return Some(page);
        }
    }
    state.clients.get(innertube::METADATA_CLIENT)?;
    let st = crate::state::global()?;
    let (tx, rx) = tokio::sync::oneshot::channel::<YoutubeHome>();
    crate::rt::spawn(async move {
        let Some(client) = st.clients.get(innertube::METADATA_CLIENT) else { return };
        let page = futures_util::join!(st.it.home(client, None), st.it.moods(client));
        if let Err(page) = tx.send(page) {
            // The Home load stopped waiting: keep this for the next one and say so.
            *LATE_YOUTUBE_HOME.lock().unwrap() = Some((std::time::Instant::now(), page));
            emit_changed(&*st.host, "youtube", None);
        }
    });
    match tokio::time::timeout(YOUTUBE_HOME_BUDGET, rx).await {
        Ok(Ok(page)) => Some(page),
        Ok(Err(_)) => None,
        Err(_) => {
            tracing::warn!(budget = ?YOUTUBE_HOME_BUDGET, "smart: youtube home over budget, painting without it");
            None
        }
    }
}

/// The whole Home. Local shelves first, then the ones that need artist pages, then YouTube's.
pub async fn home(state: &AppState) -> Vec<Shelf> {
    let db = &state.db;
    if crate::downloads::offline() {
        return offline_home(db);
    }
    let taste = super::taste::profile(db);
    let filter = Filter::load(db);
    let mut shelves = local_shelves(db, &taste, &filter);

    // stream: perf — the artist pages and YouTube's own home are independent round trips; they
    // ran one after the other. Now together, and YouTube's home gets a budget: when it stalls (a
    // 403 heals for ~7 s, a dead connection waits out a 60 s timeout) Home paints without those
    // shelves instead of showing skeletons, and the fetch carries on: when it lands, the UI is told
    // (`home-changed`) and the next Home load picks the pages up.
    let youtube = youtube_home(state);
    let (infos, youtube) = futures_util::join!(top_artist_infos(state, &taste), youtube);

    // Made for you (smart2): the stored Your Mixes, Weekly Discoveries, New Release Mix, Your Rotation and
    // Throwback Mix (`mfy.rs`). Before the first build, the clusters as live mixes.
    if let Some(s) = super::mfy::shelf(db) {
        shelves.push(s);
    } else {
        let clusters = mix_clusters(db, &taste, &infos);
        let daily: Vec<ShelfItem> = clusters
            .iter()
            .enumerate()
            .map(|(n, members)| {
                let names: Vec<String> = members
                    .iter()
                    .filter_map(|k| taste.artist_names.get(k).cloned())
                    .filter(|n| !n.is_empty())
                    .take(3)
                    .collect();
                let thumb = infos
                    .iter()
                    .find(|i| members.contains(&i.id))
                    .and_then(|i| i.thumbnail.clone());
                ShelfItem {
                    subtitle: Some(format!("{} and more", names.join(", "))),
                    thumbnail: thumb,
                    ..item(
                        "daily_mix",
                        &format!("dailymix:{}", members.join(",")),
                        &format!("Your Mix {}", n + 1),
                    )
                }
            })
            .collect();
        if !daily.is_empty() {
            shelves.push(shelf("made_for_you", "Made for you", "cards", "local", daily));
        }
    }

    // More like <artist>: the two top artists' related artists.
    for a in infos.iter().take(2) {
        let items: Vec<ShelfItem> =
            a.related.iter().filter(|r| !filter.rejects_card(&as_card(r))).cloned().collect();
        if !items.is_empty() && !a.name.is_empty() {
            shelves.push(shelf(
                &format!("more_like:{}", a.id),
                &format!("More like {}", a.name),
                "cards",
                "youtube",
                items,
            ));
        }
    }

    // New releases from top artists: this year and last, newest first, one per artist first.
    let year = current_year(super::now_secs());
    let mut releases: Vec<(i32, usize, ShelfItem)> = Vec::new();
    for a in &infos {
        for (n, (r, y)) in a.releases.iter().filter(|(_, y)| *y >= year - 1).enumerate() {
            releases.push((*y, n, r.clone()));
        }
    }
    releases.sort_by_key(|(y, n, _)| (std::cmp::Reverse(*y), *n));
    let mut seen = HashSet::new();
    let releases: Vec<ShelfItem> = releases
        .into_iter()
        .filter(|(_, _, r)| seen.insert(r.id.clone()))
        .map(|(_, _, r)| r)
        .filter(|r| !(filter.explicit_on() && r.explicit))
        .take(12)
        .collect();
    if !releases.is_empty() {
        shelves.push(shelf("new_releases", "New releases for you", "cards", "youtube", releases));
    }

    // YouTube Music's own home (charts, new releases, moods shelves; personal when signed in).
    if let Some((page, moods)) = youtube {
        match page {
            Ok(p) => shelves.extend(youtube_shelves(&p, &filter)),
            Err(e) => tracing::warn!(error = %e, "smart: youtube home failed"),
        }
        if let Ok(moods) = moods {
            // Each tile wears the cover Search's Browse tiles use (`get_mood_art`, cached a week).
            let art = crate::api::cached_mood_art(db);
            let tiles: Vec<ShelfItem> = moods
                .iter()
                .flat_map(|m| m.items.iter())
                .take(24)
                .map(|m| ShelfItem {
                    params: Some(m.params.clone()),
                    color: Some(m.color.clone()),
                    thumbnail: art.get(&m.params).cloned(),
                    ..item("mood", &m.params, &m.title)
                })
                .collect();
            let missing: Vec<String> =
                tiles.iter().filter(|t| t.thumbnail.is_none()).filter_map(|t| t.params.clone()).collect();
            fill_mood_art(missing);
            if !tiles.is_empty() {
                shelves.push(shelf("moods", "Moods & genres", "moods", "youtube", tiles));
            }
        }
    }
    shelves
}

/// Fetch the covers the moods shelf lacks in the background (one fetch at a time); when any
/// arrive, Home reloads (`home-changed`) and the tiles get their art.
fn fill_mood_art(missing: Vec<String>) {
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if missing.is_empty() || BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::rt::spawn(async move {
        if let Some(st) = crate::state::global() {
            let got = crate::api::get_mood_art(&st, missing).await.unwrap_or_default();
            if !got.is_empty() {
                emit_changed(&*st.host, "youtube", None);
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}

fn as_card(s: &ShelfItem) -> BrowseItem {
    BrowseItem {
        kind: match s.kind.as_str() {
            "artist" => "artist",
            "album" => "album",
            "playlist" => "playlist",
            _ => "song",
        },
        id: s.id.clone(),
        title: s.title.clone(),
        subtitle: s.subtitle.clone(),
        thumbnail: s.thumbnail.clone(),
        duration: None,
        album_id: None,
        artist_runs: Vec::new(),
        play_count: None,
        is_video: false,
        is_upload: false,
        explicit: s.explicit,
    }
}

// --- Your Mix tracks ----------------------------------------------------------------------------

/// The tracks of a Your Mix (`dailymix:<artist key>,<artist key>...`): half the user's own
/// favourites by those artists, half radio fill seeded from them, filtered, versions collapsed,
/// artists spread. Research 07 "Your mixes".
pub async fn mix_tracks(state: &AppState, id: &str, size: usize) -> Result<Vec<SongItem>, String> {
    let keys: Vec<String> = id
        .strip_prefix("dailymix:")
        .ok_or("not a mix")?
        .split(',')
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
        .collect();
    let db = &state.db;
    let taste = super::taste::profile(db);
    let filter = Filter::load(db);
    let recent = recent_songs(db, 1000);
    let mut favourites: Vec<(f64, SongItem)> = recent
        .into_iter()
        .filter(|h| keys.contains(&super::shuffle::artist_key(&h.song)))
        .map(|h| (taste.track(&h.song.video_id), h.song))
        .filter(|(s, song)| *s > 0.0 && !filter.rejects(song))
        .collect();
    favourites.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut favourites: Vec<SongItem> = favourites.into_iter().map(|(_, s)| s).collect();
    favourites.truncate(size / 2);
    // Radio fill from the cluster's best artists' best songs.
    let seeds: Vec<String> = favourites.iter().take(3).map(|s| s.video_id.clone()).collect();
    let radios =
        futures_util::future::join_all(seeds.iter().map(|v| super::recs::song_radio(state, v)))
            .await;
    let mut fill: Vec<SongItem> = Vec::new();
    let max_len = radios.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..max_len {
        for r in &radios {
            if let Some(s) = r.get(i) {
                fill.push(s.clone());
            }
        }
    }
    let mut filter = filter;
    if filter.explicit_on() {
        super::explicit::resolve_unknown(db, &fill, &mut filter, 12).await;
    }
    filter.retain(&mut fill, None);
    let have: HashSet<String> = favourites.iter().map(|s| s.video_id.clone()).collect();
    fill.retain(|s| !have.contains(&s.video_id));
    // Interleave: favourite, fill, favourite, fill ...
    let mut out = Vec::with_capacity(size);
    let (mut a, mut b) = (favourites.into_iter(), fill.into_iter());
    while out.len() < size * 2 {
        match (a.next(), b.next()) {
            (None, None) => break,
            (x, y) => out.extend(x.into_iter().chain(y)),
        }
    }
    for s in &mut out {
        s.autoplay = false;
    }
    // ── stream: canonical ── favourites played as music videos join as their songs.
    let mut out = crate::canonical::to_songs(state, out).await;
    super::recs::collapse_versions(&mut out);
    out.truncate(size);
    Ok(super::recs::spread_artists(out, 2, 10))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    fn dl(id: &str, kind: &str, title: &str, added_at: i64) -> crate::downloads::store::CollectionStatus {
        crate::downloads::store::CollectionStatus {
            id: id.into(),
            kind: kind.into(),
            title: title.into(),
            subtitle: None,
            thumbnail: None,
            total: 3,
            done: 3,
            failed: 0,
            bytes: 0,
            added_at,
        }
    }

    #[test]
    fn offline_home_only_shows_whats_on_the_device() {
        let shortcuts = vec![
            item("liked", "liked", "Liked Songs"),
            ShelfItem { thumbnail: Some("art".into()), ..item("album", "MPRE1", "Album One") },
            item("playlist", "PL2", "Not downloaded"),
        ];
        let played = vec![
            song_item(&song("s1", "Song 1", "A", None, "3:00")),
            song_item(&song("s2", "Song 2", "A", None, "3:00")),
        ];
        let shelves = vec![
            shelf("shortcuts", "", "shortcuts", "local", shortcuts),
            shelf("recently_played", "Recently played", "cards", "local", played),
            shelf("jump_back_in", "Jump back in", "cards", "local", vec![item("album", "MPRE9", "x")]),
        ];
        // The album downloaded first with its id as a placeholder title, Liked Songs later.
        let downloads = [dl("MPRE1", "album", "MPRE1", 1), dl("VLLM", "liked", "Liked Songs", 2)];
        let done: HashSet<String> = ["s2".to_string()].into();
        let out = offline_shelves(shelves, &downloads, &done);
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["offline", "shortcuts", "downloaded", "recently_played"]);
        assert_eq!(out[0].title, "You're offline");
        assert_eq!(out[0].layout, "notice");
        let tiles: Vec<&str> = out[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(tiles, ["liked", "MPRE1"]);
        // Newest download first; Home's tile lends the album its title and art.
        let cards = &out[2].items;
        assert_eq!((cards[0].kind.as_str(), cards[0].id.as_str()), ("liked", "liked"));
        assert_eq!(cards[0].subtitle.as_deref(), Some("3 songs"));
        assert_eq!((cards[1].title.as_str(), cards[1].thumbnail.as_deref()), ("Album One", Some("art")));
        let songs: Vec<&str> = out[3].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(songs, ["s2"]);
    }

    #[test]
    fn offline_home_without_downloads_is_just_the_notice() {
        let shelves = vec![shelf(
            "recently_played",
            "Recently played",
            "cards",
            "local",
            vec![song_item(&song("s1", "Song 1", "A", None, "3:00"))],
        )];
        let out = offline_shelves(shelves, &[], &HashSet::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "offline");
        assert!(out[0].subtitle.as_deref().unwrap().starts_with("Download music"));
    }

    #[test]
    fn offline_home_reads_the_download_tables() {
        let db = super::super::test_db();
        db.with_conn(crate::downloads::store::migrate);
        let out = offline_home(&db);
        assert_eq!(out[0].id, "offline");
    }

    fn heard(
        id: &str,
        album: Option<(&str, &str)>,
        artist_id: Option<&str>,
        last: i64,
        plays: i64,
    ) -> Heard {
        let mut s = song(id, id, "Artist", album.map(|a| a.1), "3:00");
        s.album_id = album.map(|a| a.0.to_owned());
        s.artist_id = artist_id.map(str::to_owned);
        Heard { song: s, last, plays }
    }

    #[test]
    fn contexts_merge_by_album_then_artist() {
        let recent = vec![
            heard("a", Some(("MPRE1", "One")), Some("UC1"), 300, 2),
            heard("b", None, Some("UC2"), 200, 1),
            heard("c", Some(("MPRE1", "One")), Some("UC1"), 100, 5),
            heard("d", None, None, 50, 1),
        ];
        let ctx = contexts(&recent);
        let ids: Vec<(&str, &str)> =
            ctx.iter().map(|(i, _, _)| (i.kind.as_str(), i.id.as_str())).collect();
        assert_eq!(ids, [("album", "MPRE1"), ("artist", "UC2")]);
        assert_eq!(ctx[0].1, 7);
        assert_eq!(ctx[0].2, 300);
    }

    #[test]
    fn clusters_follow_edges_and_rank_by_score() {
        let artists: Vec<(String, f64)> =
            [("rock1", 10.0), ("pop1", 8.0), ("rock2", 5.0), ("pop2", 4.0), ("jazz", 1.0)]
                .iter()
                .map(|(k, s)| (k.to_string(), *s))
                .collect();
        let edges = vec![
            ("rock1".to_string(), "rock2".to_string()),
            ("pop2".to_string(), "pop1".to_string()),
            ("rock2".to_string(), "unknown".to_string()),
        ];
        let c = cluster(&artists, &edges, 6);
        assert_eq!(c, vec![vec!["rock1", "rock2"], vec!["pop1", "pop2"], vec!["jazz"]]);
        assert_eq!(cluster(&artists, &edges, 2).len(), 2);
    }

    #[test]
    fn colisten_needs_two_sessions() {
        let plays = vec![
            ("A".to_string(), 0),
            ("B".to_string(), 600),
            ("A".to_string(), 100_000),
            ("B".to_string(), 100_300),
            ("C".to_string(), 200_000),
            ("A".to_string(), 200_100),
        ];
        assert_eq!(colisten_edges(&plays), vec![("A".to_string(), "B".to_string())]);
    }

    #[test]
    fn years() {
        assert_eq!(year_of("Single \u{2022} 2026"), Some(2026));
        assert_eq!(year_of("2019"), Some(2019));
        assert_eq!(year_of("Album"), None);
        assert_eq!(current_year(1_763_164_800), 2025); // 2025-11-15
        assert_eq!(current_year(1_794_700_800), 2026); // 2026-11-15
    }

    #[test]
    fn local_shelves_from_history() {
        let db = super::super::test_db();
        let now = super::super::now_secs();
        for (i, (id, artist)) in [("s1", "UCa"), ("s2", "UCb"), ("s3", "UCa")].iter().enumerate() {
            let mut s = song(id, id, &format!("Artist {artist}"), Some("Alb"), "3:00");
            s.artist_id = Some(artist.to_string());
            s.album_id = Some(format!("MPRE{artist}"));
            let json = serde_json::to_string(&s).unwrap();
            db.record_listen(id, &json, now - 100 + i as i64, 180);
        }
        db.with_conn(|c| {
            c.execute("INSERT INTO hidden_songs(video_id, context_id, hidden_at) VALUES('s2', '', 0)", []).unwrap();
        });
        let taste = super::super::taste::profile(&db);
        let shelves = local_shelves(&db, &taste, &Filter::load(&db));
        let ids: Vec<&str> = shelves.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["shortcuts", "recently_played", "top_mixes"]);
        let played: Vec<&str> = shelves[1].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(played, ["s3", "s1"]);
        assert_eq!(shelves[2].items[0].kind, "mix");
        assert_eq!(shelves[2].items[0].id, "UCa");
    }

    // stream: magic-d4 — the shortcut grid mixes in podcast episodes.
    fn ep(id: &str, show: &str, published: i64) -> crate::podcasts::Episode {
        crate::podcasts::Episode {
            id: id.into(),
            show_id: Some(format!("MPSP{show}")),
            show_title: show.into(),
            title: format!("Episode {id}"),
            published_at: Some(published),
            duration_secs: Some(3600),
            thumbnail: Some(format!("https://img/{id}")),
            ..Default::default()
        }
    }

    #[test]
    fn episode_tiles_carry_progress_and_new_badges() {
        use crate::podcasts::{store, Show};
        let db = super::super::test_db();
        let now = super::super::now_secs();
        {
            let c = db.podcasts_conn();
            store::init(&c).unwrap();
            for show in ["Lab", "Talk", "Old"] {
                store::follow(&c, &Show { id: format!("MPSP{show}"), title: show.into(), ..Default::default() }).unwrap();
            }
            store::upsert_episodes(
                &c,
                &[
                    ep("started", "Lab", now - 30 * 86_400),
                    ep("fresh", "Talk", now - 3600),
                    ep("fresh2", "Talk", now - 7200),
                    ep("stale", "Old", now - 20 * 86_400),
                    ep("labnew", "Lab", now - 60),
                ],
            )
            .unwrap();
            store::save_position(&c, "started", 900.0, 3600.0).unwrap();
        }
        let tiles = episode_shortcuts(&db, now + 10);
        let ids: Vec<&str> = tiles.iter().map(|(t, _)| t.id.as_str()).collect();
        // `labnew` is newer, but Lab already has its started episode (listened at `now`).
        assert_eq!(ids, ["started", "fresh"]);
        let started = &tiles[0].0;
        assert_eq!(started.kind, "episode");
        assert!((started.progress.unwrap() - 0.25).abs() < 1e-6);
        assert!(!started.new_episode);
        assert_eq!(started.song.as_ref().unwrap().video_id, "started");
        assert_eq!(started.params.as_deref(), Some("MPSPLab"));
        assert!(tiles[1].0.new_episode && tiles[1].0.progress.is_none());
        let json = serde_json::to_value(&tiles[1].0).unwrap();
        assert_eq!(json["newEpisode"], true);
        assert!(json.get("progress").is_none());
    }

    #[test]
    fn shortcut_grid_orders_contexts_and_episodes_by_last_touch() {
        let ctx = vec![
            (item("playlist", "VLPLa", "Road trip"), 3, 500),
            (item("podcast", "MPSPLab", "Lab"), 2, 450),
            (item("album", "MPREb", "Album"), 9, 100),
        ];
        let episodes = vec![(ShelfItem { subtitle: Some("Lab".into()), ..item("episode", "e1", "Ep") }, 300)];
        let tiles = shortcut_tiles(true, &ctx, &episodes);
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["liked", "VLPLa", "e1", "MPREb"], "the Lab show gives way to its episode");
        let many: Vec<(ShelfItem, i64, i64)> =
            (0..12).map(|n| (item("album", &format!("MPRE{n}"), "A"), 1, 1000 - n)).collect();
        assert_eq!(shortcut_tiles(false, &many, &[]).len(), 8);
    }
}
