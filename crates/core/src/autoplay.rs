//! What plays next (stream: autoplay). Spotify's context rules, as pure functions the queue code
//! in `state.rs` calls:
//!
//! - **A single pick** (a search result, a Home or recommendation card, a song link) plays that
//!   song only. Nothing else from the list it was in follows it: the search list is "what you
//!   might have meant", not a queue. With the user queue empty, **Autoplay** carries on with songs
//!   like it: its YouTube Music radio, run on the studio recording ([`crate::canonical`]),
//!   reranked by taste, filtered (blocked, hidden, explicit), nothing heard in the last hour, at
//!   most two in a row by one artist, and never a different song that only shares the title.
//! - **A context** (album, playlist, Liked Songs, an artist's Popular, a show, the queue) plays
//!   to its end. Autoplay then continues from the context: the radios of its last
//!   [`CONTEXT_SEEDS`] songs fused by reciprocal rank, plus the context's own playlist radio when
//!   YouTube has one.
//! - **A podcast** never runs on into music.
//! - **Autoplay off** (Settings ▸ Playback): the queue simply ends.
//!
//! Benchmark: `docs/bench/recommendations-scripts/bench5.py`, report `docs/streams/autoplay.md`.

use std::collections::{HashMap, HashSet};

use innertube::SongItem;

use crate::identity::{self, Track};

/// Songs at the end of a context whose radios seed autoplay (bench5 task C: fusing the radios of
/// the last 3 scored NDCG@10 0.154 against 0.124 for the last song alone).
pub const CONTEXT_SEEDS: usize = 3;
/// Reciprocal-rank fusion constant, the one `recs::vote` uses.
const RRF_K: f64 = 10.0;
/// "Recently played": autoplay never brings back what was heard in the last hour.
pub const RECENT_SECS: i64 = 3600;

/// What autoplay should be seeded from when the queue is about to run out.
#[derive(Debug, Clone, PartialEq)]
pub struct Seeds {
    /// The context's own radio (`RDAMPL…` for an album or playlist), when YouTube has one.
    pub pinned: Option<String>,
    /// The songs to seed from, newest first: the last few YouTube songs of what played.
    pub songs: Vec<SongItem>,
}

/// Rows that can't seed a YouTube radio: episodes, local files.
fn seedable(s: &SongItem) -> bool {
    !crate::podcasts::is_episode_item(s) && !crate::local::is_local_song(&s.video_id)
}

/// Decide what autoplay continues from, or `None` when the queue should simply end: the setting
/// is off, or the queue ends on a podcast episode (Spotify never runs a show on into music), or
/// there is nothing YouTube knows to seed from (a queue of local files without a playlist radio).
pub fn seeds(items: &[SongItem], enabled: bool, pinned: Option<&str>) -> Option<Seeds> {
    if !enabled {
        return None;
    }
    let last = items.last()?;
    if crate::podcasts::is_episode_item(last) {
        return None;
    }
    let songs: Vec<SongItem> =
        items.iter().rev().filter(|s| seedable(s)).take(CONTEXT_SEEDS).cloned().collect();
    if songs.is_empty() && pinned.is_none() {
        return None;
    }
    Some(Seeds { pinned: pinned.map(str::to_owned), songs })
}

/// Reciprocal-rank fusion of several radio pages: a song high on several of them comes first.
/// Ties keep the first page's order. Deduped by id.
pub fn fuse(pages: &[Vec<SongItem>]) -> Vec<SongItem> {
    if pages.len() == 1 {
        let mut seen = HashSet::new();
        return pages[0].iter().filter(|s| seen.insert(s.video_id.clone())).cloned().collect();
    }
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut order: Vec<SongItem> = Vec::new();
    for page in pages {
        let mut here = HashSet::new();
        for (i, s) in page.iter().enumerate() {
            if !here.insert(s.video_id.as_str()) {
                continue;
            }
            let e = score.entry(s.video_id.clone()).or_insert_with(|| {
                order.push(s.clone());
                0.0
            });
            *e += 1.0 / (i as f64 + RRF_K);
        }
    }
    // Stable: equal scores keep first-seen order.
    order.sort_by(|a, b| score[&b.video_id].total_cmp(&score[&a.video_id]));
    order
}

