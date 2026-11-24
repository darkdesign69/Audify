//! The playback context behind each listen: what the user pressed play on.
//!
//! Spotify's Recents, "Recent shortcuts" and "Jump back in" list *contexts*: the playlist, album,
//! artist, Liked Songs, podcast show or radio a song was played from. The queue only keeps
//! `source_id` / `source_name`, so this module remembers the full card behind that pair when a
//! queue is started ([`register`], from `play_playlist` / `start_radio`), falls back to deriving one
//! from the pair itself ([`derive`]: local playlists, Liked Songs, YouTube playlists and albums,
//! podcast episodes), and on every listen past the threshold ([`on_listen`]):
//! - stamps `lib_played` (the library's Recents sort and `lib_recent_contexts`), and
//! - logs `polish_listen_contexts`, which [`home_contexts`] turns into Home's context shelves.

use std::collections::HashMap;

use innertube::SongItem;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::smart::home::{Heard, ShelfItem};

/// A context card. Same field names as a `BrowseItem` / Home `ShelfItem`, so the UI can use it
/// as either.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayContext {
    /// `playlist` | `album` | `artist` | `liked` | `podcast` | `radio` | `mix` | `daily_mix`.
    pub kind: String,
    /// Navigation id: a playlist id (`VL…`, `LOCALPLAYLIST:n`), album `MPRE…`, artist `UC…`,
    /// `VLLM` for Liked Songs, a show id (`MPSP…` or feed URL), the radio seed id, a mix id.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// `radio` only: the seed's kind when it isn't a song (`album` | `playlist` | `artist`), i.e.
    /// `start_radio(params ?? 'song', id, title)` replays it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<String>,
}

impl PlayContext {
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }

    /// Spotify's fixed spellings / ids for the special contexts.
    fn normalized(mut self) -> Self {
        if self.kind == "liked" || self.id == crate::library::LIKED_ID || self.id == "LM" {
            self.kind = "liked".into();
            self.id = crate::library::LIKED_ID.into();
            if self.title.is_empty() {
                self.title = "Liked Songs".into();
            }
        }
        // A song is not a context (Spotify lists the album it's on instead).
        if self.kind == "song" {
            self.kind = String::new();
        }
        self
    }

    /// A context Home and Recents can show: it needs a kind, an id and a title (a card with no
    /// title rendered as bare artwork on Home).
    fn valid(&self) -> bool {
        !self.kind.is_empty() && !self.id.is_empty() && !self.title.trim().is_empty()
    }

    /// stream: tilefix — swap artwork that no longer loads for art that does. A podcast show's
    /// card keeps the YouTube show art it was recorded with, a signed URL that 404s hours later
    /// (Home's shortcut tile showed a broken image every launch); see
    /// [`crate::podcasts::store::show_art`].
    pub fn live_art(&mut self, conn: &Connection) {
        if self.kind == "podcast" {
            self.thumbnail = crate::podcasts::store::show_art(conn, &self.id, self.thumbnail.as_deref());
        }
    }

    /// The card stored in `lib_played.card_json`.
    fn card(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// As a Home shelf item. Liked Songs uses Home's own `liked` id.
    pub fn shelf_item(&self) -> ShelfItem {
        ShelfItem {
            kind: self.kind.clone(),
            id: if self.kind == "liked" { "liked".into() } else { self.id.clone() },
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            thumbnail: self.thumbnail.clone(),
            explicit: false,
            song: None,
            params: self.params.clone(),
            color: None,
            progress: None,
            new_episode: false,
        }
    }
}

fn source_key(source_id: Option<&str>, source_name: Option<&str>) -> String {
    format!("{}\u{1f}{}", source_id.unwrap_or(""), source_name.unwrap_or(""))
}

/// Remember the card behind a queue's (`source_id`, `source_name`).
pub fn register(db: &Db, source_id: Option<&str>, source_name: Option<&str>, ctx: &PlayContext) {
    if source_id.is_none() && source_name.is_none() {
        return;
    }
    let ctx = ctx.clone().normalized();
    if !ctx.valid() {
        return;
    }
    let json = serde_json::to_string(&ctx).unwrap_or_default();
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO polish_contexts(key, json, at) VALUES(?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET json = excluded.json, at = excluded.at",
            params![source_key(source_id, source_name), json, super::now_secs()],
        );
        // Only the last few hundred queues can still be playing or restored.
        let _ = c.execute(
            "DELETE FROM polish_contexts WHERE key NOT IN
               (SELECT key FROM polish_contexts ORDER BY at DESC LIMIT 300)",
            [],
        );
    });
}

