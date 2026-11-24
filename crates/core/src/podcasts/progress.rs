//! The playing episode: resume position, played state, SponsorBlock skips.
//!
//! Driven by four one-line hooks in `state.rs` (marked `stream: podcasts`):
//! - `emit_now_playing` → [`track_started`]: every track change, user-started or gapless.
//! - `on_position` → [`on_tick`]: mpv's position ticks. Returns a seek target for SponsorBlock.
//! - `on_duration` → [`on_duration`].
//! - `flush_position` (pause, quit) → [`flush`].
//! and `start_current` asks [`resume_position`] for the `start=` offset.
//!
//! State is a std `Mutex` private to this module, so a tick costs one uncontended lock and,
//! every [`rules::SAVE_EVERY_SECS`], one small SQLite write — never the app's queue lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use innertube::SongItem;
use crate::host::HostExt;

use super::sponsorblock::{self, Segment};
use super::{is_episode_item, is_rss_id, rules, store, Episode, Progress};
use crate::db::Db;

struct Ctx {
    db: Arc<Db>,
}

static CTX: OnceLock<Ctx> = OnceLock::new();
static CUR: Mutex<Option<Current>> = Mutex::new(None);
/// We turned crossfade off for an episode and owe the user's setting back.
static CROSSFADE_HELD: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
struct Current {
    id: String,
    youtube: bool,
    duration: f64,
    last_pos: f64,
    saved_pos: f64,
    segments: Vec<Segment>,
    skipped: Vec<bool>,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent<'a> {
    id: &'a str,
    progress: Progress,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SkipEvent<'a> {
    id: &'a str,
    category: &'a str,
    from: f64,
    to: f64,
}

// ── stream: polish ── crossfade next to an episode (into it as well as out of it).
/// Hold crossfade off (`speech`: the playing track or the one primed next is an episode), or
/// release the hold. True when the hold changed, i.e. the caller should re-apply crossfade.
pub fn hold_crossfade(speech: bool) -> bool {
    CROSSFADE_HELD.swap(speech, Ordering::SeqCst) != speech
}

/// Crossfade is being held off for an episode; `AppState::apply_crossfade` honours this.
pub fn crossfade_held() -> bool {
    CROSSFADE_HELD.load(Ordering::SeqCst)
}
// ── end stream: polish ──

pub fn init(db: Arc<Db>) {
    let _ = CTX.set(Ctx { db });
}

fn ctx() -> Option<&'static Ctx> {
    CTX.get()
}

/// Where to start `video_id`, from its saved progress. `None` for music and for episodes that
/// are unstarted or finished.
pub fn resume_position(video_id: &str) -> Option<f64> {
    let c = ctx()?;
    let p = store::progress(&c.db.podcasts_conn(), video_id)?;
    rules::resume_from(p.position_secs, p.duration_secs, p.played)
}

