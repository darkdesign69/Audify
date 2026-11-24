//! Keeping anonymous catalog calls up through Google's bot wall. See
//! `docs/streams/reliability.md` for the measurements behind every number here.
//!
//! What fails: in bursts, `music.youtube.com` answers a share of InnerTube calls (search, next,
//! browse, from every client identity) with **HTTP 403 and an HTML "Sorry... your computer or
//! network may be sending automated queries" page**. That page comes from Google's front end,
//! before InnerTube sees the request, and is decided per request against the IP's recent volume:
//! the next request a moment later usually goes through. Client version, visitorData, cookies,
//! HTTP/2 and the client identity (WEB_REMIX, IOS_MUSIC, ANDROID_MUSIC) made no measurable
//! difference. So the answer is the one yt-dlp and rustypipe use for flaky answers: retry with
//! jittered backoff, plus sending less (dedupe, a short cache) and pacing what nobody is waiting
//! for.
//!
//! Three pieces, all owned by [`crate::InnerTube`] and shared by its clones:
//! - [`Gate`]: a cap on requests in flight, with background work (Made For You, home shelves,
//!   canonical lookups) on a token bucket, a lower cap, and behind any waiting user action. A
//!   throttle answer also cools background traffic down for a few seconds.
//! - [`RetryPolicy`]: what to retry and how long to wait.
//! - [`ResponseCache`]: identical read requests in flight share one answer; anonymous reads are
//!   kept for a few minutes.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// Who is waiting for a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// A user action: a page, a search, a play. The default.
    Foreground,
    /// User-driven prefetch: a resolve started because the pointer rests on a row (hover play
    /// hints). The user may click any moment, so it goes ahead of generic background work and
    /// has slots of its own, but it never delays a real user action, and a miss costs nothing,
    /// so it gives up early instead of holding a slot.
    Speculative,
    /// Work nobody is watching: generators, enrichment lookups, cache warming.
    Background,
}

impl Priority {
    /// Higher goes first; a request may only join an in-flight one of equal or higher rank.
    fn rank(self) -> u8 {
        match self {
            Priority::Foreground => 2,
            Priority::Speculative => 1,
            Priority::Background => 0,
        }
    }
}

tokio::task_local! {
    static PRIORITY: Priority;
}

/// Run `f` with every InnerTube request it makes marked [`Priority::Background`].
///
/// A task-local, like [`crate::without_healing`]: `join_all` inside `f` inherits it,
/// `tokio::spawn` does not (a spawned task is foreground unless it wraps itself too).
pub async fn background<F: Future>(f: F) -> F::Output {
    PRIORITY.scope(Priority::Background, f).await
}

/// Run `f` with every InnerTube request it makes marked [`Priority::Speculative`]: user-driven
/// prefetch such as a hover play hint. Inherited and not inherited exactly like [`background`].
pub async fn speculative<F: Future>(f: F) -> F::Output {
    PRIORITY.scope(Priority::Speculative, f).await
}

/// The priority of the calling task.
pub fn current_priority() -> Priority {
    PRIORITY.try_with(|p| *p).unwrap_or(Priority::Foreground)
}

/// Tuning. The defaults are what the soak test passed with (`docs/streams/reliability.md`).
#[derive(Clone, Debug)]
pub struct Limits {
    /// Requests in flight, all priorities.
    pub max_in_flight: usize,
    /// Of those, background requests.
    pub max_background: usize,
    /// Of those, speculative requests. With `max_background + max_speculative < max_in_flight`,
    /// background work can never take a speculative request's slot, and neither can take the
    /// last ones a user action needs.
    pub max_speculative: usize,
    /// Background requests per second, sustained.
    pub background_rate: f64,
    /// Background requests allowed back to back.
    pub background_burst: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_in_flight: 8,
            max_background: 4,
            max_speculative: 2,
            background_rate: 8.0,
            background_burst: 12.0,
        }
    }
}