pub fn lookup(db: &Db, source_id: Option<&str>, source_name: Option<&str>) -> Option<PlayContext> {
    db.with_conn(|c| {
        c.query_row(
            "SELECT json FROM polish_contexts WHERE key = ?1",
            [source_key(source_id, source_name)],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    })
    .and_then(|j| serde_json::from_str(&j).ok())
}

/// A context from the queue's own source fields and the playing row, when nothing was
/// registered (an older UI, a restored queue from before this existed).
pub fn derive(
    db: &Db,
    source_id: Option<&str>,
    source_name: Option<&str>,
    item: &SongItem,
) -> Option<PlayContext> {
    let name = source_name.unwrap_or_default().to_owned();
    if let Some(id) = source_id.filter(|s| !s.is_empty()) {
        if id == crate::library::LIKED_ID || id == "LM" {
            return Some(PlayContext { kind: "liked".into(), ..Default::default() }.normalized());
        }
        if crate::state::is_local_playlist(id) {
            let p = db.with_conn(|c| crate::library::playlist(c, id))?;
            return Some(PlayContext {
                kind: "playlist".into(),
                id: p.id,
                title: p.name,
                subtitle: Some("Playlist".into()),
                thumbnail: p.thumbnail,
                params: None,
            });
        }
        let bare = id.strip_prefix("VL").unwrap_or(id);
        // An album page plays its `OLAK5uy_…` playlist; the album's page is the row's MPRE.
        if bare.starts_with("OLAK5uy_") {
            if let Some(album) = item.album_id.as_ref().filter(|a| a.starts_with("MPRE")) {
                return Some(PlayContext {
                    kind: "album".into(),
                    id: album.clone(),
                    title: item.album.clone().filter(|a| !a.is_empty()).unwrap_or(name),
                    subtitle: Some(crate::smart::taste::display_artist(item)),
                    thumbnail: item.thumbnail.clone(),
                    params: None,
                });
            }
        }
        if name.is_empty() {
            return None;
        }
        return Some(PlayContext {
            kind: "playlist".into(),
            id: if bare.starts_with("PL") || bare.starts_with("RD") || bare.starts_with("OLAK") {
                format!("VL{bare}")
            } else {
                id.to_owned()
            },
            title: name,
            subtitle: Some("Playlist".into()),
            thumbnail: item.thumbnail.clone(),
            params: None,
        });
    }
    // An episode belongs to its show, however it was queued.
    if crate::podcasts::is_episode_item(item) {
        let ep = crate::podcasts::store::get_episode(&db.podcasts_conn(), &item.video_id)?;
        let show = ep.show_id.filter(|s| !s.is_empty())?;
        // stream: audiobooks — a book's chapter belongs to the book.
        if let Some(book) = crate::audiobooks::book_id(&show) {
            return Some(PlayContext {
                kind: "audiobook".into(),
                id: book.to_owned(),
                title: if ep.show_title.is_empty() { item.artists.clone() } else { ep.show_title },
                subtitle: Some("Audiobook".into()),
                thumbnail: ep.thumbnail.or_else(|| item.thumbnail.clone()),
                params: None,
            });
        }
        return Some(PlayContext {
            kind: "podcast".into(),
            id: show,
            title: if ep.show_title.is_empty() { item.artists.clone() } else { ep.show_title },
            subtitle: Some("Podcast".into()),
            thumbnail: ep.thumbnail.or_else(|| item.thumbnail.clone()),
            params: None,
        });
    }
    None
}

pub fn resolve(
    db: &Db,
    source_id: Option<&str>,
    source_name: Option<&str>,
    item: &SongItem,
) -> Option<PlayContext> {
    lookup(db, source_id, source_name)
        .or_else(|| derive(db, source_id, source_name, item))
        .filter(PlayContext::valid)
}

fn stamp(c: &Connection, ctx: &PlayContext, now: i64) {
    let _ = crate::library::note_played(c, &ctx.id, Some(&ctx.kind), Some(&ctx.card()), now);
}

/// A queue was just started (`play_playlist`, `start_radio`): Recents moves at once, as in
/// Spotify, rather than waiting for the first listen.
pub fn started(
    db: &Db,
    app: Option<&dyn crate::host::Host>,
    source_id: Option<&str>,
    source_name: Option<&str>,
    given: Option<PlayContext>,
    first: Option<&SongItem>,
) {
    if let Some(g) = given.as_ref() {
        register(db, source_id, source_name, g);
    }
    let ctx = match first {
        Some(item) => resolve(db, source_id, source_name, item),
        None => lookup(db, source_id, source_name),
    };
    let Some(ctx) = ctx else { return };
    if crate::library::is_private(db) {
        return;
    }
    db.with_conn(|c| stamp(c, &ctx, super::now_secs()));
    emit_played(app);
    // stream: magic-d4 — Home's shortcut tiles move at once too (desktop and iOS listen).
    if let Some(app) = app {
        crate::smart::home::emit_changed(app, "played", Some(&ctx));
    }
}

fn emit_played(app: Option<&dyn crate::host::Host>) {
    use crate::host::HostExt;
    if let Some(app) = app {
        let _ = app.emit("library-changed", serde_json::json!({ "scope": "played" }));
    }
}

/// A listen crossed the threshold (state.rs `on_position`, after the private-session check).
pub fn on_listen(
    db: &Db,
    app: Option<&dyn crate::host::Host>,
    item: &SongItem,
    source_id: Option<&str>,
    source_name: Option<&str>,
) -> Option<PlayContext> {
    let ctx = resolve(db, source_id, source_name, item)?;
    log_listen(db, item, &ctx, super::now_secs());
    emit_played(app);
    Some(ctx)
}

pub fn log_listen(db: &Db, item: &SongItem, ctx: &PlayContext, now: i64) {
    let json = serde_json::to_string(ctx).unwrap_or_default();
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO polish_listen_contexts(video_id, context_key, json, at)
             VALUES(?1, ?2, ?3, ?4)",
            params![item.video_id, ctx.key(), json, now],
        );
        stamp(c, ctx, now);
    });
}