/// A track started playing (any path). Settles the previous episode and, for an episode, starts
/// tracking this one. Returns the episode's resume position (the caller seeks to it only when
/// the track started without a `start=` offset, i.e. a gapless advance).
pub fn track_started(item: &SongItem) -> Option<f64> {
    let c = ctx()?;
    let prev = CUR.lock().unwrap().take();
    if let Some(p) = prev {
        if p.id != item.video_id {
            save(c, &p.id, p.last_pos, p.duration);
        }
    }
    if !is_episode_item(item) {
        if CROSSFADE_HELD.swap(false, Ordering::SeqCst) {
            if let Some(st) = crate::state::global() {
                crate::rt::spawn(async move { st.apply_crossfade().await });
            }
        }
        return None;
    }
    // Speech: no crossfade into or out of the next item while an episode plays.
    if let Some(st) = crate::state::global() {
        st.player.set_crossfade(None);
        CROSSFADE_HELD.store(true, Ordering::SeqCst);
    }
    let duration = crate::podcasts::youtube::parse_duration_text(item.duration.as_deref().unwrap_or(""))
        .map(|d| d as f64)
        .unwrap_or(0.0);
    // Make sure Continue listening can show it even when it was played from a search row.
    let conn = c.db.podcasts_conn();
    if store::get_episode(&conn, &item.video_id).is_none() {
        let ep = Episode {
            id: item.video_id.clone(),
            show_title: item.artists.clone(),
            title: item.title.clone(),
            duration_secs: (duration > 0.0).then_some(duration as i64),
            thumbnail: item.thumbnail.clone(),
            video_id: (!is_rss_id(&item.video_id)).then(|| item.video_id.clone()),
            enclosure_url: super::rss_url(&item.video_id).map(str::to_owned),
            ..Default::default()
        };
        let _ = store::upsert_episodes(&conn, &[ep]);
    }
    let saved = store::progress(&conn, &item.video_id);
    drop(conn);
    let youtube = !is_rss_id(&item.video_id);
    *CUR.lock().unwrap() = Some(Current {
        id: item.video_id.clone(),
        youtube,
        duration: saved.map(|p| p.duration_secs).filter(|d| *d > 0.0).unwrap_or(duration),
        last_pos: 0.0,
        saved_pos: 0.0,
        ..Default::default()
    });
    if youtube && sponsorblock::enabled(&c.db) {
        let id = item.video_id.clone();
        let db = c.db.clone();
        crate::rt::spawn(async move {
            let segs = sponsorblock::segments(&db, &id).await;
            if segs.is_empty() {
                return;
            }
            let mut cur = CUR.lock().unwrap();
            if let Some(cur) = cur.as_mut().filter(|c| c.id == id) {
                cur.skipped = vec![false; segs.len()];
                cur.segments = segs;
            }
        });
    }
    saved.and_then(|p| rules::resume_from(p.position_secs, p.duration_secs, p.played))
}

/// A position tick. Returns where to seek when a SponsorBlock segment starts.
pub fn on_tick(pos: f64) -> Option<f64> {
    let c = ctx()?;
    let (save_now, skip) = {
        let mut guard = CUR.lock().unwrap();
        let cur = guard.as_mut()?;
        // A tick near zero right after a long way in is the next file starting before
        // `track_started` has run: it must not overwrite this episode's position.
        if pos < 1.0 && cur.last_pos > 10.0 {
            return None;
        }
        cur.last_pos = pos;
        let mut skip = None;
        if cur.youtube {
            if let Some(i) = sponsorblock::skip_at(&cur.segments, &cur.skipped, pos) {
                cur.skipped[i] = true;
                let s = &cur.segments[i];
                skip = Some((s.category.clone(), s.end));
            }
        }
        let save_now = ((pos - cur.saved_pos).abs() >= rules::SAVE_EVERY_SECS).then(|| {
            cur.saved_pos = pos;
            (cur.id.clone(), cur.duration)
        });
        let skip = skip.map(|(cat, to)| (cur.id.clone(), cat, to));
        (save_now, skip)
    };
    if let Some((id, dur)) = save_now {
        save(c, &id, pos, dur);
    }
    let (id, category, to) = skip?;
    emit("podcasts:skipped", SkipEvent { id: &id, category: &category, from: pos, to });
    Some(to)
}

pub fn on_duration(secs: f64) {
    if secs.is_finite() && secs > 0.0 {
        if let Some(cur) = CUR.lock().unwrap().as_mut() {
            cur.duration = secs;
        }
    }
}

/// Pause / stop / quit: write the exact position now.
pub fn flush() {
    let Some(c) = ctx() else { return };
    let snap = CUR.lock().unwrap().as_mut().map(|cur| {
        cur.saved_pos = cur.last_pos;
        (cur.id.clone(), cur.last_pos, cur.duration)
    });
    if let Some((id, pos, dur)) = snap {
        save(c, &id, pos, dur);
    }
}

fn save(c: &Ctx, id: &str, pos: f64, dur: f64) {
    if !(pos > 0.0) {
        return;
    }
    let res = store::save_position(&c.db.podcasts_conn(), id, pos, dur);
    match res {
        Ok(progress) => {
            emit("podcasts:progress", ProgressEvent { id, progress });
        }
        Err(e) => tracing::warn!("podcasts: saving position failed: {e}"),
    }
}

/// Send a progress/skip event through the running app's host (none before it has started).
fn emit<S: serde::Serialize>(event: &str, payload: S) {
    if let Some(st) = crate::state::global() {
        let _ = st.host.emit(event, payload);
    }
}