/// Counters for diagnostics and the soak test.
#[derive(Debug, Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub throttled: AtomicU64,
    pub server_errors: AtomicU64,
    pub retries: AtomicU64,
    pub gave_up: AtomicU64,
    pub deduped: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cut_short: AtomicU64,
}

/// A copy of [`Stats`] at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// HTTP requests actually sent (every attempt).
    pub requests: u64,
    /// Answers that were the bot wall (403 HTML or 429).
    pub throttled: u64,
    /// 5xx answers.
    pub server_errors: u64,
    /// Attempts after the first.
    pub retries: u64,
    /// Calls that failed after their last attempt.
    pub gave_up: u64,
    /// Calls that joined an identical one already in flight.
    pub deduped: u64,
    /// Calls answered from the short cache.
    pub cache_hits: u64,
    /// Attempts abandoned for answering too slowly (the bot wall's tarpit). Also in `throttled`.
    pub cut_short: u64,
}

impl Stats {
    pub fn snapshot(&self) -> StatsSnapshot {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StatsSnapshot {
            requests: g(&self.requests),
            throttled: g(&self.throttled),
            server_errors: g(&self.server_errors),
            retries: g(&self.retries),
            gave_up: g(&self.gave_up),
            deduped: g(&self.deduped),
            cache_hits: g(&self.cache_hits),
            cut_short: g(&self.cut_short),
        }
    }

    pub(crate) fn bump(a: &AtomicU64) {
        a.fetch_add(1, Ordering::Relaxed);
    }
}

// --- the gate -----------------------------------------------------------------------------------

struct Bucket {
    tokens: f64,
    at: Instant,
}

struct Cooldown {
    until: Instant,
    strikes: u32,
}

/// Admission for one request. See the module docs.
pub(crate) struct Gate {
    limits: Limits,
    total: Arc<Semaphore>,
    background: Arc<Semaphore>,
    speculative: Arc<Semaphore>,
    fg_waiting: AtomicUsize,
    spec_waiting: AtomicUsize,
    bucket: Mutex<Bucket>,
    cooldown: Mutex<Cooldown>,
    /// Moving average of how long a successful answer takes to start (ms). See
    /// [`Gate::header_deadline`].
    typical_ms: AtomicU64,
}

/// Held for the span of one HTTP attempt.
pub(crate) struct Permit {
    _total: OwnedSemaphorePermit,
    _lane: Option<OwnedSemaphorePermit>,
}

