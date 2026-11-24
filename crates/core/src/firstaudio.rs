//! stream: firstaudio. Click-to-audible timing marks for the perf harness (docs/streams/perf.md,
//! docs/streams/firstaudio.md).
//!
//! Silent unless the app was started with `ENCORE_PERF` set: then every stage of a user's play
//! (resolve steps, mpv load, the audio proxy's first upstream byte, the first audible tick) logs a
//! `perf: fa:<stage> {"ms":…}` line, measured from the moment the play command reached the backend.
//! `scripts/perf-report.py` and the probe's own `audio:*` lines read them back.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ENCORE_PERF").is_some())
}

/// The play being timed.
#[derive(Clone)]
struct Play {
    t0: Instant,
    id: String,
    /// mpv was handed the new file.
    loaded: bool,
    /// A position at the start of the new file was seen since. Ticks of the outgoing track arrive
    /// after the load too, and must not count as the new one being audible.
    reset: bool,
}

static CURRENT: Mutex<Option<Play>> = Mutex::new(None);

/// A user asked to play `video_id`: start the clock.
pub fn start(video_id: &str) {
    if !enabled() {
        return;
    }
    if let Ok(mut c) = CURRENT.lock() {
        *c = Some(Play { t0: Instant::now(), id: video_id.to_owned(), loaded: false, reset: false });
    }
    log("start", 0.0, video_id);
}

/// One stage of the play in progress reached.
pub fn mark(stage: &str) {
    if !enabled() {
        return;
    }
    let Some(p) = CURRENT.lock().ok().and_then(|c| c.clone()) else { return };
    log(stage, p.t0.elapsed().as_secs_f64() * 1000.0, &p.id);
}

/// mpv was handed the timed play's file.
pub fn loaded() {
    if !enabled() {
        return;
    }
    if let Ok(mut c) = CURRENT.lock() {
        if let Some(p) = c.as_mut() {
            p.loaded = true;
        }
    }
    mark("loaded");
}

/// mpv reported a position: the first one past 50 ms of a timed play is the "audible" mark, and
/// ends the play's timing.
pub fn position(pos: f64) {
    if !enabled() {
        return;
    }
    let done = {
        let Ok(mut c) = CURRENT.lock() else { return };
        match c.as_mut() {
            Some(p) if p.loaded && pos <= 0.05 => {
                p.reset = true;
                None
            }
            Some(p) if p.loaded && p.reset => c.take(),
            _ => None,
        }
    };
    if let Some(p) = done {
        log("audible", p.t0.elapsed().as_secs_f64() * 1000.0, &p.id);
    }
}

fn log(stage: &str, ms: f64, id: &str) {
    tracing::info!(
        "perf: fa:{stage} {}",
        serde_json::json!({ "ms": (ms * 10.0).round() / 10.0, "videoId": id })
    );
}

// ── Play hints: resolve a track before it is clicked ─────────────────────────────────────────────
//
// ExoPlayer apps (Metrolist, InnerTune) pre-buffer what is likely to play next, and Spotify feels
// instant because the click never waits on a lookup. A hint here (a row hovered for a moment) runs
// the normal resolve at background priority, which fills the stream-URL cache, and warms the
// stream's first bytes in the audio proxy. The click then starts from the cached path.
//
// Every cold hint is a `/player` call, so they are rationed: one at a time, each track once per
// [`HINT_AGAIN`], and at most [`HINTS_PER_MINUTE`] network resolves a minute. Background priority
// puts them behind every user action and inside the reliability limiter's cooldowns.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// A track hinted once is not hinted again for this long (its URL is cached for hours anyway).
const HINT_AGAIN: Duration = Duration::from_secs(10 * 60);
/// Network resolves hints may spend per rolling minute. Fewer on the iPhone: a hint there is a
/// touch-down, which scrolling a list produces constantly, and each one costs two `/player` calls
/// (WEB_REMIX + VISIONOS) and two stream ranges. YouTube Music itself asks for `/player` only on
/// play, so a session that hints a dozen tracks a minute does not look like one person listening.
const HINTS_PER_MINUTE: usize = if cfg!(any(target_os = "ios", target_os = "android")) { 4 } else { 12 };

#[derive(Default)]
pub struct Hints {
    seen: HashMap<String, Instant>,
    spent: VecDeque<Instant>,
    /// When the hint in flight started.
    busy: Option<Instant>,
    /// YouTube refused a resolve: speculative work (hints, the gapless lookahead) holds off.
    pub backoff: Backoff,
}

/// A hint still running after this is stuck behind the background limiter; the next one may start
/// beside it rather than every later hover being dropped until it clears.
const STUCK: Duration = Duration::from_secs(3);

/// The first hold after a refused resolve; each refusal in a row doubles it, up to [`BACKOFF_MAX`].
const BACKOFF_FIRST: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(10 * 60);

/// After a resolve fails transiently (a bot check, a 403/429, no network: `ResolveError::
/// is_transient`), nothing the user did not ask for resolves for a while: hints and the gapless
/// lookahead wait, 60 s doubling to 10 min. Requests that keep coming while a source refuses us
/// are what keeps it refusing. A tap still resolves (once, with no auto-advance after it), and the
/// first resolve that works ends the hold.
#[derive(Debug)]
pub struct Backoff {
    until: Option<Instant>,
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { until: None, next: BACKOFF_FIRST }
    }
}

impl Backoff {
    /// Start (or lengthen) the hold; returns how long it is.
    pub fn refused(&mut self, now: Instant) -> Duration {
        let hold = self.next;
        self.until = Some(now + hold);
        self.next = (hold * 2).min(BACKOFF_MAX);
        hold
    }

    pub fn succeeded(&mut self) {
        *self = Backoff::default();
    }