/// Contexts with their listen count and last listen, most recent first.
pub fn recorded(db: &Db, limit: usize) -> Vec<(PlayContext, i64, i64)> {
    db.with_conn(|c| {
        let c_conn = c;
        let Ok(mut st) = c.prepare(
            "SELECT (SELECT json FROM polish_listen_contexts j WHERE j.context_key = l.context_key
                     ORDER BY j.at DESC LIMIT 1),
                    COUNT(*), MAX(at) AS last
             FROM polish_listen_contexts l GROUP BY context_key ORDER BY last DESC LIMIT ?1",
        ) else {
            return Vec::new();
        };
        st.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })
        .map(|rows| {
            rows.flatten()
                .filter_map(|(j, n, last)| {
                    let mut c: PlayContext = serde_json::from_str(&j).ok()?;
                    // Rows recorded before titles were required (a radio started with no name).
                    if !c.valid() {
                        return None;
                    }
                    c.live_art(c_conn);
                    Some((c, n, last))
                })
                .collect()
        })
        .unwrap_or_default()
    })
}

/// One played-from context on the Recently played page (both apps): the context, how often and
/// when it was last played from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentPlayContext {
    #[serde(flatten)]
    pub context: PlayContext,
    pub plays: i64,
    pub played_at: i64,
}

/// The Recently played page's grid (`recent_play_contexts`): [`recorded`], newest first.
pub fn recent_play_contexts(db: &Db, limit: Option<usize>) -> Vec<RecentPlayContext> {
    recorded(db, limit.unwrap_or(50))
        .into_iter()
        .map(|(context, plays, played_at)| RecentPlayContext { context, plays, played_at })
        .collect()
}