/// Decrements the foreground-waiting count however the wait ends (including cancellation).
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Gate {
    pub(crate) fn new(limits: Limits) -> Self {
        let now = Instant::now();
        Gate {
            total: Arc::new(Semaphore::new(limits.max_in_flight.max(1))),
            background: Arc::new(Semaphore::new(limits.max_background.max(1))),
            speculative: Arc::new(Semaphore::new(limits.max_speculative.max(1))),
            fg_waiting: AtomicUsize::new(0),
            spec_waiting: AtomicUsize::new(0),
            bucket: Mutex::new(Bucket { tokens: limits.background_burst, at: now }),
            cooldown: Mutex::new(Cooldown { until: now, strikes: 0 }),
            typical_ms: AtomicU64::new(600),
            limits,
        }
    }

    pub(crate) async fn acquire(&self, priority: Priority) -> Permit {
        match priority {
            Priority::Foreground => {
                self.fg_waiting.fetch_add(1, Ordering::AcqRel);
                let _waiting = Waiting(&self.fg_waiting);
                let total = self.total.clone().acquire_owned().await.expect("never closed");
                Permit { _total: total, _lane: None }
            }
            Priority::Speculative => {
                // Its own lane: background work cannot hold these slots. No token bucket and no
                // cooldown either; the caller rations hints itself (12 resolves a minute).
                let lane = self.speculative.clone().acquire_owned().await.expect("never closed");
                self.spec_waiting.fetch_add(1, Ordering::AcqRel);
                let _waiting = Waiting(&self.spec_waiting);
                loop {
                    if self.fg_waiting.load(Ordering::Acquire) > 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                    let Ok(total) = self.total.clone().try_acquire_owned() else {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    };
                    return Permit { _total: total, _lane: Some(lane) };
                }
            }
            Priority::Background => {
                let bg = self.background.clone().acquire_owned().await.expect("never closed");
                loop {
                    let cool = self.cooldown_left();
                    if !cool.is_zero() {
                        tokio::time::sleep(cool).await;
                        continue;
                    }
                    // A user action or a hint is queued for a slot: let it have the next one.
                    if self.fg_waiting.load(Ordering::Acquire) > 0
                        || self.spec_waiting.load(Ordering::Acquire) > 0
                    {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    }
                    // Polled rather than queued on the semaphore: a queued background waiter
                    // would be ahead of a user action that arrives after it (FIFO).
                    let Ok(total) = self.total.clone().try_acquire_owned() else {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        continue;
                    };
                    if let Err(wait) = self.take_token() {
                        drop(total);
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Permit { _total: total, _lane: Some(bg) };
                }
            }
        }
    }

    fn take_token(&self) -> Result<(), Duration> {
        let mut b = self.bucket.lock().unwrap();
        let now = Instant::now();
        let rate = self.limits.background_rate.max(0.01);
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * rate)
            .min(self.limits.background_burst.max(1.0));
        b.at = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / rate))
        }
    }

    fn cooldown_left(&self) -> Duration {
        self.cooldown.lock().unwrap().until.saturating_duration_since(Instant::now())
    }

    /// The bot wall answered: hold background traffic back, longer on each strike in a row.
    pub(crate) fn throttled(&self) {
        let mut c = self.cooldown.lock().unwrap();
        c.strikes = (c.strikes + 1).min(4);
        let pause = Duration::from_millis(250 << (c.strikes - 1)); // 0.25s … 2s
        c.until = c.until.max(Instant::now() + pause);
    }

    /// A request went through in `took` (to its headers): the strike count starts over.
    pub(crate) fn succeeded(&self, took: Duration) {
        self.cooldown.lock().unwrap().strikes = 0;
        let ms = took.as_millis().min(60_000) as u64;
        // EWMA, 1/8 weight; a race between two updates loses one sample, which is fine.
        let old = self.typical_ms.load(Ordering::Relaxed);
        self.typical_ms.store((old * 7 + ms) / 8, Ordering::Relaxed);
    }

    /// How long one attempt may wait for its answer to start before it is abandoned and resent.
    ///
    /// The bot wall does not answer at once: its 403 arrives after 5-10 s (measured: every one
    /// of 33 took 5.3 s or more, while every 200 took under 0.8 s). Waiting that out is most of
    /// what a throttled call used to cost, and it held an in-flight slot all the while. So an
    /// attempt gets four times the usual answer time, never under 2.5 s (a slow network or a
    /// proxy just raises the average) and never over 10 s. The last attempt waits as long as
    /// the client's own timeout, so a network that is merely slow still gets its answer.
    ///
    /// Work nobody waits on is cut much sooner (2.5× the usual answer time, 1.2–4 s), and its
    /// last attempt is bounded too (6×, 3–15 s): a tarpitted enrichment search used to hold a
    /// background slot for the full 2.5 s floor, and a handful of them clogged the lane that
    /// play hints used. A speculative attempt is never left uncapped: a hint that misses costs
    /// nothing, a hint holding a slot costs the next one.
    pub(crate) fn header_deadline(
        &self,
        priority: Priority,
        attempt: u32,
        attempts: u32,
    ) -> Option<Duration> {
        let typical = self.typical_ms.load(Ordering::Relaxed);
        let ms = |k: f64, lo: u64, hi: u64| {
            Some(Duration::from_millis(((typical as f64 * k) as u64).clamp(lo, hi)))
        };
        let last = attempt >= attempts;
        match priority {
            Priority::Foreground if last => None,
            Priority::Foreground => ms(4.0, 2_500, 10_000),
            Priority::Background if last => ms(6.0, 3_000, 15_000),
            Priority::Background | Priority::Speculative => ms(2.5, 1_200, 4_000),
        }
    }
}

// --- retries ------------------------------------------------------------------------------------

