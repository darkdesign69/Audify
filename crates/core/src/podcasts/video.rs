//! Video podcasts (stream video-pods): which picture goes with an episode, if any.
//!
//! The sound never changes. The picture is either:
//! - a **YouTube video**: the episode itself when it plays from YouTube, or the YouTube twin an RSS
//!   episode streams instead of its enclosure (`polish::podmatch`). Same upload as the sound, so
//!   the two line up exactly. On mpv hosts (iOS, Linux, Windows) the core adds it to the playing
//!   file (`AppState::attach_video`); the macOS `<video>` element fetches it through
//!   `videoproxy`, like a music video.
//! - a **video enclosure**: the RSS file playing *is* the video. mpv already has its picture track
//!   (it only needs `vid` on); the desktop `<video>` loads the same URL, muted.
//!
//! Spotify's rules, the music-video ones: no picture offline, in Offline Mode or on Data Saver;
//! a setting plays video podcasts as audio first (the "Switch to video" pill still offers it).

use innertube::SongItem;
use serde::Serialize;

use crate::db::Db;

/// `"true"`: video podcasts start on the audio side.
pub const AUDIO_ONLY: &str = "podcast_audio_only";
/// The iPhone's Data Saver switch (`SettingKey.dataSaver`), stored with the core's settings.
const DATA_SAVER: &str = "ios_data_saver";

/// The picture for one episode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeVideo {
    /// The YouTube video to show (the episode, or an RSS episode's YouTube twin).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    /// A video enclosure: the file the sound plays from carries the picture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Start on the video side. `false` when the user plays video podcasts as audio.
    pub auto: bool,
}

/// What [`pick`] needs to know about one episode.
#[derive(Debug, Default, Clone)]
pub struct Facts<'a> {
    /// The queue row's id: a YouTube videoId or `PODURL:<enclosure>`.
    pub id: &'a str,
    /// YouTube's or the feed's word that this is an episode.
    pub is_episode: bool,
    /// The RSS episode's YouTube twin, when matching is on and found one.
    pub twin: Option<String>,
    /// The stored RSS episode's enclosure is `video/*`.
    pub video_enclosure: bool,
    /// Offline, Offline Mode or Data Saver.
    pub blocked: bool,
    pub audio_only: bool,
}

/// The rule, without I/O.
pub fn pick(f: &Facts) -> Option<EpisodeVideo> {
    let rss = super::rss_url(f.id);
    if (!f.is_episode && rss.is_none()) || f.blocked || f.id.is_empty() {
        return None;
    }
    let auto = !f.audio_only;
    match rss {
        // A YouTube episode is a video upload: its own picture.
        None => Some(EpisodeVideo { video_id: Some(f.id.to_owned()), url: None, auto }),
        // An RSS episode heard from its YouTube twin shows the twin; the enclosure is not playing.
        Some(_) if f.twin.is_some() => Some(EpisodeVideo { video_id: f.twin.clone(), url: None, auto }),
        Some(url) if f.video_enclosure => Some(EpisodeVideo { video_id: None, url: Some(url.to_owned()), auto }),
        Some(_) => None,
    }
}

/// Offline, in Offline Mode, or on Data Saver: no picture at all, and nothing resolved for one.
pub fn blocked(db: &Db) -> bool {
    db.get_setting(DATA_SAVER).as_deref() == Some("true")
        || crate::downloads::offline()
}

pub fn audio_only(db: &Db) -> bool {
    db.get_setting(AUDIO_ONLY).as_deref() == Some("true")
}

/// The picture for `item` (a queue row), `None` for a song or an audio-only episode.
pub fn for_item(db: &Db, item: &SongItem) -> Option<EpisodeVideo> {
    for_id(db, &item.video_id, super::is_episode_item(item))
}

/// [`for_item`] by id. `is_episode` is the row's flag; an RSS id is an episode regardless.
pub fn for_id(db: &Db, id: &str, is_episode: bool) -> Option<EpisodeVideo> {
    let rss = super::is_rss_id(id);
    let stored = rss.then(|| super::store::get_episode(&db.podcasts_conn(), id)).flatten();
    // An audiobook chapter (stored under `abook:<id>`) is never a video podcast.
    if stored.as_ref().and_then(|e| e.show_id.as_deref()).and_then(crate::audiobooks::book_id).is_some() {
        return None;
    }
    let twin = rss
        .then(|| crate::polish::podmatch::enabled(db))
        .filter(|on| *on)
        .and_then(|_| crate::polish::podmatch::known_twin(db, id));
    let video_enclosure = twin.is_none() && stored.is_some_and(|e| e.is_video);
    pick(&Facts {
        id,
        is_episode,
        twin,
        video_enclosure,
        blocked: blocked(db),
        audio_only: audio_only(db),
    })
}

