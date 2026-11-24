//! Audiobooks (stream: audiobooks): LibriVox's public-domain recordings, the third Spotify content
//! type next to music and podcasts.
//!
//! Sources, both free, keyless and legal (every recording is public domain):
//! - **Internet Archive** (`archive.org/advancedsearch.php`, `archive.org/metadata/<id>`): the
//!   `librivoxaudio` collection (~22k books) holds every LibriVox book with its download count
//!   (our "popular" order), subjects (genre shelves), cover art and per-chapter MP3s. Fast.
//! - **LibriVox API** (`librivox.org/api/feed/audiobooks?id=…&extended=1`): the readers (our
//!   "narrator"), genres and a clean summary. Slower, so it is best effort with a short timeout.
//!
//! A book is played like a podcast show: each chapter is an [`Episode`] whose id is the podcasts
//! stream's `PODURL:` + MP3 URL, stored in `podcast_episodes` under the show id `abook:<id>`. So
//! mpv playback, resume per chapter, played state, the speed control and episode downloads all
//! come from the podcast path unchanged. Per-book resume is the chapter touched last ([`resume`]).

pub mod api;
pub mod ia;
pub mod store;

use serde::{Deserialize, Serialize};

use crate::podcasts::{Episode, Progress};

/// Show id prefix of a book's chapters in `podcast_episodes`.
pub const SHOW_PREFIX: &str = "abook:";

pub fn show_id(book_id: &str) -> String {
    format!("{SHOW_PREFIX}{book_id}")
}

/// The book behind a chapter's `show_id`.
pub fn book_id(show_id: &str) -> Option<&str> {
    show_id.strip_prefix(SHOW_PREFIX).filter(|s| !s.is_empty())
}

/// One audiobook. `id` is its Internet Archive identifier (`pride_and_prejudice_librivox`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Audiobook {
    pub id: String,
    pub title: String,
    pub author: String,
    /// "Kristen McQuillin", or "Various narrators" for a group recording.
    pub narrator: Option<String>,
    pub thumbnail: Option<String>,
    /// Plain text.
    pub description: Option<String>,
    pub duration_secs: Option<i64>,
    pub chapter_count: Option<u32>,
    pub genres: Vec<String>,
    pub language: Option<String>,
    /// In Your Library.
    pub saved: bool,
    /// 0..=1 of the whole book listened (saved / continue rows); `None` when never started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

/// Where Play continues: chapter `index` (its saved position resumes in the player).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Resume {
    pub index: usize,
    /// Anything listened yet: the button reads "Resume" instead of "Play".
    pub started: bool,
    /// Every chapter played: Play starts over.
    pub finished: bool,
}

/// The book page: header, chapters in reading order, and where Play goes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AudiobookPage {
    pub book: Audiobook,
    pub chapters: Vec<Episode>,
    pub resume: Resume,
    /// The book's LibriVox catalog page (Share).
    pub librivox_url: Option<String>,
}

/// Per-book resume from the chapters' progress: the chapter listened to last, or the one after
/// it when that one is finished. Unstarted books start at chapter 1; finished ones start over.
pub fn resume(chapters: &[Episode]) -> Resume {
    let started = |p: &Progress| p.played || p.position_secs > 0.0;
    let last = chapters
        .iter()
        .enumerate()
        .filter(|(_, e)| started(&e.progress) && e.progress.updated_at > 0)
        .max_by_key(|(_, e)| e.progress.updated_at);
    let Some((i, e)) = last else {
        return Resume::default();
    };
    if !e.progress.played {
        return Resume { index: i, started: true, finished: false };
    }
    match chapters.iter().skip(i + 1).position(|c| !c.progress.played) {
        Some(n) => Resume { index: i + 1 + n, started: true, finished: false },
        None => match chapters.iter().position(|c| !c.progress.played) {
            Some(n) => Resume { index: n, started: true, finished: false },
            None => Resume { index: 0, started: true, finished: true },
        },
    }
}

/// How much of the book is listened, 0..=1, from chapter durations; `None` before any listening.
pub fn book_progress(chapters: &[Episode]) -> Option<f64> {
    let mut total = 0.0;
    let mut heard = 0.0;
    let mut any = false;
    for c in chapters {
        let d = c.duration_secs.map(|d| d as f64).filter(|d| *d > 0.0).unwrap_or(c.progress.duration_secs);
        total += d;
        if c.progress.played {
            heard += d;
            any = true;
        } else if c.progress.position_secs > 0.0 {
            heard += c.progress.position_secs.min(d);
            any = true;
        }
    }
    (any && total > 0.0).then(|| (heard / total).clamp(0.0, 1.0))
}

/// Setup hook (boot.rs): the saved-books table.
pub fn init(db: &crate::db::Db) {
    if let Err(e) = store::init(&db.podcasts_conn()) {
        tracing::error!("audiobooks: cannot create tables: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(dur: i64, pos: f64, played: bool, at: i64) -> Episode {
        Episode {
            duration_secs: Some(dur),
            progress: Progress { position_secs: pos, duration_secs: dur as f64, played, updated_at: at, ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn show_ids_round_trip() {
        assert_eq!(book_id(&show_id("x_librivox")), Some("x_librivox"));
        assert_eq!(book_id("https://feed"), None);
        assert_eq!(book_id("abook:"), None);
    }

    #[test]
    fn resume_picks_the_chapter_listened_last() {
        let fresh = vec![ch(100, 0.0, false, 0), ch(100, 0.0, false, 0)];
        assert_eq!(resume(&fresh), Resume::default());
        // Halfway through chapter 3, listened after chapter 1 was finished.
        let mid = vec![ch(100, 0.0, true, 10), ch(100, 0.0, false, 0), ch(100, 50.0, false, 20)];
        assert_eq!(resume(&mid), Resume { index: 2, started: true, finished: false });
        // Chapter 1 just finished: the next unplayed one.
        let next = vec![ch(100, 0.0, true, 30), ch(100, 0.0, false, 0)];
        assert_eq!(resume(&next), Resume { index: 1, started: true, finished: false });
        // The last chapter finished but an earlier one was skipped: go back to it.
        let gap = vec![ch(100, 0.0, false, 0), ch(100, 0.0, true, 30)];
        assert_eq!(resume(&gap).index, 0);
        let done = vec![ch(100, 0.0, true, 30), ch(100, 0.0, true, 40)];
        assert_eq!(resume(&done), Resume { index: 0, started: true, finished: true });
    }

    #[test]
    fn progress_weights_chapters_by_length() {
        assert_eq!(book_progress(&[ch(100, 0.0, false, 0)]), None);
        let p = book_progress(&[ch(300, 0.0, true, 1), ch(100, 50.0, false, 2)]).unwrap();
        assert!((p - 350.0 / 400.0).abs() < 1e-9);
    }
}
