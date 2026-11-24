//! ── lane magic-d1 ── "Go to song radio" as a page (MAGIC-GAPS #1), plus the two playback entry
//! points the radio page and the "Open link" dialog need.
//!
//! Spotify's "Go to song radio" opens a browsable "<Song> Radio" playlist and leaves the music
//! alone; only its Play button replaces the queue. [`radio_tracks`] is that page's data: the same
//! seed resolution and finishing pass as `AppState::start_radio` (canonical audio seed, blocklist /
//! hidden / explicit filter, unheard-first + taste order), without touching the queue.
//!
//! ponytail: the kind → radio-playlist resolution is a copy of the head of `start_radio` (state.rs
//! belongs to another lane right now). Fold `start_radio` onto [`resolve`] when that lane merges.

use std::sync::Arc;

use innertube::SongItem;
use serde::Serialize;

use crate::state::{radio_title, AppState};

/// One radio, ready to show as a playlist page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioPage {
    /// "<seed> Radio".
    pub title: String,
    /// The radio playlist YouTube actually answered with (`RDAMVM…`, `RDAMPL…`, an artist's `RD…`).
    /// Played as the queue's source id, so Autoplay keeps going on this same radio.
    pub playlist_id: String,
    /// The seed song (song radios only), after the canonical swap to its audio track.
    pub seed: Option<SongItem>,
    /// The seed's art: the song cover, the album cover or the artist photo, when known.
    pub thumbnail: Option<String>,
    /// "With <artists>", Spotify's radio description, from the first few distinct artists.
    pub description: Option<String>,
    pub items: Vec<SongItem>,
}

/// Ids that have no YouTube item behind them, so no radio either.
fn radio_less(id: &str) -> bool {
    crate::local::is_local_song(id)
        || crate::podcasts::is_rss_id(id)
        || id.starts_with(crate::local::ALBUM_PREFIX)
        || id.starts_with(crate::local::ARTIST_PREFIX)
        || id == crate::state::ON_REPEAT_ID
        || crate::state::is_local_playlist(id)
}

/// The seed's (videoId?, radio playlist id, art, name) for a `kind`/`id` pair.
async fn resolve(
    state: &AppState,
    kind: &str,
    id: &str,
) -> Result<(Option<String>, String, Option<String>, Option<String>), String> {
    if radio_less(id) {
        return Err("This has no radio behind it.".into());
    }
    let client = state.clients.get(innertube::METADATA_CLIENT).ok_or("no metadata client")?;
    Ok(match kind {
        "song" => (Some(id.to_owned()), format!("RDAMVM{id}"), None, None),
        "playlist" => {
            let pl = crate::autoplay::context_radio(id).ok_or("This playlist has no radio.")?;
            (None, pl, None, None)
        }
        "album" => {
            let page = state.it.album(client, id).await.map_err(|e| e.to_string())?;
            let pl = page.playlist_id.clone().ok_or("This album has no radio.")?;
            let pl = crate::autoplay::context_radio(&pl).ok_or("This album has no radio.")?;
            (None, pl, page.thumbnail, page.title)
        }
        "artist" => {
            let page = state.it.artist(client, id).await.map_err(|e| e.to_string())?;
            match page.radio_playlist_id {
                Some(pl) => (None, pl, page.thumbnail, page.name),
                None => {
                    let top = page.top_songs.first().ok_or("This artist has no radio.")?;
                    let vid = top.video_id.clone();
                    (Some(vid.clone()), format!("RDAMVM{vid}"), page.thumbnail, page.name)
                }
            }
        }
        other => return Err(format!("unknown radio kind: {other}")),
    })
}

/// "With A, B, C and more": the first distinct artists after the seed's own, Spotify's wording.
pub fn describe(items: &[SongItem], seed_artist: Option<&str>) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    let seed_key = seed_artist.map(crate::identity::primary_artist);
    for s in items {
        let a = crate::smart::taste::display_artist(s);
        let a = a.trim();
        if a.is_empty() || Some(crate::identity::primary_artist(a)) == seed_key {
            continue;
        }
        if !seen.iter().any(|x| x.eq_ignore_ascii_case(a)) {
            seen.push(a.to_owned());
        }
        if seen.len() == 3 {
            break;
        }
    }
    match seen.len() {
        0 => None,
        _ => Some(format!("With {} and more", seen.join(", "))),
    }
}