/// The `podcast_episode_video` command: the picture for the episode `id`. Both UIs ask when an
/// episode starts (after `now-playing`, so an RSS episode's twin is already known).
pub fn podcast_episode_video(db: &Db, id: &str) -> Option<EpisodeVideo> {
    for_id(db, id, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::podcasts::Episode;

    fn facts(id: &str) -> Facts<'_> {
        Facts { id, is_episode: true, ..Default::default() }
    }

    #[test]
    fn a_youtube_episode_shows_itself() {
        let v = pick(&facts("abcdefghijk")).unwrap();
        assert_eq!(v.video_id.as_deref(), Some("abcdefghijk"));
        assert_eq!(v.url, None);
        assert!(v.auto);
    }

    #[test]
    fn songs_have_no_episode_picture() {
        assert_eq!(pick(&Facts { is_episode: false, ..facts("abcdefghijk") }), None);
    }

    #[test]
    fn an_rss_episode_shows_its_youtube_twin_first() {
        let id = "PODURL:https://x/ep.mp4";
        let f = Facts { twin: Some("TWINTWINTWI".into()), video_enclosure: true, ..facts(id) };
        let v = pick(&f).unwrap();
        assert_eq!(v.video_id.as_deref(), Some("TWINTWINTWI"));
        assert_eq!(v.url, None);
    }

    #[test]
    fn a_video_enclosure_is_its_own_picture() {
        let id = "PODURL:https://x/ep.mp4";
        let v = pick(&Facts { video_enclosure: true, is_episode: false, ..facts(id) }).unwrap();
        assert_eq!(v.url.as_deref(), Some("https://x/ep.mp4"));
        assert_eq!(v.video_id, None);
        // An audio enclosure with no twin has nothing to show.
        assert_eq!(pick(&facts("PODURL:https://x/ep.mp3")), None);
    }

    #[test]
    fn offline_and_data_saver_mean_audio_only() {
        assert_eq!(pick(&Facts { blocked: true, ..facts("abcdefghijk") }), None);
        let id = "PODURL:https://x/ep.mp4";
        assert_eq!(pick(&Facts { blocked: true, video_enclosure: true, ..facts(id) }), None);
    }

    #[test]
    fn the_audio_only_setting_starts_on_audio_but_keeps_the_video() {
        let v = pick(&Facts { audio_only: true, ..facts("abcdefghijk") }).unwrap();
        assert!(!v.auto);
        assert_eq!(v.video_id.as_deref(), Some("abcdefghijk"));
    }

    #[test]
    fn settings_feed_the_rule() {
        let db = crate::polish::test_db();
        crate::podcasts::store::init(&db.podcasts_conn()).unwrap();
        let yt = for_id(&db, "abcdefghijk", true).unwrap();
        assert!(yt.auto);
        db.set_setting(AUDIO_ONLY, "true");
        assert!(!for_id(&db, "abcdefghijk", true).unwrap().auto);
        db.set_setting(DATA_SAVER, "true");
        assert_eq!(for_id(&db, "abcdefghijk", true), None);
        db.set_setting(DATA_SAVER, "false");
        // An unknown RSS episode (not stored, no twin) has no picture...
        let rss = "PODURL:https://x/a.mp3";
        assert_eq!(for_id(&db, rss, true), None);
        // ...until it is matched to its YouTube twin, which it then shows (unless matching is off).
        crate::polish::podmatch::remember(&db, rss, Some("TWINTWINTWI"), crate::db::now_secs());
        assert_eq!(for_id(&db, rss, true).unwrap().video_id.as_deref(), Some("TWINTWINTWI"));
        db.set_setting(crate::polish::podmatch::SETTING, "false");
        assert_eq!(for_id(&db, rss, true), None);
        db.set_setting(crate::polish::podmatch::SETTING, "true");
        // An audiobook chapter never shows a picture, even with a twin on record.
        let ch = Episode {
            id: rss.into(),
            show_id: Some(crate::audiobooks::show_id("book_librivox")),
            title: "Chapter 1".into(),
            ..Default::default()
        };
        crate::podcasts::store::upsert_episodes(&db.podcasts_conn(), &[ch]).unwrap();
        assert_eq!(for_id(&db, rss, true), None);
    }
}