/// Why an attempt failed, as far as retrying goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Google's bot wall: 403 with an HTML body, or 429. Try again shortly.
    Throttled,
    /// 500/502/503/504. Try again.
    Server,
    /// Anything else: retrying gets the same answer.
    Final,
}

/// Classify a non-2xx answer. `html` is whether its content type was HTML: InnerTube itself
/// always answers JSON, so an HTML 403 is the front end's "Sorry" page and says nothing about
/// the request or the session.
pub(crate) fn classify(status: u16, html: bool) -> Failure {
    match status {
        429 => Failure::Throttled,
        403 if html => Failure::Throttled,
        500 | 502 | 503 | 504 => Failure::Server,
        _ => Failure::Final,
    }
}

/// How many attempts and how long between them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RetryPolicy {
    pub attempts: u32,
    pub base: Duration,
    pub cap: Duration,
}

impl RetryPolicy {
    pub(crate) fn for_priority(p: Priority) -> Self {
        match p {
            // Someone is looking at a spinner: up to ~5s of waiting in all, then an error.
            Priority::Foreground => RetryPolicy {
                attempts: 6,
                base: Duration::from_millis(250),
                cap: Duration::from_secs(2),
            },
            // A guess at the next click: a few quick tries, then let the click do it.
            Priority::Speculative => RetryPolicy {
                attempts: 3,
                base: Duration::from_millis(200),
                cap: Duration::from_millis(800),
            },
            // Nobody is: be patient and gentle.
            Priority::Background => RetryPolicy {
                attempts: 8,
                base: Duration::from_millis(600),
                cap: Duration::from_secs(10),
            },
        }
    }

    /// The wait before attempt `attempt + 1` (`attempt` ≥ 1 just failed): exponential, capped,
    /// with "equal jitter" (half fixed, half random) so a burst of failures does not come back in
    /// lockstep.
    pub(crate) fn delay(&self, attempt: u32, unit: f64) -> Duration {
        let exp = self.base.saturating_mul(1u32 << (attempt - 1).min(16)).min(self.cap);
        exp / 2 + exp.mul_f64(unit.clamp(0.0, 1.0) / 2.0)
    }
}

/// A number in [0, 1) that differs call to call. Not crypto; keeps `rand` out of the tree.
pub(crate) fn jitter_unit() -> f64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

// --- dedupe + cache -----------------------------------------------------------------------------

/// Read endpoints whose answers depend only on the request: safe to share and to keep a while.
/// Exact path match (before any query), so `browse/edit_playlist` and friends never qualify.
pub(crate) fn is_cacheable_read(path: &str) -> bool {
    let bare = path.split('?').next().unwrap_or(path);
    matches!(bare, "search" | "next" | "browse" | "music/get_search_suggestions")
}

pub(crate) const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_MAX: usize = 256;

type Shared = Option<Arc<serde_json::Value>>;

/// In-flight dedupe for identical reads, and a short cache for anonymous ones.
#[derive(Default)]
pub(crate) struct ResponseCache {
    /// key → (answer channel, leader's priority, leader id).
    in_flight: Mutex<HashMap<u64, (watch::Receiver<Shared>, Priority, u64)>>,
    next_id: AtomicU64,
    done: Mutex<HashMap<u64, (Instant, Arc<serde_json::Value>)>>,
}

/// What the caller should do about a request.
pub(crate) enum Lookup {
    /// Answered already.
    Hit(Arc<serde_json::Value>),
    /// The same request is in flight: wait on this.
    Join(watch::Receiver<Shared>),
    /// Send it, then call [`Leader::finish`].
    Lead(Leader),
}

/// The one caller actually sending a deduped request. Dropping it without `finish` (an error,
/// a cancellation) releases the joiners with nothing, and they send their own.
pub(crate) struct Leader {
    key: u64,
    id: u64,
    tx: watch::Sender<Shared>,
    cache: Arc<ResponseCache>,
    keep: bool,
}