/// Home's contexts (Recent shortcuts, Jump back in): the recorded ones, plus album/artist contexts
/// derived (smart `contexts`) only for songs whose latest listen had no recorded context, merged
/// and most recent first. Liked Songs is left out: Home pins it as its own first shortcut.
pub fn home_contexts(db: &Db, recent: &[Heard]) -> Vec<(ShelfItem, i64, i64)> {
    let last_ctx: HashMap<String, i64> = db.with_conn(|c| {
        let Ok(mut st) =
            c.prepare("SELECT video_id, MAX(at) FROM polish_listen_contexts GROUP BY video_id")
        else {
            return HashMap::new();
        };
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    });
    let loose: Vec<Heard> = recent
        .iter()
        .filter(|h| last_ctx.get(&h.song.video_id).is_none_or(|at| *at < h.last - 120))
        .cloned()
        .collect();
    let mut out: Vec<(ShelfItem, i64, i64)> = recorded(db, 200)
        .into_iter()
        .filter(|(c, _, _)| c.kind != "liked")
        .map(|(c, n, last)| (c.shelf_item(), n, last))
        .collect();
    let mut at: HashMap<String, usize> =
        out.iter().enumerate().map(|(i, (s, _, _))| (format!("{}:{}", s.kind, s.id), i)).collect();
    for (s, n, last) in crate::smart::home::contexts(&loose) {
        let key = format!("{}:{}", s.kind, s.id);
        match at.get(&key) {
            Some(&i) => {
                out[i].1 += n;
                out[i].2 = out[i].2.max(last);
            }
            None => {
                at.insert(key, out.len());
                out.push((s, n, last));
            }
        }
    }
    out.sort_by_key(|(_, _, last)| std::cmp::Reverse(*last));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, album: Option<(&str, &str)>) -> SongItem {
        SongItem {
            video_id: id.into(),
            title: format!("Song {id}"),
            artists: "Artist".into(),
            artist_id: Some("UCartist".into()),
            album_id: album.map(|a| a.0.into()),
            album: album.map(|a| a.1.into()),
            thumbnail: Some(format!("https://img/{id}")),
            duration: Some("3:00".into()),
            ..Default::default()
        }
    }

    fn listen(db: &Db, s: &SongItem, at: i64) {
        db.record_listen(&s.video_id, &serde_json::to_string(s).unwrap(), at, 180);
    }

    #[test]
    fn registered_context_wins_and_survives_by_source_key() {
        let db = crate::polish::test_db();
        let ctx = PlayContext {
            kind: "playlist".into(),
            id: "VLPLabc".into(),
            title: "Road trip".into(),
            thumbnail: Some("https://img/pl".into()),
            ..Default::default()
        };
        register(&db, Some("VLPLabc"), Some("Road trip"), &ctx);
        let got = resolve(&db, Some("VLPLabc"), Some("Road trip"), &song("a", None)).unwrap();
        assert_eq!(got, ctx);
        // A different queue name is a different queue.
        let other = resolve(&db, None, Some("Other"), &song("a", None));
        assert!(other.is_none());
    }

    #[test]
    fn derive_covers_liked_local_album_and_playlists() {
        let db = crate::polish::test_db();
        let liked = derive(&db, Some("VLLM"), Some("Liked Music"), &song("a", None)).unwrap();
        assert_eq!((liked.kind.as_str(), liked.id.as_str()), ("liked", "VLLM"));
        assert_eq!(liked.title, "Liked Songs");

        let p = db
            .with_conn(|c| crate::library::create_playlist(c, Some("Mine"), None, None, 1))
            .unwrap();
        let local = derive(&db, Some(&p.id), Some("Mine"), &song("a", None)).unwrap();
        assert_eq!((local.kind.as_str(), local.title.as_str()), ("playlist", "Mine"));
        assert_eq!(local.id, p.id);

        let s = song("b", Some(("MPREalb", "The Album")));
        let album = derive(&db, Some("OLAK5uy_xyz"), Some("The Album"), &s).unwrap();
        assert_eq!((album.kind.as_str(), album.id.as_str()), ("album", "MPREalb"));

        let pl = derive(&db, Some("PLxyz"), Some("Chill"), &song("c", None)).unwrap();
        assert_eq!(
            (pl.kind.as_str(), pl.id.as_str(), pl.title.as_str()),
            ("playlist", "VLPLxyz", "Chill")
        );

        assert!(derive(&db, None, Some("Search"), &song("d", None)).is_none());
    }

    #[test]
    fn listens_stamp_recents_and_feed_home_contexts() {
        let db = crate::polish::test_db();
        let now = super::super::now_secs();
        let pl = PlayContext {
            kind: "playlist".into(),
            id: "VLPLroad".into(),
            title: "Road trip".into(),
            ..Default::default()
        };
        // Two songs from a playlist (their albums must not show up as contexts)…
        for (i, id) in ["p1", "p2"].iter().enumerate() {
            let s = song(id, Some((&format!("MPRE{id}"), "Some album")));
            listen(&db, &s, now - 500 + i as i64);
            log_listen(&db, &s, &pl, now - 500 + i as i64);
        }
        // …and one heard from a search row, which falls back to its album.
        let loose = song("x", Some(("MPRElone", "Lone album")));
        listen(&db, &loose, now - 100);

        let recent = crate::smart::home::recent_songs(&db, 50);
        let ctx = home_contexts(&db, &recent);
        let keys: Vec<String> =
            ctx.iter().map(|(s, _, _)| format!("{}:{}", s.kind, s.id)).collect();
        assert_eq!(keys, ["album:MPRElone", "playlist:VLPLroad"]);
        assert_eq!(ctx[1].1, 2, "two listens from the playlist");

        let recents = db.with_conn(|c| crate::library::recent_contexts(c, 10));
        assert_eq!(recents[0].item_id, "VLPLroad");
        assert_eq!(recents[0].kind, "playlist");
        assert_eq!(recents[0].card.as_ref().unwrap()["title"], "Road trip");
    }

    #[test]
    fn liked_is_left_to_homes_own_shortcut() {
        let db = crate::polish::test_db();
        let liked = PlayContext { kind: "liked".into(), ..Default::default() }.normalized();
        let s = song("l", None);
        listen(&db, &s, super::super::now_secs());
        log_listen(&db, &s, &liked, super::super::now_secs());
        let recent = crate::smart::home::recent_songs(&db, 50);
        assert!(home_contexts(&db, &recent).iter().all(|(s, _, _)| s.kind != "liked"));
        // …but the library's Recents does get it.
        let recents = db.with_conn(|c| crate::library::recent_contexts(c, 10));
        assert_eq!(recents[0].item_id, "VLLM");
    }

    #[test]
    fn untitled_contexts_never_reach_home() {
        let db = crate::polish::test_db();
        let untitled = PlayContext {
            kind: "radio".into(),
            id: "dQw4w9WgXcQ".into(),
            subtitle: Some("Rick Astley".into()),
            ..Default::default()
        };
        assert!(!untitled.valid());
        let s = song("r", None);
        listen(&db, &s, super::super::now_secs());
        // A row written before titles were required is skipped on the way out.
        log_listen(&db, &s, &untitled, super::super::now_secs());
        assert!(recorded(&db, 10).is_empty());
        let recent = crate::smart::home::recent_songs(&db, 50);
        assert!(home_contexts(&db, &recent).iter().all(|(s, _, _)| !s.title.is_empty()));
    }

    // stream: tilefix — Home's "Huberman Lab" shortcut showed a broken image every launch: the
    // card kept the show's signed YouTube art, dead hours after it was recorded.
    #[test]
    fn a_show_card_never_carries_dead_youtube_art() {
        let db = crate::polish::test_db();
        db.with_conn(|c| crate::podcasts::store::init(c).unwrap());
        // The URL from the bug report, signed 2026-11-15 and long dead.
        let dead = "https://i.ytimg.com/pl_c/PLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW/studio_square_thumbnail.jpg?sqp=CLq15dcG-oaymwEKCKAEEKAEIABIWqLzl_8DBgiR84OgBg&rs=AMzJL3kpyYOtkgEvOZJkYW5Yo5CQjDhz4A";
        let show = PlayContext {
            kind: "podcast".into(),
            id: "MPSPPLPNW".into(),
            title: "Huberman Lab".into(),
            subtitle: Some("Andrew Huberman".into()),
            thumbnail: Some(dead.into()),
            ..Default::default()
        };
        let s = song("ep", None);
        let now = super::super::now_secs();
        listen(&db, &s, now);
        log_listen(&db, &s, &show, now);
        let tile = |db: &Db| {
            let recent = crate::smart::home::recent_songs(db, 50);
            home_contexts(db, &recent).into_iter().find(|(s, _, _)| s.kind == "podcast").unwrap().0
        };
        let recents = |db: &Db| db.with_conn(|c| crate::library::recent_contexts(c, 10));
        // Nothing better known: no URL, so the UI draws its placeholder tile.
        assert_eq!(tile(&db).thumbnail, None);
        assert_eq!(recorded(&db, 10)[0].0.thumbnail, None);
        assert!(recents(&db)[0].card.as_ref().unwrap().get("thumbnail").is_none());
        // Once a show fetch hands out fresh art, every reader uses it.
        db.with_conn(|c| crate::podcasts::store::remember_show_art(c, "MPSPPLPNW", Some("https://fresh/art.jpg")));
        assert_eq!(tile(&db).thumbnail.as_deref(), Some("https://fresh/art.jpg"));
        assert_eq!(
            recents(&db)[0].card.as_ref().unwrap()["thumbnail"].as_str(),
            Some("https://fresh/art.jpg")
        );
    }

    // stream: magic-d4 — pressing play on a playlist moves it to the first shortcut tile at once
    // (before any listen crosses the threshold), and says so with `home-changed`.
    struct Rec(std::sync::Mutex<Vec<(String, String)>>);
    impl crate::host::Host for Rec {
        fn emit_json(&self, event: &str, json: String) {
            self.0.lock().unwrap().push((event.to_owned(), json));
        }
        fn data_dir(&self) -> std::path::PathBuf {
            std::env::temp_dir()
        }
    }

    #[test]
    fn starting_a_context_moves_home_and_emits_home_changed() {
        let db = crate::polish::test_db();
        let now = super::super::now_secs();
        // Heard earlier: an album (two songs) and an artist, so there are tiles to jump ahead of.
        let a = song("a", Some(("MPREold", "Old album")));
        listen(&db, &a, now - 600);
        let b = SongItem { artist_id: Some("UCother".into()), ..song("b", None) };
        listen(&db, &b, now - 500);
        let road = PlayContext {
            kind: "playlist".into(),
            id: "VLPLroad".into(),
            title: "Road trip".into(),
            thumbnail: Some("https://img/road".into()),
            ..Default::default()
        };
        let host = Rec(Default::default());
        started(&db, Some(&host), Some("VLPLroad"), Some("Road trip"), Some(road.clone()), None);

        let events = host.0.lock().unwrap().clone();
        let home: Vec<&(String, String)> = events.iter().filter(|(e, _)| e == "home-changed").collect();
        assert_eq!(home.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&home[0].1).unwrap();
        assert_eq!(payload["reason"], "played");
        assert_eq!(payload["context"]["id"], "VLPLroad");
        assert_eq!(payload["context"]["kind"], "playlist");
        assert!(events.iter().any(|(e, j)| e == "library-changed" && j.contains("played")));

        // Home's first context is the playlist now, with no listen from it yet.
        let shelves = crate::smart::home::local_shelves(
            &db,
            &crate::smart::taste::profile(&db),
            &crate::smart::library::Filter::load(&db),
        );
        let tiles = &shelves.iter().find(|s| s.id == "shortcuts").unwrap().items;
        assert_eq!((tiles[0].kind.as_str(), tiles[0].id.as_str()), ("playlist", "VLPLroad"));
        assert_eq!(tiles[0].title, "Road trip");
        // Pressing play on the old album again moves it back in front.
        let alb = PlayContext {
            kind: "album".into(),
            id: "MPREold".into(),
            title: "Old album".into(),
            ..Default::default()
        };
        // Same second as the playlist: make the album's start strictly later.
        db.with_conn(|c| stamp(c, &alb, now + 5));
        let shelves = crate::smart::home::local_shelves(
            &db,
            &crate::smart::taste::profile(&db),
            &crate::smart::library::Filter::load(&db),
        );
        let tiles = &shelves.iter().find(|s| s.id == "shortcuts").unwrap().items;
        let ids: Vec<&str> = tiles.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(&ids[..2], ["MPREold", "VLPLroad"]);
        assert_eq!(ids.iter().filter(|i| **i == "MPREold").count(), 1, "no duplicate tile");
    }

    #[test]
    fn a_private_session_start_moves_nothing() {
        let db = crate::polish::test_db();
        db.with_conn(|c| crate::library::set_private_session(c, true, super::super::now_secs()).unwrap());
        let host = Rec(Default::default());
        let ctx = PlayContext { kind: "album".into(), id: "MPREx".into(), title: "X".into(), ..Default::default() };
        started(&db, Some(&host), Some("x"), Some("X"), Some(ctx), None);
        assert!(host.0.lock().unwrap().iter().all(|(e, _)| e != "home-changed"));
    }
}