fn artists_of(s: &SongItem) -> HashSet<String> {
    let mut out: HashSet<String> = identity::artist_names(&s.artists).into_iter().collect();
    out.extend(s.artist_runs.iter().map(|r| identity::norm(&r.text)).filter(|t| !t.is_empty()));
    out.extend(s.artist_id.iter().cloned());
    out.extend(s.artist_runs.iter().filter_map(|r| r.id.clone()));
    out
}

/// Drop candidates that only *look* related to a seed: a different artist's song with the seed's
/// title ("Queen" by someone else after a Queen song) or titled like the seed's artist (a song
/// called "Queen" after searching Queen). The seed's own artists' songs always stay.
pub fn drop_namesakes(items: &mut Vec<SongItem>, seeds: &[SongItem]) {
    let marks: Vec<(HashSet<String>, HashSet<String>)> = seeds
        .iter()
        .map(|s| {
            let artists = artists_of(s);
            let mut names: HashSet<String> = HashSet::new();
            let base = Track::from_song(s).parsed().base;
            if !base.is_empty() {
                names.insert(base);
            }
            names.extend(identity::artist_names(&s.artists));
            names.extend(s.artist_runs.iter().map(|r| identity::norm(&r.text)));
            names.retain(|n| !n.is_empty());
            (names, artists)
        })
        .collect();
    items.retain(|c| {
        let base = Track::from_song(c).parsed().base;
        let theirs = artists_of(c);
        !marks.iter().any(|(names, artists)| {
            names.contains(&base) && theirs.is_disjoint(artists)
        })
    });
}

/// Drop what was heard recently (`recent`: ids played in the last [`RECENT_SECS`]). If that would
/// leave nothing, the page is kept as it is: a repeat beats silence.
pub fn drop_recent(items: &mut Vec<SongItem>, recent: &HashSet<String>) {
    if items.iter().all(|s| recent.contains(&s.video_id)) {
        return;
    }
    items.retain(|s| !recent.contains(&s.video_id));
}