impl Leader {
    pub(crate) fn finish(self, value: &serde_json::Value) {
        let v = Arc::new(value.clone());
        if self.keep {
            let mut done = self.cache.done.lock().unwrap();
            if done.len() >= CACHE_MAX {
                let now = Instant::now();
                done.retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
                if done.len() >= CACHE_MAX {
                    if let Some(oldest) =
                        done.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k)
                    {
                        done.remove(&oldest);
                    }
                }
            }
            done.insert(self.key, (Instant::now(), v.clone()));
        }
        let _ = self.tx.send(Some(v));
        // `Drop` takes it out of `in_flight`.
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        // Only our own entry: a user action may have taken the key over (see `lookup`).
        let mut in_flight = self.cache.in_flight.lock().unwrap();
        if in_flight.get(&self.key).is_some_and(|(_, _, id)| *id == self.id) {
            in_flight.remove(&self.key);
        }
    }
}

impl ResponseCache {
    pub(crate) fn key(url: &str, headers: &[(&str, &str)], body: &[u8]) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut h);
        headers.hash(&mut h);
        body.hash(&mut h);
        h.finish()
    }

    /// `keep`: also remember the answer for [`CACHE_TTL`] (anonymous requests only).
    ///
    /// A user action never joins a background request: that one may be paced, cooling down or
    /// in a long backoff. It sends its own and takes the key over, so later user actions join it
    /// instead; the background request still finishes for its own callers.
    pub(crate) fn lookup(self: &Arc<Self>, key: u64, keep: bool, priority: Priority) -> Lookup {
        if keep {
            let mut done = self.done.lock().unwrap();
            match done.get(&key) {
                Some((at, v)) if at.elapsed() < CACHE_TTL => return Lookup::Hit(v.clone()),
                Some(_) => {
                    done.remove(&key);
                }
                None => {}
            }
        }
        let mut in_flight = self.in_flight.lock().unwrap();
        if let Some((rx, leader, _)) = in_flight.get(&key) {
            if leader.rank() >= priority.rank() {
                return Lookup::Join(rx.clone());
            }
        }
        let (tx, rx) = watch::channel(None);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        in_flight.insert(key, (rx, priority, id));
        Lookup::Lead(Leader { key, id, tx, cache: self.clone(), keep })
    }

    /// Forget every kept answer (a sign-in or sign-out changes what anonymous means).
    pub(crate) fn clear(&self) {
        self.done.lock().unwrap().clear();
    }
}