/// The radio for `kind`/`id`, without playing anything.
pub async fn radio_tracks(
    state: &Arc<AppState>,
    kind: &str,
    id: &str,
    name: Option<String>,
) -> Result<RadioPage, String> {
    let (video_id, playlist_id, art, page_name) = resolve(state, kind, id).await?;
    let (mut items, mut seed_id) = state.fetch_radio(video_id.as_deref(), &playlist_id).await?;
    // A radio on a music video runs on its song (`start_radio` does the same).
    let mut video_id = video_id;
    let seed_row = video_id.as_ref().and_then(|v| items.iter().find(|i| &i.video_id == v).cloned());
    if let Some(row) = seed_row.as_ref() {
        if let Some(atv) = crate::canonical::audio_seed(state, row).await {
            if let Ok((page, s)) = state.fetch_radio(Some(&atv), &format!("RDAMVM{atv}")).await {
                items = page;
                seed_id = s;
                video_id = Some(atv);
            }
        }
    }
    let canon_seed = video_id.as_ref().and_then(|v| items.iter().find(|i| &i.video_id == v).cloned());
    let pending = crate::canonical::prepare_db(&state.db, &mut items, canon_seed.as_ref());
    drop(pending); // cache-only cleanup is enough for a page; the queue maps the rest when played
    crate::smart::recs::filter_radio(&state.db, &mut items, video_id.as_deref()).await;
    let seed_at = video_id.as_ref().and_then(|v| items.iter().position(|i| &i.video_id == v));
    // The seed opens the radio (Spotify lists it first); the rest is freshened like a real radio.
    if let Some(i) = seed_at {
        let s = items.remove(i);
        items.insert(0, s);
    }
    let from = usize::from(seed_at.is_some());
    state.freshen_radio(&mut items[from..]);
    let seed = seed_at.map(|_| items[0].clone());
    let seed_title = seed.as_ref().map(|s| s.title.as_str());
    let name = name.filter(|n| !n.trim().is_empty()).or(page_name);
    let title = radio_title(name.as_deref(), seed_title).unwrap_or_else(|| "Radio".into());
    let thumbnail = art.or_else(|| seed.as_ref().and_then(|s| s.thumbnail.clone()));
    let description = describe(&items[from..], seed.as_ref().map(|s| s.artists.as_str()));
    Ok(RadioPage { title, playlist_id: seed_id, seed, thumbnail, description, items })
}

/// A single pick that starts partway in: a pasted song link with `t=`. The offset rides on
/// `pending_seek` (handed to mpv's `loadfile` as `start=`), keyed on the video so it can't land
/// on anything else. Autoplay follows, exactly as for a search pick.
pub async fn play_single_at(
    state: &Arc<AppState>,
    item: SongItem,
    from: Option<String>,
    start: Option<f64>,
) -> Result<(), String> {
    if let Some(pos) = start.filter(|s| s.is_finite() && *s > 0.0) {
        *state.pending_seek.lock().unwrap() = Some((item.video_id.clone(), pos));
    }
    state.play_single(item, from).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, artist: &str) -> SongItem {
        SongItem { video_id: id.into(), title: id.into(), artists: artist.into(), ..Default::default() }
    }

    #[test]
    fn description_names_other_artists_once() {
        let items = vec![
            song("a", "Queen"),
            song("b", "Led Zeppelin"),
            song("c", "Led Zeppelin"),
            song("d", "AC/DC"),
            song("e", "Eagles"),
            song("f", "Toto"),
        ];
        assert_eq!(
            describe(&items, Some("Queen")).as_deref(),
            Some("With Led Zeppelin, AC/DC, Eagles and more")
        );
        assert_eq!(describe(&[song("a", "Queen")], Some("Queen")), None);
    }

    #[test]
    fn local_and_on_repeat_ids_have_no_radio() {
        assert!(radio_less(crate::state::ON_REPEAT_ID));
        assert!(radio_less("LOCALPLAYLIST:3"));
        assert!(!radio_less("BSTsnWoslP4"));
    }
}