/// The radio playlist that continues a context, for the ids YouTube has one for: a playlist or
/// album (`PL…`, `OLAK5uy_…`, with or without `VL`) gets `RDAMPL<id>`, and a mix id (`RD…`) is
/// already one. Liked Songs (`VLLM`, kept on this machine), an artist's channel (`UC…`), Made For
/// You (`MFY_…`), local playlists and shows have none, and autoplay seeds from their last songs.
pub fn context_radio(source_id: &str) -> Option<String> {
    let id = source_id.strip_prefix("VL").unwrap_or(source_id);
    if id.starts_with("RD") {
        Some(id.to_owned())
    } else if id.starts_with("PL") || id.starts_with("OLAK5uy_") {
        Some(format!("RDAMPL{id}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use innertube::models::metadata::ArtistRun;

    fn song(id: &str, title: &str, artist: &str) -> SongItem {
        SongItem {
            video_id: id.into(),
            title: title.into(),
            artists: artist.into(),
            artist_runs: vec![ArtistRun { text: artist.into(), id: None }],
            ..Default::default()
        }
    }

    #[test]
    fn a_search_pick_seeds_autoplay_from_that_song() {
        let pick = song("bo", "Bohemian Rhapsody", "Queen");
        let s = seeds(&[pick.clone()], true, None).expect("autoplay continues a single pick");
        assert_eq!(s.pinned, None);
        assert_eq!(s.songs, vec![pick]);
    }

    #[test]
    fn an_album_end_seeds_from_its_radio_and_last_songs() {
        let album: Vec<SongItem> =
            (1..=10).map(|i| song(&format!("t{i}"), &format!("Track {i}"), "Queen")).collect();
        let pinned = context_radio("OLAK5uy_abc");
        let s = seeds(&album, true, pinned.as_deref()).expect("autoplay continues an album");
        assert_eq!(s.pinned.as_deref(), Some("RDAMPLOLAK5uy_abc"));
        let ids: Vec<&str> = s.songs.iter().map(|s| s.video_id.as_str()).collect();
        assert_eq!(ids, ["t10", "t9", "t8"], "newest first");
    }

    #[test]
    fn autoplay_off_means_the_queue_ends() {
        let pick = song("bo", "Bohemian Rhapsody", "Queen");
        assert_eq!(seeds(&[pick], false, None), None);
        assert_eq!(seeds(&[], true, None), None);
    }

    #[test]
    fn a_podcast_never_runs_on_into_music() {
        let mut ep = song("ep1", "Episode 12", "Some Show");
        ep.is_episode = true;
        let before = song("bo", "Bohemian Rhapsody", "Queen");
        assert_eq!(seeds(&[before, ep], true, None), None);
    }

    #[test]
    fn an_artist_name_collision_is_never_chosen() {
        // Searched "Queen", played Queen's "Bohemian Rhapsody". YouTube's page has a song called
        // "Queen" by someone else and a cover with the seed's title; neither is autoplay.
        let seed = song("bo", "Bohemian Rhapsody", "Queen");
        let mut page = vec![
            song("q1", "Queen", "Perfume Genius"),
            song("dm", "Don't Stop Me Now", "Queen"),
            song("bc", "Bohemian Rhapsody", "Panic! At The Disco"),
            song("ab", "Another One Bites the Dust", "Queen"),
            song("q2", "Queen (Official Video)", "Loren Gray"),
            song("ul", "Under Pressure", "Queen, David Bowie"),
            song("hs", "Hotel California", "Eagles"),
        ];
        drop_namesakes(&mut page, &[seed]);
        let ids: Vec<&str> = page.iter().map(|s| s.video_id.as_str()).collect();
        assert_eq!(ids, ["dm", "ab", "ul", "hs"]);
    }

    #[test]
    fn a_song_titled_like_its_own_artist_stays() {
        // Queen's seed: a Queen song can be titled anything, even "Queen".
        let seed = song("bo", "Bohemian Rhapsody", "Queen");
        let mut page = vec![song("own", "Queen", "Queen")];
        drop_namesakes(&mut page, &[seed]);
        assert_eq!(page.len(), 1);
    }

    #[test]
    fn fusion_puts_agreement_first() {
        let a = vec![song("x", "X", "A"), song("y", "Y", "B"), song("z", "Z", "C")];
        let b = vec![song("z", "Z", "C"), song("w", "W", "D")];
        let ids: Vec<String> = fuse(&[a, b]).into_iter().map(|s| s.video_id).collect();
        assert_eq!(ids, ["z", "x", "y", "w"], "ties keep first-seen order");
    }

    #[test]
    fn recent_songs_are_left_out_unless_nothing_else_is_left() {
        let mut page = vec![song("a", "A", "X"), song("b", "B", "Y")];
        let recent: HashSet<String> = ["a".to_string()].into();
        drop_recent(&mut page, &recent);
        assert_eq!(page.len(), 1);
        let mut all = vec![song("a", "A", "X")];
        drop_recent(&mut all, &recent);
        assert_eq!(all.len(), 1, "a repeat beats silence");
    }

    #[test]
    fn context_radio_only_for_ids_youtube_has_one_for() {
        assert_eq!(context_radio("VLPL123").as_deref(), Some("RDAMPLPL123"));
        assert_eq!(context_radio("OLAK5uy_x").as_deref(), Some("RDAMPLOLAK5uy_x"));
        assert_eq!(context_radio("VLRDCLAK5uy_x").as_deref(), Some("RDCLAK5uy_x"));
        assert_eq!(context_radio("VLLM"), None, "Liked Songs");
        assert_eq!(context_radio("UCabc"), None, "an artist's Popular");
        assert_eq!(context_radio("MFY_DAILY_1"), None);
    }
}
