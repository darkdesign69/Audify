//! Explicit flags (research 06 §A rule 3). YouTube Music badges songs on search, album and
//! artist rows, but `/next` rows (radio, autoplay) carry no badge. So the flag comes from:
//!
//! 1. the row's own badge (`SongItem::explicit` / `BrowseItem::explicit`), remembered here by
//!    videoId whenever a badged list passes through the smart layer ([`remember_rows`]);
//! 2. a Deezer lookup by title + artist (+ duration ±3 s) for rows still unknown, cached forever
//!    (`api.deezer.com/search`, no key; `explicit_lyrics`). Best-effort with a short timeout.
//!    Unknown after both counts as not explicit (Metrolist/YTM behaviour).

use std::collections::HashMap;
use std::time::Duration;

use innertube::{BrowseItem, SongItem};
use serde::Deserialize;

use crate::db::Db;
use crate::identity::{self, Track};

/// Every flag learned so far.
pub fn known(db: &Db) -> HashMap<String, bool> {
    db.with_conn(|c| {
        let Ok(mut st) =
            c.prepare("SELECT video_id, explicit FROM smart_explicit WHERE explicit IS NOT NULL")
        else {
            return HashMap::new();
        };
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    })
}

fn put(db: &Db, video_id: &str, explicit: Option<bool>, source: &str) {
    db.with_conn(|c| {
        let _ = c.execute(
            "INSERT INTO smart_explicit(video_id, explicit, source, at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(video_id) DO UPDATE SET explicit = excluded.explicit,
                 source = excluded.source, at = excluded.at
             WHERE smart_explicit.source != 'youtube' OR excluded.source = 'youtube'",
            rusqlite::params![video_id, explicit.map(i64::from), source, super::now_secs()],
        );
    });
}

/// Remember badges from a list YouTube badges reliably (search songs, album and artist rows).
/// Only `true` is trusted from rows of unknown provenance; `badged` says the list is one whose
/// rows always carry the badge, so a missing badge means "clean".
pub fn remember_rows(db: &Db, items: &[SongItem], badged: bool) {
    for s in items {
        if s.explicit || (badged && !s.is_video) {
            put(db, &s.video_id, Some(s.explicit), "youtube");
        }
    }
}

pub fn remember_cards(db: &Db, items: &[BrowseItem]) {
    for c in items.iter().filter(|c| c.kind == "song" && c.explicit) {
        put(db, &c.id, Some(true), "youtube");
    }
}

#[derive(Deserialize)]
struct DzSearch {
    #[serde(default)]
    data: Vec<DzTrack>,
}

#[derive(Deserialize)]
struct DzTrack {
    title: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    explicit_lyrics: bool,
    artist: DzArtist,
}

#[derive(Deserialize)]
struct DzArtist {
    name: String,
}

/// Pick Deezer's answer for `s` out of a search response: same work (title + primary artist,
/// identity rules) and, when both have one, a length within 3 s.
fn pick(s: &SongItem, resp: &DzSearch) -> Option<bool> {
    let want = Track::from_song(s);
    let wk = identity::work_key(&want);
    resp.data.iter().find_map(|d| {
        let cand = Track {
            title: d.title.clone(),
            artist_line: d.artist.name.clone(),
            artists: vec![d.artist.name.clone()],
            duration: d.duration,
            ..Default::default()
        };
        let ck = identity::work_key(&cand);
        let same_len = match (want.duration, d.duration) {
            (Some(a), Some(b)) => (a - b).abs() <= 3.0,
            _ => true,
        };
        (ck.base == wk.base && ck.version == wk.version && same_len && {
            let a = &wk.artist;
            let b = &ck.artist;
            a == b || strsim::jaro_winkler(a, b) >= 0.92
        })
        .then_some(d.explicit_lyrics)
    })
}

/// Look one row up on Deezer and cache the answer (a miss is cached as unknown, so it isn't asked
/// again). Returns the flag when Deezer knew it.
pub async fn lookup(db: &Db, s: &SongItem) -> Option<bool> {
    let p = identity::parse(&s.title, &s.artists, s.album.as_deref());
    let artist = identity::primary_artist(&s.artists);
    if p.base.is_empty() || artist.is_empty() || crate::local::is_local_song(&s.video_id) {
        return None;
    }
    let q = format!("artist:\"{}\" track:\"{}\"", artist, p.base_raw.replace('"', ""));
    let url = format!("https://api.deezer.com/search?limit=10&q={}", urlencoding::encode(&q));
    let resp = crate::http::client()
        .get(url)
        .header("User-Agent", concat!("Encore/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?
        .json::<DzSearch>()
        .await
        .ok()?;
    let flag = pick(s, &resp);
    put(db, &s.video_id, flag, "deezer");
    flag
}

/// Fill in unknown flags on `items` (at most `max` lookups, a few at a time), learning them into
/// `filter`. Rows already asked once (cached as unknown) are not asked again.
pub async fn resolve_unknown(
    db: &Db,
    items: &[SongItem],
    filter: &mut super::library::Filter,
    max: usize,
) {
    let asked: std::collections::HashSet<String> = db.with_conn(|c| {
        let Ok(mut st) = c.prepare("SELECT video_id FROM smart_explicit") else {
            return Default::default();
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    });
    let todo: Vec<&SongItem> = items
        .iter()
        .filter(|s| filter.explicit_unknown(s) && !asked.contains(&s.video_id))
        .take(max)
        .collect();
    for chunk in todo.chunks(4) {
        let flags = futures_util::future::join_all(chunk.iter().map(|s| lookup(db, s))).await;
        for (s, f) in chunk.iter().zip(flags) {
            if let Some(f) = f {
                filter.learn_explicit(&s.video_id, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    #[test]
    fn deezer_pick_matches_title_artist_and_length() {
        let resp: DzSearch = serde_json::from_str(
            r#"{"data":[
                {"title":"HUMBLE. (Karaoke)","duration":177,"explicit_lyrics":false,"artist":{"name":"Karaoke Crew"}},
                {"title":"HUMBLE.","duration":177,"explicit_lyrics":true,"artist":{"name":"Kendrick Lamar"}}
            ]}"#,
        )
        .unwrap();
        let s = song("h", "HUMBLE.", "Kendrick Lamar", None, "2:57");
        assert_eq!(pick(&s, &resp), Some(true));
        let other = song("x", "HUMBLE.", "Kendrick Lamar", None, "4:10");
        assert_eq!(pick(&other, &resp), None);
    }

    #[test]
    fn badges_are_remembered_and_youtube_wins_over_deezer() {
        let db = super::super::test_db();
        let mut e = song("e", "X", "Y", None, "3:00");
        e.explicit = true;
        remember_rows(&db, &[e, song("c", "Z", "Y", None, "3:00")], true);
        put(&db, "e", Some(false), "deezer");
        let k = known(&db);
        assert_eq!(k.get("e"), Some(&true));
        assert_eq!(k.get("c"), Some(&false));
    }
}