    pub fn holds(&self, now: Instant) -> bool {
        self.until.is_some_and(|u| now < u)
    }
}

/// Record a network resolve's outcome in [`Hints::backoff`]. `transient`: `None` on success.
pub fn note_resolve(transient: Option<bool>) {
    let Ok(mut h) = HINTS.lock() else { return };
    match transient {
        None => h.backoff.succeeded(),
        Some(true) => {
            let hold = h.backoff.refused(Instant::now());
            tracing::warn!(?hold, "resolve refused: holding hints and lookahead");
        }
        Some(false) => {}
    }
}

/// Whether speculative resolves (the gapless lookahead) should wait. See [`Backoff`].
pub fn backing_off() -> bool {
    HINTS.lock().is_ok_and(|h| h.backoff.holds(Instant::now()))
}

impl Hints {
    /// Whether a hint for `id` may start now. Admitting it marks the ration as busy until
    /// [`Hints::done`].
    pub fn admit(&mut self, id: &str, now: Instant) -> bool {
        if self.backoff.holds(now) {
            return false;
        }
        if self.busy.is_some_and(|t| now.duration_since(t) < STUCK) {
            return false;
        }
        self.seen.retain(|_, t| now.duration_since(*t) < HINT_AGAIN);
        if self.seen.contains_key(id) {
            return false;
        }
        while self.spent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
            self.spent.pop_front();
        }
        if self.spent.len() >= HINTS_PER_MINUTE {
            return false;
        }
        self.seen.insert(id.to_owned(), now);
        self.busy = Some(now);
        true
    }

    /// The admitted hint finished; `network` when it cost a resolve (a cache hit is free).
    pub fn done(&mut self, network: bool, now: Instant) {
        self.busy = None;
        if network {
            self.spent.push_back(now);
        }
    }
}

pub static HINTS: std::sync::LazyLock<Mutex<Hints>> = std::sync::LazyLock::new(Default::default);

/// One network resolve per videoId at a time. A click that lands while that track's hint (a
/// hover, an iPhone touch-down, the restored track's warm-up) or lookahead is still resolving
/// waits for it and takes its cached URL, instead of starting a second `/player` round from
/// scratch that finishes no sooner. Hold the returned guard across the cache read and the resolve.
///
/// The wait is capped at [`GATE_WAIT`]: a hint runs in the background lane and can sit out a
/// throttle cooldown that a click would not, so past that the click goes ahead on its own (`None`).
pub async fn resolve_gate(video_id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    static GATES: Mutex<Option<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> =
        Mutex::new(None);
    let gate = {
        let mut g = GATES.lock().unwrap_or_else(|e| e.into_inner());
        let map = g.get_or_insert_with(HashMap::new);
        map.retain(|_, m| std::sync::Arc::strong_count(m) > 1);
        map.entry(video_id.to_owned()).or_default().clone()
    };
    tokio::time::timeout(GATE_WAIT, gate.lock_owned()).await.ok()
}

const GATE_WAIT: Duration = Duration::from_secs(3);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_holds_hints_back_doubling_to_ten_minutes_until_a_resolve_works() {
        let t = Instant::now();
        let mut h = Hints::default();
        assert_eq!(h.backoff.refused(t), Duration::from_secs(60));
        assert!(!h.admit("a", t + Duration::from_secs(59)), "no hints while refused");
        assert!(h.backoff.holds(t + Duration::from_secs(59)));
        assert!(!h.backoff.holds(t + Duration::from_secs(60)));
        assert_eq!(h.backoff.refused(t), Duration::from_secs(120));
        for _ in 0..6 {
            h.backoff.refused(t);
        }
        assert_eq!(h.backoff.refused(t), BACKOFF_MAX, "capped");
        h.backoff.succeeded();
        assert!(h.admit("a", t), "a working resolve ends the hold");
        assert_eq!(h.backoff.refused(t), BACKOFF_FIRST, "and the next run starts short again");
    }

    #[test]
    fn hints_run_one_at_a_time_and_once_per_track() {
        let mut h = Hints::default();
        let t = Instant::now();
        assert!(h.admit("a", t));
        assert!(!h.admit("b", t), "one in flight");
        h.done(true, t);
        assert!(!h.admit("a", t), "a was hinted already");
        assert!(h.admit("b", t));
        h.done(false, t);
        assert!(h.admit("a", t + HINT_AGAIN), "and again after the window");
    }

    #[test]
    fn a_stuck_hint_does_not_block_the_next_for_long() {
        let mut h = Hints::default();
        let t = Instant::now();
        assert!(h.admit("a", t));
        assert!(!h.admit("b", t + Duration::from_secs(1)));
        assert!(h.admit("b", t + STUCK), "a is stuck; b goes beside it");
    }

    #[test]
    fn network_hints_are_rationed_per_minute() {
        let mut h = Hints::default();
        let t = Instant::now();
        for i in 0..HINTS_PER_MINUTE {
            assert!(h.admit(&format!("n{i}"), t));
            h.done(true, t);
        }
        assert!(!h.admit("over", t));
        // Cache hits are free, but the ration is checked before knowing; a minute later it refills.
        assert!(h.admit("over", t + Duration::from_secs(60)));
    }

    #[tokio::test]
    async fn resolves_of_one_track_queue_and_other_tracks_do_not() {
        let held = resolve_gate("a").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), resolve_gate("b")).await.is_ok(),
            "another track is not held up"
        );
        let waiter = tokio::spawn(async { resolve_gate("a").await.is_some() });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "the same track waits for the resolve in flight");
        drop(held);
        assert!(tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_stuck_resolve_holds_the_next_one_back_only_so_long() {
        let _held = resolve_gate("s").await;
        assert!(resolve_gate("s").await.is_none(), "gave up waiting after GATE_WAIT");
    }
}