/// Wait for a leader's answer: `None` if it failed or went away.
pub(crate) async fn join(mut rx: watch::Receiver<Shared>) -> Option<Arc<serde_json::Value>> {
    loop {
        if let Some(v) = rx.borrow_and_update().clone() {
            return Some(v);
        }
        if rx.changed().await.is_err() {
            return rx.borrow().clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bot_wall_and_server_errors_are_retried() {
        assert_eq!(classify(403, true), Failure::Throttled, "the HTML Sorry page");
        assert_eq!(classify(429, false), Failure::Throttled);
        assert_eq!(classify(403, false), Failure::Final, "a JSON 403 is InnerTube's own answer");
        assert_eq!(classify(401, false), Failure::Final);
        assert_eq!(classify(400, false), Failure::Final);
        assert_eq!(classify(404, true), Failure::Final);
        for s in [500, 502, 503, 504] {
            assert_eq!(classify(s, false), Failure::Server);
        }
    }

    #[test]
    fn backoff_grows_caps_and_jitters_within_bounds() {
        let p = RetryPolicy::for_priority(Priority::Foreground);
        assert_eq!(p.delay(1, 0.0), p.base / 2);
        assert_eq!(p.delay(1, 1.0), p.base);
        assert_eq!(p.delay(2, 0.0), p.base);
        assert_eq!(p.delay(30, 1.0), p.cap, "capped, and no overflow on a huge attempt");
        let total: Duration = (1..p.attempts).map(|a| p.delay(a, 1.0)).sum();
        assert!(total <= Duration::from_secs(6), "foreground gives up within ~6s: {total:?}");
        let bg = RetryPolicy::for_priority(Priority::Background);
        assert!(bg.attempts > p.attempts && bg.base > p.base);
    }

    #[test]
    fn jitter_is_a_unit_fraction_that_varies() {
        let xs: Vec<f64> = (0..100).map(|_| jitter_unit()).collect();
        assert!(xs.iter().all(|x| (0.0..1.0).contains(x)));
        assert!(xs.windows(2).any(|w| w[0] != w[1]));
    }

    #[test]
    fn writes_are_never_cacheable() {
        assert!(is_cacheable_read("search"));
        assert!(is_cacheable_read("next"));
        assert!(is_cacheable_read("browse"));
        assert!(is_cacheable_read("browse?ctoken=x&continuation=x&type=next"));
        for w in ["browse/edit_playlist", "player", "like/like", "playlist/create", "feedback"] {
            assert!(!is_cacheable_read(w), "{w}");
        }
    }

    #[tokio::test]
    async fn priority_is_task_local_and_defaults_to_foreground() {
        assert_eq!(current_priority(), Priority::Foreground);
        background(async {
            assert_eq!(current_priority(), Priority::Background);
            let joined = futures_util::future::join(async { current_priority() }, async {
                current_priority()
            })
            .await;
            assert_eq!(joined, (Priority::Background, Priority::Background));
            let spawned = tokio::spawn(async { current_priority() }).await.unwrap();
            assert_eq!(spawned, Priority::Foreground, "spawn does not inherit it");
        })
        .await;
    }

    #[tokio::test]
    async fn background_is_rate_limited_and_foreground_is_not() {
        tokio::time::pause();
        let gate = Gate::new(Limits {
            max_in_flight: 100,
            max_background: 100,
            max_speculative: 100,
            background_rate: 2.0,
            background_burst: 2.0,
        });
        let start = Instant::now();
        for _ in 0..6 {
            drop(gate.acquire(Priority::Background).await);
        }
        // Two from the burst, then four at 2/s.
        assert!(start.elapsed() >= Duration::from_millis(1900), "{:?}", start.elapsed());
        let start = Instant::now();
        for _ in 0..50 {
            drop(gate.acquire(Priority::Foreground).await);
        }
        assert!(start.elapsed() < Duration::from_millis(10));
    }

    #[tokio::test]
    async fn a_throttle_cools_background_down_and_not_foreground() {
        tokio::time::pause();
        let gate = Gate::new(Limits::default());
        gate.throttled();
        gate.throttled();
        let start = Instant::now();
        drop(gate.acquire(Priority::Foreground).await);
        assert!(start.elapsed() < Duration::from_millis(10));
        drop(gate.acquire(Priority::Background).await);
        assert!(start.elapsed() >= Duration::from_millis(500), "two strikes: 0.5s cooldown");
        gate.succeeded(Duration::from_millis(500));
        assert_eq!(gate.cooldown.lock().unwrap().strikes, 0);
    }

    #[test]
    fn the_attempt_deadline_follows_the_network_within_bounds() {
        let gate = Gate::new(Limits::default());
        assert_eq!(
            gate.header_deadline(FG, 1, 6),
            Some(Duration::from_millis(2_500)),
            "fast net: floor"
        );
        assert_eq!(gate.header_deadline(FG, 6, 6), None, "the last attempt waits it out");
        for _ in 0..100 {
            gate.succeeded(Duration::from_millis(1_500)); // a slow proxy
        }
        let d = gate.header_deadline(FG, 1, 6).unwrap();
        assert!(d >= Duration::from_millis(5_500) && d <= Duration::from_secs(6), "{d:?}");
        for _ in 0..100 {
            gate.succeeded(Duration::from_secs(30));
        }
        assert_eq!(gate.header_deadline(FG, 1, 6), Some(Duration::from_secs(10)), "ceiling");
    }

    #[tokio::test]
    async fn the_in_flight_cap_holds_and_a_waiting_user_action_goes_first() {
        tokio::time::pause();
        let gate = Arc::new(Gate::new(Limits {
            max_in_flight: 1,
            max_background: 4,
            max_speculative: 2,
            background_rate: 1000.0,
            background_burst: 1000.0,
        }));
        let held = gate.acquire(Priority::Background).await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let bg = {
            let (g, o) = (gate.clone(), order.clone());
            tokio::spawn(async move {
                let _p = g.acquire(Priority::Background).await;
                o.lock().unwrap().push("bg");
            })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        let fg = {
            let (g, o) = (gate.clone(), order.clone());
            tokio::spawn(async move {
                let _p = g.acquire(Priority::Foreground).await;
                o.lock().unwrap().push("fg");
                tokio::time::sleep(Duration::from_millis(50)).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(order.lock().unwrap().is_empty(), "nobody passes the cap");
        drop(held);
        fg.await.unwrap();
        bg.await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["fg", "bg"]);
    }

    #[tokio::test]
    async fn identical_reads_share_one_answer_and_failures_release_joiners() {
        let cache = Arc::new(ResponseCache::default());
        let Lookup::Lead(leader) = cache.lookup(1, true, FG) else { panic!("first leads") };
        let Lookup::Join(rx) = cache.lookup(1, true, FG) else { panic!("second joins") };
        let joined = tokio::spawn(join(rx));
        leader.finish(&serde_json::json!({"a": 1}));
        assert_eq!(*joined.await.unwrap().unwrap(), serde_json::json!({"a": 1}));
        assert!(matches!(cache.lookup(1, true, FG), Lookup::Hit(_)), "kept");

        // Not kept (signed in): shared while in flight only.
        let Lookup::Lead(leader) = cache.lookup(2, false, FG) else { panic!() };
        leader.finish(&serde_json::json!(2));
        assert!(matches!(cache.lookup(2, false, FG), Lookup::Lead(_)));

        // A leader that fails releases its joiners empty-handed.
        let Lookup::Lead(leader) = cache.lookup(3, true, FG) else { panic!() };
        let Lookup::Join(rx) = cache.lookup(3, true, FG) else { panic!() };
        drop(leader);
        assert!(join(rx).await.is_none());
        assert!(matches!(cache.lookup(3, true, FG), Lookup::Lead(_)), "nothing cached");

        cache.clear();
        assert!(matches!(cache.lookup(1, true, FG), Lookup::Lead(_)), "cleared");
    }

    const FG: Priority = Priority::Foreground;
    const BG: Priority = Priority::Background;

    #[tokio::test]
    async fn a_user_action_never_waits_on_a_background_request() {
        let cache = Arc::new(ResponseCache::default());
        let Lookup::Lead(bg) = cache.lookup(5, true, BG) else { panic!("bg leads") };
        let Lookup::Join(bg_rx) = cache.lookup(5, true, BG) else { panic!("bg joins bg") };
        let Lookup::Lead(fg) = cache.lookup(5, true, FG) else { panic!("fg sends its own") };
        let Lookup::Join(fg_rx) = cache.lookup(5, true, FG) else { panic!("fg joins fg") };
        let Lookup::Join(_) = cache.lookup(5, true, BG) else { panic!("bg joins fg") };
        // The background leader ending must not drop the user action's entry.
        drop(bg);
        assert!(join(bg_rx).await.is_none());
        assert!(matches!(cache.lookup(5, false, FG), Lookup::Join(_)), "fg entry still there");
        fg.finish(&serde_json::json!(5));
        assert_eq!(*join(fg_rx).await.unwrap(), serde_json::json!(5));
    }

    const SPEC: Priority = Priority::Speculative;

    #[tokio::test]
    async fn requests_join_only_equal_or_higher_priority() {
        let cache = Arc::new(ResponseCache::default());
        let Lookup::Lead(_bg) = cache.lookup(7, true, BG) else { panic!() };
        let Lookup::Lead(_spec) = cache.lookup(7, true, SPEC) else { panic!("hint skips bg") };
        let Lookup::Join(_) = cache.lookup(7, true, BG) else { panic!("bg joins the hint") };
        let Lookup::Join(_) = cache.lookup(7, true, SPEC) else { panic!("hint joins hint") };
        let Lookup::Lead(_fg) = cache.lookup(7, true, FG) else { panic!("a click skips a hint") };
        let Lookup::Join(_) = cache.lookup(7, true, SPEC) else { panic!("hint joins a click") };
    }

    /// The firstaudio finding: hover hints never landed because tarpitted enrichment searches
    /// held every background slot. A hint has slots background work cannot take.
    #[tokio::test]
    async fn a_hint_gets_a_slot_while_background_work_holds_its_whole_lane() {
        tokio::time::pause();
        let gate = Arc::new(Gate::new(Limits::default()));
        let mut held = Vec::new();
        for _ in 0..Limits::default().max_background {
            held.push(gate.acquire(BG).await);
        }
        let start = Instant::now();
        let a = gate.acquire(SPEC).await;
        let b = gate.acquire(SPEC).await;
        assert!(start.elapsed() < Duration::from_millis(5), "{:?}", start.elapsed());
        // And with both hint slots taken too, a click still goes straight through.
        drop(gate.acquire(FG).await);
        assert!(start.elapsed() < Duration::from_millis(5));
        drop((a, b, held));
    }

    #[tokio::test]
    async fn background_waits_for_a_queued_hint() {
        tokio::time::pause();
        let gate = Arc::new(Gate::new(Limits {
            max_in_flight: 1,
            max_background: 4,
            max_speculative: 2,
            background_rate: 1000.0,
            background_burst: 1000.0,
        }));
        let held = gate.acquire(FG).await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let spawn = |p: Priority, name: &'static str| {
            let (g, o) = (gate.clone(), order.clone());
            tokio::spawn(async move {
                let _p = g.acquire(p).await;
                o.lock().unwrap().push(name);
                tokio::time::sleep(Duration::from_millis(30)).await;
            })
        };
        let bg = spawn(BG, "bg");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let spec = spawn(SPEC, "spec");
        tokio::time::sleep(Duration::from_millis(5)).await;
        drop(held);
        spec.await.unwrap();
        bg.await.unwrap();
        assert_eq!(*order.lock().unwrap(), ["spec", "bg"]);
    }

    #[test]
    fn unattended_attempts_are_cut_sooner_and_never_left_uncapped() {
        let gate = Gate::new(Limits::default()); // typical 600 ms
        let fg = gate.header_deadline(FG, 1, 6).unwrap();
        let bg = gate.header_deadline(BG, 1, 8).unwrap();
        let spec = gate.header_deadline(SPEC, 1, 3).unwrap();
        assert_eq!(bg, Duration::from_millis(1_500));
        assert!(bg < fg && spec < fg);
        assert_eq!(gate.header_deadline(FG, 6, 6), None, "a click's last attempt waits it out");
        assert_eq!(gate.header_deadline(SPEC, 3, 3), Some(spec), "a hint never holds on");
        assert_eq!(gate.header_deadline(BG, 8, 8), Some(Duration::from_millis(3_600)));
        assert!(gate.header_deadline(BG, 1, 8).unwrap() >= Duration::from_millis(1_200), "floor");
    }

    #[tokio::test]
    async fn speculative_is_task_local_like_background() {
        speculative(async {
            assert_eq!(current_priority(), Priority::Speculative);
            assert_eq!(tokio::spawn(async { current_priority() }).await.unwrap(), FG);
        })
        .await;
        let p = RetryPolicy::for_priority(SPEC);
        assert!(p.attempts < RetryPolicy::for_priority(FG).attempts, "a hint gives up early");
    }

    #[tokio::test]
    async fn kept_answers_expire() {
        tokio::time::pause();
        let cache = Arc::new(ResponseCache::default());
        let Lookup::Lead(l) = cache.lookup(9, true, FG) else { panic!() };
        l.finish(&serde_json::json!(9));
        tokio::time::advance(CACHE_TTL + Duration::from_secs(1)).await;
        assert!(matches!(cache.lookup(9, true, FG), Lookup::Lead(_)));
    }
}
