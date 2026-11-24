//! HTTP transport. context/01. Pure — no Tauri/webview/mpv.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::sync::{watch, Notify};

use crate::blocklist::BlockList;
use crate::clients::YouTubeClient;
use crate::models::context::Locale;
use crate::reliability::{
    self, classify, current_priority, jitter_unit, Failure, Gate, Limits, Lookup, ResponseCache,
    RetryPolicy, Stats, StatsSnapshot,
};

pub const BASE_URL: &str = "https://music.youtube.com/youtubei/v1/";
pub const ORIGIN: &str = "https://music.youtube.com";
pub const REFERER: &str = "https://music.youtube.com/";
pub const SW_JS_DATA_URL: &str = "https://music.youtube.com/sw.js_data";

/// How long a rejected request waits for a heal it has no evidence is running.
///
/// Deliberately *not* sized to the healer's worst case, because nothing here can know it: the
/// app's refresh allows 45s for its webview alone, then reads a cookie jar off the main thread
/// with no deadline of its own, then signs in over requests that each retry three times against
/// a 60s client timeout. Any number picked here would be a guess, and reports a false "session
/// expired" the moment a slow heal outruns it. So a waiter that can see a heal genuinely in
/// flight ([`InnerTube::begin_heal`]) keeps waiting, and this bounds only the case where nothing
/// answered at all, including no listener: the crate stays pure, so it cannot require one.
pub const HEAL_TIMEOUT: Duration = Duration::from_secs(90);

tokio::task_local! {
    /// Set for the span of the app's own healing and auth work. See [`without_healing`].
    static NO_HEALING: ();
}

/// Run `f` with heal-waiting turned off for anything it requests.
///
/// Wraps the healer itself (`refresh_session` -> `sign_in` -> `account_menu` -> [`InnerTube::post`]):
/// a 401 in there has to come straight back, because waiting would park the healing task on the
/// heal it is itself performing. The auth flows take it for their own reason, that they are
/// validating a cookie already in hand and want its real answer rather than a rescue.
///
/// Task-local rather than a flag on the transport: `InnerTube` is cloned everywhere and shares
/// its state, so a flag would also silence healing for unrelated requests that merely overlapped
/// a heal, which is the false "session expired" all of this exists to prevent. A task-local is
/// not inherited by `tokio::spawn`, so anything spawned out of these flows heals as usual.
pub async fn without_healing<F: std::future::Future>(f: F) -> F::Output {
    NO_HEALING.scope((), f).await
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("visitorData not found in sw.js_data")]
    VisitorDataNotFound,
    #[error("Your YouTube Music session expired — open the account menu and sign in again.")]
    SessionExpired,
    #[error("This track is already in the playlist.")]
    AlreadyInPlaylist,
    /// YouTube refused an add to a playlist this account doesn't own. See
    /// [`InnerTube::playlist_add`].
    #[error("Only the playlist's owner can add songs to it.")]
    PlaylistNotEditable,
    /// A post or reply that may have reached YouTube with its answer lost. See
    /// [`InnerTube::post_write`].
    #[error("The request may have reached YouTube.")]
    WriteUncertain,
    /// The signed-in account changed since the comments were read.
    #[error("The signed-in account changed.")]
    AccountChanged,
    /// A comment action or write answered 200 without success.
    #[error("YouTube did not accept that action.")]
    ActionRejected,
    #[error(
        "YouTube Music only allows custom playlist art on accounts with a verified phone number."
    )]
    CoverRefused,
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// YouTube answered 404: for a request about one comment, the comment is gone.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Http(e) if e.status().is_some_and(|s| s.as_u16() == 404))
    }
}

/// `s` if it looks like an enum value (`STATUS_SUCCEEDED`), safe to log.
pub(crate) fn enum_like(s: &str) -> Option<&str> {
    (s.len() <= 64 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
        .then_some(s)
}

/// Session state, set once at startup / login. context/01 §mutable session state.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub locale: Locale,
    pub visitor_data: Option<String>,
    pub data_sync_id: Option<String>,
    /// Full cookie string (Phase 3). Present ⇒ authenticated requests possible.
    pub cookie: Option<String>,
}

impl Session {
    /// Pull the `SAPISID` value out of the cookie string, if present.
    fn sapisid(&self) -> Option<String> {
        self.cookie.as_deref().and_then(cookie_sapisid).map(str::to_owned)
    }
}

/// Extract the `SAPISID` (or its modern `__Secure-3PAPISID` alias) value from a Cookie header
/// string. Public so the login flow (context/15) can validate a pasted cookie before setting it.
pub fn cookie_sapisid(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        matches!(k.trim(), "SAPISID" | "__Secure-3PAPISID").then(|| v.trim())
    })
}

/// Apply `Set-Cookie` response values to a `Cookie` request header, returning the new header only
/// when something actually changed. Existing names keep their position; new ones go on the end.
///
/// No domain/path matching: every response this is fed comes from music.youtube.com and the jar
/// only ever goes back there, so a `Domain=` a browser would reject cannot reach us.
///
/// ponytail: deletions (`NAME=;`) are ignored rather than applied. A cookie Google wants gone is
/// dead server-side anyway, so carrying it costs nothing, while honouring the deletion would let
/// one odd response drop `SAPISID` and silently sign the user out.
pub(crate) fn merge_set_cookie(cookie: &str, set_cookie: &[&str]) -> Option<String> {
    let mut jar: Vec<(String, String)> = cookie
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut changed = false;
    for line in set_cookie {
        let pair = line.split(';').next().unwrap_or_default();
        let Some((name, value)) = pair.split_once('=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match jar.iter_mut().find(|(n, _)| n == name) {
            Some(entry) if entry.1 == value => {}
            Some(entry) => {
                entry.1 = value.to_owned();
                changed = true;
            }
            None => {
                jar.push((name.to_owned(), value.to_owned()));
                changed = true;
            }
        }
    }
    changed.then(|| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
}

/// What the app's session healer is doing, as one value so a waiter can never read half of it.
///
/// `active` alone cannot answer the question a waiter actually has, because it reads zero both
/// before anything started and after everything finished. `completed` is what separates the two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HealState {
    /// Heals running right now. More than one is not reachable through the app's single healing
    /// task, but `begin_heal` is public and nothing here needs to assume that.
    active: usize,
    /// Heals that have finished, ever.
    completed: u64,
}

/// The transport client. One shared `reqwest::Client`; proxy must be set before the
/// first request or reqwest snapshots it as none (context/12, the App.kt gotcha).
///
/// `session` is behind a shared lock: the app clones `InnerTube` into the orchestrator, and a
/// runtime login (context/15) must be visible through every clone. Reads/writes are quick and
/// never held across an `.await`, so a std `RwLock` is right (no async lock needed).
#[derive(Clone)]
pub struct InnerTube {
    http: reqwest::Client,
    session: Arc<RwLock<Session>>,
    /// "Hide music videos" (off by default): drop non-ATV rows from the surfaces YouTube
    /// generates. Shared like `session` so a settings toggle reaches every clone, and an atomic
    /// rather than part of `Session` because the endpoints read it on every parse.
    hide_videos: Arc<AtomicBool>,
    /// Artists the user blocked: dropped from the surfaces YouTube generates (radio, autoplay,
    /// home, carousels), never from a list the user opened. Shared like `session` so a settings
    /// change reaches every clone. A `RwLock` rather than an atomic because it is a set, and the
    /// same reasoning as `session` applies: reads are quick and never held across an `.await`.
    blocked: Arc<RwLock<BlockList>>,
    /// Pinged whenever a signed-in request comes back rejected (401/403, or a 200 carrying the
    /// logged-out browse state). The app listens and re-mints the cookie; this crate stays pure,
    /// so it only raises the flag. `notify_one` stores a permit, so a single listener that is
    /// busy healing still sees the next rejection.
    session_rejected: Arc<Notify>,
    /// What the app's healer is doing. A `watch` rather than a flag plus a `Notify`, because a
    /// waiter has to read the state and wait for the next change without a gap between the two:
    /// `send_modify` publishes the new value and the wakeup together, and a change that lands
    /// before the waiter gets back to `changed()` is still seen. Both halves of the answer are
    /// carried here too, so a heal that starts, runs and finishes between two polls cannot be
    /// mistaken for one that never ran.
    ///
    /// Deliberately not "a cookie was written": the healer declines most of a burst on its own
    /// cooldown, and a sign-out or a rolled-back sign-in write a cookie without healing anything.
    /// A waiter that treated either as success would retry straight into the same 401.
    heal: Arc<watch::Sender<HealState>>,
    /// Pinged when a response's `Set-Cookie` actually changed the stored jar, so the app can
    /// write the rotated cookie back to disk. See [`InnerTube::absorb_cookies`].
    cookie_changed: Arc<Notify>,
    /// Where `post` sends to: [`BASE_URL`], except in tests, which point it at a local server.
    /// Shared by every clone, so a test can redirect an `InnerTube` already inside an `AppState`.
    base_url: Arc<RwLock<String>>,
    // ── stream: reliability ──
    /// Admission control: in-flight cap, background pacing. See [`crate::reliability`].
    gate: Arc<Gate>,
    /// In-flight dedupe and the short anonymous cache for identical reads.
    cache: Arc<ResponseCache>,
    stats: Arc<Stats>,
    /// Live client versions by `clientName` (from `sw.js_data`), over the bundled ones. See
    /// [`InnerTube::set_client_version`].
    versions: Arc<RwLock<HashMap<String, String>>>,
    // ── end stream: reliability ──
    /// A fresh anonymous visitorData for the clients that never carry the account (VISIONOS and
    /// the other `loginSupported: false` identities). `None` until the orchestrator sees one of
    /// them bot-checked; then it wins over the session's, for those clients only. See
    /// [`InnerTube::set_anon_visitor_data`].
    anon_visitor: Arc<RwLock<Option<String>>>,
}

impl InnerTube {
    pub fn new(session: Session, proxy: Option<&str>) -> Result<Self, Error> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .pool_idle_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(10);
        if let Some(p) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(p)?);
        }
        // Tests talk to a server on localhost: a system proxy must not get in the way.
        #[cfg(test)]
        {
            builder = builder.no_proxy();
        }
        Ok(InnerTube {
            http: builder.build()?,
            session: Arc::new(RwLock::new(session)),
            hide_videos: Arc::new(AtomicBool::new(false)),
            blocked: Arc::new(RwLock::new(BlockList::default())),
            session_rejected: Arc::new(Notify::new()),
            heal: Arc::new(watch::Sender::new(HealState::default())),
            cookie_changed: Arc::new(Notify::new()),
            base_url: Arc::new(RwLock::new(BASE_URL.to_owned())),
            gate: Arc::new(Gate::new(Limits::default())),
            cache: Arc::new(ResponseCache::default()),
            stats: Arc::new(Stats::default()),
            versions: Arc::new(RwLock::new(HashMap::new())),
            anon_visitor: Arc::new(RwLock::new(None)),
        })
    }

    // ── stream: reliability ──

    /// Replace the admission limits (tests, the soak test). Affects every clone made after.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.gate = Arc::new(Gate::new(limits));
        self
    }

    /// Retry/throttle/dedupe counters since startup.
    pub fn reliability_stats(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }

    /// Use `version` for `client_name` instead of the bundled `clients.json` value, on every
    /// clone. Ignored unless it looks like a version (`1.20261110.04.00`).
    pub fn set_client_version(&self, client_name: &str, version: &str) {
        if !is_version_like(version) {
            tracing::warn!(client_name, "ignoring an unusable client version");
            return;
        }
        self.versions.write().unwrap().insert(client_name.to_owned(), version.to_owned());
    }

    /// The version actually sent for `client`.
    pub fn client_version(&self, client: &YouTubeClient) -> String {
        self.versions
            .read()
            .unwrap()
            .get(&client.client_name)
            .cloned()
            .unwrap_or_else(|| client.client_version.clone())
    }

    fn with_live_version(
        &self,
        client: &YouTubeClient,
        mut ctx: crate::models::context::Context,
    ) -> crate::models::context::Context {
        if let Some(v) = self.versions.read().unwrap().get(&client.client_name) {
            ctx.client.client_version = v.clone();
        }
        ctx
    }

    /// Bootstrap `visitorData` and the live WEB_REMIX client version from `sw.js_data` in one
    /// small request (the same file the web app's service worker reads; rustypipe and yt-dlp
    /// take the version from the page config the same way).
    pub async fn fetch_web_config(&self) -> Result<WebConfig, Error> {
        let text = self
            .http
            .get(SW_JS_DATA_URL)
            .header("user-agent", WEB_UA)
            // The EU consent choice ("reject all"), as yt-dlp sends it: without one an EU IP can
            // be answered with the consent interstitial instead of the config. The app is in
            // Belgium; this is the request that mints the anonymous clients' visitorData.
            .header("cookie", "SOCS=CAI")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(WebConfig {
            visitor_data: parse_visitor_data(&text)?,
            web_remix_version: parse_web_remix_version(&text),
        })
    }
    // ── end stream: reliability ──

    /// Point `post` (this handle and every clone of it) somewhere else: a local test server.
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_base_url(&self, url: &str) {
        *self.base_url.write().unwrap() = url.to_owned();
    }

    /// Signal raised when YouTube rejects the signed-in session. See the field.
    pub fn session_rejected(&self) -> Arc<Notify> {
        self.session_rejected.clone()
    }

    /// Signal raised when the stored cookie jar was updated from a response. See the field.
    pub fn cookie_changed(&self) -> Arc<Notify> {
        self.cookie_changed.clone()
    }

    /// The session no longer authenticates. Both callers (transport 401/403 and the logged-out
    /// browse payload) route through here so the healer hears about either one.
    pub(crate) fn reject_session(&self) -> Error {
        self.session_rejected.notify_one();
        Error::SessionExpired
    }

    /// Merge a response's `Set-Cookie` into the stored jar.
    ///
    /// Google rotates `__Secure-1PSIDTS` / `__Secure-3PSIDTS` on the very requests this client
    /// makes, and invalidates the previous value when it does. Dropping the new one (which is
    /// what a `reqwest` client with no cookie store does) is what killed the login a few hours
    /// into every session — issue #165 / KI-2. Note that `cookie_store(true)` would not help:
    /// reqwest skips its own store whenever a `Cookie` header is already set, and `headers()`
    /// always sets one.
    fn absorb_cookies(&self, headers: &HeaderMap) {
        if headers.get(SET_COOKIE).is_none() {
            return;
        }
        let set_cookie: Vec<&str> =
            headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).collect();
        {
            let mut s = self.session.write().unwrap();
            let Some(merged) = s.cookie.as_deref().and_then(|c| merge_set_cookie(c, &set_cookie))
            else {
                return;
            };
            s.cookie = Some(merged);
        }
        // Names only, never values: this line ends up in the log a user attaches to a bug report,
        // and it is the one piece of evidence that says whether rotation is being kept.
        let names: Vec<&str> = set_cookie
            .iter()
            .filter_map(|line| line.split(';').next()?.split_once('='))
            .map(|(name, _)| name.trim())
            .collect();
        tracing::debug!(cookies = ?names, "kept rotated cookies");
        self.cookie_changed.notify_one();
    }

    /// Turn "hide music videos" on/off (context: the user setting, default off).
    pub fn set_hide_videos(&self, on: bool) {
        self.hide_videos.store(on, Ordering::Relaxed);
    }

    pub(crate) fn hide_videos(&self) -> bool {
        self.hide_videos.load(Ordering::Relaxed)
    }

    /// Replace the blocked-artist list (the app rebuilds it from the stored settings row on every
    /// change; there is no incremental update).
    pub fn set_blocked(&self, list: BlockList) {
        *self.blocked.write().unwrap() = list;
    }

    pub(crate) fn blocked(&self) -> std::sync::RwLockReadGuard<'_, BlockList> {
        self.blocked.read().unwrap()
    }

    // --- session accessors (context/15) -----------------------------------------------------

    /// True when a login cookie is set.
    pub fn is_logged_in(&self) -> bool {
        self.session.read().unwrap().cookie.is_some()
    }

    /// The current visitorData (read fresh per resolve — a login may have refreshed it).
    pub fn visitor_data(&self) -> Option<String> {
        self.session.read().unwrap().visitor_data.clone()
    }

    /// Give the anonymous clients (`loginSupported: false`) a visitorData of their own.
    ///
    /// Measured 2026-11-17: VISIONOS answers "Sign in to confirm you're not a bot" for generated
    /// audio tracks (the "- Topic" uploads) when its visitorData is missing or one YouTube does
    /// not accept, while music videos still play; with a fresh anonymous one the same ids play.
    /// That is the iPhone's failure exactly (Topic tracks refused, the Rick Astley health probe
    /// fine), and there the session's visitorData is the signed-in account's, sent by a client
    /// that never carries the account. The session's stays as it is for every other client.
    pub fn set_anon_visitor_data(&self, vd: Option<String>) {
        *self.anon_visitor.write().unwrap() = vd;
    }

    /// The visitorData `client` sends: the anonymous one for a client that never logs in, when
    /// there is one, else the session's.
    fn visitor_for(&self, client: &YouTubeClient, session: &Session) -> Option<String> {
        if !client.login_supported {
            if let Some(vd) = self.anon_visitor.read().unwrap().clone() {
                return Some(vd);
            }
        }
        session.visitor_data.clone()
    }

    /// The current cookie header, if logged in (for the stream-validation HEAD request).
    pub fn cookie(&self) -> Option<String> {
        self.session.read().unwrap().cookie.clone()
    }

    pub fn set_cookie(&self, cookie: Option<String>) {
        let signed_in = cookie.is_some();
        let was = std::mem::replace(&mut self.session.write().unwrap().cookie, cookie).is_some();
        // ── stream: reliability ── kept answers are anonymous ones; a sign-in/out starts over.
        if signed_in != was {
            self.cache.clear();
        }
    }

    pub fn set_data_sync_id(&self, id: Option<String>) {
        self.session.write().unwrap().data_sync_id = id;
    }

    pub fn data_sync_id(&self) -> Option<String> {
        self.session.read().unwrap().data_sync_id.clone()
    }

    /// Who is signed in, for comment tokens that must not cross accounts or channels: a hash of
    /// `SAPISID` and `data_sync_id`, so a rotated `__Secure-*SIDTS` keeps it. `None` signed out.
    /// Not for logging.
    pub fn comments_identity(&self) -> Option<String> {
        let s = self.session.read().unwrap();
        let sapisid = s.sapisid()?;
        let dsid = s.data_sync_id.as_deref().unwrap_or_default();
        Some(sha1_hex(&format!("limusic-comments-identity-v1\0{sapisid}\0{dsid}")))
    }

    pub fn set_visitor_data(&self, vd: Option<String>) {
        self.session.write().unwrap().visitor_data = vd;
    }

    /// The language YouTube answers in (`hl`), from the UI's own language setting.
    ///
    /// Home shelf titles, mood chips, playlist subtitles and auto-playlist names are YouTube's
    /// text, not ours, so a Korean UI left on `hl=en` reads half English (#274). `gl` stays where
    /// it is: that picks *which* feed comes back, not the language of its labels, and a user who
    /// switches the interface to Korean has not asked for a different catalogue.
    ///
    /// Ignores anything that is not tag-shaped. The value comes from the webview and ends up in a
    /// request header, and a locale we do not recognize is better dropped than sent.
    pub fn set_locale(&self, hl: &str) {
        if hl.is_empty() || !hl.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            tracing::warn!(hl, "ignoring an unusable locale");
            return;
        }
        self.session.write().unwrap().locale.hl = hl.to_owned();
    }

    /// Build the request `context` for a client from the current session. Crate-internal — the
    /// endpoints facade calls it. Reads and drops the lock synchronously (no `.await` inside).
    pub(crate) fn context_for(&self, client: &YouTubeClient) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        // `onBehalfOfUser` makes Google *require* a credential: with no cookie it turns a request
        // that would have worked anonymously into a hard 401. Only send it when we can authenticate.
        let dsid = s.cookie.as_ref().and(s.data_sync_id.as_deref());
        let vd = self.visitor_for(client, &s);
        let ctx = client.to_context(&s.locale, vd.as_deref(), dsid);
        drop(s);
        self.with_live_version(client, ctx)
    }

    /// A context with no account attached, for requests that must not be attributed to the user:
    /// no `onBehalfOfUser`, and the caller passes `set_login = false` so no cookie goes with it.
    pub(crate) fn context_anonymous(
        &self,
        client: &YouTubeClient,
    ) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        let ctx = client.to_context(&s.locale, s.visitor_data.as_deref(), None);
        drop(s);
        self.with_live_version(client, ctx)
    }

    /// Build a one-off authenticated context for identity validation without changing the shared
    /// session seen by concurrent browse/playback requests. The caller commits the id only after
    /// the validation response succeeds.
    pub(crate) fn context_for_identity(
        &self,
        client: &YouTubeClient,
        data_sync_id: &str,
    ) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        let dsid = s.cookie.as_ref().map(|_| data_sync_id);
        let ctx = client.to_context(&s.locale, s.visitor_data.as_deref(), dsid);
        drop(s);
        self.with_live_version(client, ctx)
    }

    /// POST a JSON body to an InnerTube endpoint with this client's headers, retrying
    /// transient network errors (3 attempts, 500ms × 2 backoff). context/01 §retry.
    pub async fn post<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
    ) -> Result<serde_json::Value, Error> {
        self.post_inner(path, client, body, set_login, true, true).await
    }

    /// [`InnerTube::post`] that hands a 401/403 back as `Error::Http` instead of waking the healer,
    /// for a read the user did not ask for. The caller decides, normally by asking anonymously.
    pub(crate) async fn post_no_heal<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
    ) -> Result<serde_json::Value, Error> {
        self.post_inner(path, client, body, set_login, false, true).await
    }

    /// [`InnerTube::post`] for a write that is not idempotent (post, reply): never retried, since a
    /// lost answer would mean a duplicate. A refusal is still healed and re-sent. A failure after
    /// the request went out (timeout, reset, unreadable answer) is `Error::WriteUncertain`.
    pub(crate) async fn post_write<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
    ) -> Result<serde_json::Value, Error> {
        match self.post_inner(path, client, body, set_login, true, false).await {
            Err(Error::Http(e)) if e.status().is_none() && !e.is_connect() && !e.is_builder() => {
                tracing::debug!(
                    timeout = e.is_timeout(),
                    decode = e.is_decode(),
                    "comment write outcome unknown"
                );
                Err(Error::WriteUncertain)
            }
            other => other,
        }
    }

    /// `heal`: a 401/403 on a signed-in request may wake the healer. `retry`: the request is an
    /// idempotent read, so a connect error, a timeout, a 5xx or Google's bot wall is sent again
    /// (see [`crate::reliability`]), and an identical read already in flight is joined rather
    /// than sent twice. Both `true` for [`InnerTube::post`].
    async fn post_inner<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
        heal: bool,
        retry: bool,
    ) -> Result<serde_json::Value, Error> {
        // `path` may already carry query params (e.g. browse continuations); chain accordingly.
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!("{}{path}{sep}prettyPrint=false", self.base_url.read().unwrap());
        let body = serde_json::to_vec(body)?;

        // ── stream: reliability ── identical reads share one request; anonymous ones are kept.
        if !(retry && reliability::is_cacheable_read(path)) {
            return self.send(path, &url, client, &body, set_login, heal, retry).await;
        }
        let cookie = (set_login && client.login_supported).then(|| self.cookie()).flatten();
        let version = self.client_version(client);
        let key = ResponseCache::key(
            &url,
            &[
                ("client", &client.client_id),
                ("version", &version),
                ("cookie", cookie.as_deref().unwrap_or_default()),
            ],
            &body,
        );
        let leader = match self.cache.lookup(key, cookie.is_none(), current_priority()) {
            Lookup::Hit(v) => {
                Stats::bump(&self.stats.cache_hits);
                return Ok((*v).clone());
            }
            Lookup::Join(rx) => {
                Stats::bump(&self.stats.deduped);
                if let Some(v) = reliability::join(rx).await {
                    return Ok((*v).clone());
                }
                // The leader failed: try for ourselves (its retries are already spent, so this
                // is the joiner's own budget, not a loop).
                None
            }
            Lookup::Lead(l) => Some(l),
        };
        let res = self.send(path, &url, client, &body, set_login, heal, retry).await;
        if let (Ok(v), Some(l)) = (&res, leader) {
            l.finish(v);
        }
        res
    }

    /// One logical request: the attempts, the healing, the retries.
    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        path: &str,
        url: &str,
        client: &YouTubeClient,
        body: &[u8],
        set_login: bool,
        heal: bool,
        retry: bool,
    ) -> Result<serde_json::Value, Error> {
        let priority = current_priority();
        let policy = RetryPolicy::for_priority(priority);
        let mut attempt = 0;
        let mut healed = false;

        loop {
            attempt += 1;
            let retry_left = retry && attempt < policy.attempts;
            // Rebuild headers on every iteration so retried requests use the updated session cookie.
            let headers = self.headers(client, set_login);
            // Held for the attempt only, never across a backoff sleep.
            let permit = self.gate.acquire(priority).await;
            Stats::bump(&self.stats.requests);

            let sent = self.http.post(url).headers(headers).body(body.to_vec()).send();
            let started = tokio::time::Instant::now();
            let deadline =
                retry.then(|| self.gate.header_deadline(priority, attempt, policy.attempts));
            let res = match deadline.flatten() {
                Some(d) => match tokio::time::timeout(d, sent).await {
                    Ok(res) => res,
                    // The bot wall's tarpit (or a stalled connection): abandon and resend.
                    Err(_) => {
                        drop(permit);
                        Stats::bump(&self.stats.cut_short);
                        Stats::bump(&self.stats.throttled);
                        Stats::bump(&self.stats.retries);
                        self.gate.throttled();
                        tracing::info!(
                            attempt,
                            ?d,
                            ?priority,
                            "InnerTube {path}: no answer, resending"
                        );
                        tokio::time::sleep(policy.delay(attempt, jitter_unit())).await;
                        continue;
                    }
                },
                None => sent.await,
            };
            let resp = match res {
                Ok(resp) => resp,
                // Retry on connect/timeout (transient), matching Metrolist's IOException filter.
                Err(e) if retry_left && (e.is_timeout() || e.is_connect() || e.is_request()) => {
                    drop(permit);
                    tracing::warn!(attempt, error = %e, "retrying InnerTube POST {path}");
                    Stats::bump(&self.stats.retries);
                    tokio::time::sleep(policy.delay(attempt, jitter_unit())).await;
                    continue;
                }
                Err(e) => {
                    if retry && (e.is_timeout() || e.is_connect()) {
                        Stats::bump(&self.stats.gave_up);
                    }
                    return Err(e.into());
                }
            };

            if resp.status().is_success() {
                self.gate.succeeded(started.elapsed());
                self.absorb_cookies(resp.headers());
                let value = resp.json().await;
                drop(permit);
                return Ok(value?);
            }
            drop(permit);

            let status = resp.status().as_u16();
            let html = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.starts_with("text/html"));
            let failure = classify(status, html);
            match failure {
                Failure::Throttled => {
                    Stats::bump(&self.stats.throttled);
                    self.gate.throttled();
                }
                Failure::Server => Stats::bump(&self.stats.server_errors),
                Failure::Final => {}
            }
            let e = resp.error_for_status().expect_err("not a success status");

            match failure {
                // Google's front end, not InnerTube and not the session: wait and resend.
                // A 5xx on a write may have landed, so only reads resend one; the bot wall
                // answers before InnerTube sees anything, so any request may resend that.
                Failure::Throttled | Failure::Server
                    if retry_left && (failure == Failure::Throttled || idempotent(path)) =>
                {
                    let wait = policy.delay(attempt, jitter_unit());
                    tracing::info!(attempt, status, ?wait, ?priority, "InnerTube {path}: retrying");
                    Stats::bump(&self.stats.retries);
                    tokio::time::sleep(wait).await;
                    continue;
                }
                Failure::Throttled | Failure::Server => {
                    if retry {
                        Stats::bump(&self.stats.gave_up);
                    }
                    tracing::warn!(attempt, status, "InnerTube {path}: giving up");
                    return Err(e.into());
                }
                Failure::Final => {}
            }

            // Signed in and Google says "no credential" (401) or "not for you" (403): the
            // stored cookie has gone stale. Most of these are survivable: the healer
            // re-mints from the login webview's own Google session, so wait for it and try
            // once more rather than telling a user whose session is about to come back that
            // it expired (#211). Only for a request that actually carried the cookie: a deliberately
            // anonymous one (a search preview) is refused for its own reasons and says
            // nothing about the session, and `headers` sends the cookie only for a client
            // that supports login, so every anonymous stream client in the fallback chain
            // would otherwise sign the user out on the 403 that made the orchestrator move to
            // the next one. `healing_suspended` keeps the healer's own validation call from
            // waiting on the healer that is making it. (The bot wall's HTML 403 never gets here:
            // it is about the IP, not the cookie.)
            let refused = status == 401 || status == 403;
            if heal && set_login && client.login_supported && self.is_logged_in() && refused {
                if !healed && !self.healing_suspended() {
                    healed = true;
                    tracing::warn!(status, "InnerTube {path} rejected the session, healing");
                    self.wait_for_session_heal().await?;
                    tracing::info!("heal finished, retrying {path}");
                    continue;
                }
                // Rejected again with whatever the heal produced, or with healing unavailable:
                // the session really is dead. Raw reqwest text here reads as a broken app and
                // hands the user a URL instead of the one thing that fixes it, so this stays
                // `SessionExpired`.
                tracing::warn!(status, "InnerTube {path} rejected the session");
                return Err(self.reject_session());
            }
            return Err(e.into());
        }
    }

    /// POST raw bytes to a path on the same origin that is *not* under `/youtubei`, with this
    /// client's headers plus `extra`, and hand back the response headers along with the body.
    ///
    /// Google's resumable uploader ("Scotty") lives on its own path and answers the first step in
    /// a header, so neither `post`'s URL shape nor its JSON-only return works here. The
    /// `content-type: application/json` the client headers carry stays put even when the body is
    /// an image: the uploader ignores it, and that is the shape known to work.
    pub(crate) async fn post_upload(
        &self,
        path: &str,
        client: &YouTubeClient,
        extra: &[(&'static str, String)],
        body: Vec<u8>,
    ) -> Result<(HeaderMap, Vec<u8>), Error> {
        let mut headers = self.headers(client, true);
        for (name, value) in extra {
            if let Ok(v) = HeaderValue::from_str(value) {
                headers.insert(HeaderName::from_static(name), v);
            }
        }
        // Explicitly, from the body we are about to send: reqwest omits `content-length` entirely
        // when the body is empty, and the uploader answers the empty "start" call with a bare
        // 411 Length Required. Sending it ourselves costs nothing on the calls that carry bytes.
        if let Ok(v) = HeaderValue::from_str(&body.len().to_string()) {
            headers.insert(reqwest::header::CONTENT_LENGTH, v);
        }
        let resp = self
            .http
            .post(format!("{ORIGIN}/{path}"))
            .headers(headers)
            .body(body)
            .send()
            .await?
            .error_for_status()?;
        let headers = resp.headers().clone();
        self.absorb_cookies(&headers);
        Ok((headers, resp.bytes().await?.to_vec()))
    }

    /// Per-request headers. context/01 §ytClient. Note `X-YouTube-Client-Name` carries the
    /// numeric client **id**, not the name string — intentional and required.
    fn headers(&self, client: &YouTubeClient, set_login: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
            if let Ok(val) = HeaderValue::from_str(v) {
                h.insert(HeaderName::from_static(k), val);
            }
        };
        set(&mut h, "content-type", "application/json");
        set(&mut h, "accept", "application/json");
        set(&mut h, "x-goog-api-format-version", "1");
        set(&mut h, "x-youtube-client-name", &client.client_id);
        set(&mut h, "x-youtube-client-version", &self.client_version(client));
        set(&mut h, "x-origin", ORIGIN);
        set(&mut h, "referer", REFERER);
        set(&mut h, "user-agent", &client.user_agent);

        let s = self.session.read().unwrap();
        set(&mut h, "accept-language", &s.locale.accept_language());
        if let Some(vd) = self.visitor_for(client, &s) {
            set(&mut h, "x-goog-visitor-id", &vd);
        }

        // SAPISIDHASH cookie auth — only when logged in AND the client supports it (Phase 3).
        if set_login && client.login_supported {
            if let Some(cookie) = &s.cookie {
                set(&mut h, "cookie", cookie);
                if let Some(sapisid) = s.sapisid() {
                    if let Ok(val) = HeaderValue::from_str(&sapisid_hash(&sapisid, ORIGIN)) {
                        h.insert(HeaderName::from_static("authorization"), val);
                    }
                }
            }
        }
        h
    }

    /// Bootstrap `visitorData` anonymously by scraping `sw.js_data`. context/04 §A.
    pub async fn fetch_visitor_data(&self) -> Result<String, Error> {
        Ok(self.fetch_web_config().await?.visitor_data)
    }

    /// Register a play in watch history: GET the response's
    /// `playbackTracking.videostatsPlaybackUrl.baseUrl` with `c`/`cpn`/`ver` (+ `list`/`referrer`
    /// in a playlist) and the authed client headers. context/01 §registerPlayback. Best-effort —
    /// the caller logs-and-ignores errors.
    pub async fn register_playback(
        &self,
        client: &YouTubeClient,
        base_url: &str,
        cpn: &str,
        playlist_id: Option<&str>,
    ) -> Result<(), Error> {
        let url = build_playback_url(base_url, &client.client_name, cpn, playlist_id);
        let headers = self.headers(client, true);
        let resp = self.http.get(&url).headers(headers).send().await?.error_for_status()?;
        self.absorb_cookies(resp.headers());
        Ok(())
    }

    #[cfg(any(test, feature = "integration-tests"))]
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Raise the rejection flag and wait for the app's healer to have its go, so the caller can
    /// retry once with whatever it managed to re-mint.
    ///
    /// Returns as soon as a heal has run its course, whether or not it re-minted anything: the
    /// caller retries either way, and one that really is dead can then say so instead of holding
    /// a spinner. While a heal is still running the wait continues however long it takes, since
    /// [`HEAL_TIMEOUT`] is our deadline and not evidence about the session. The timeout only
    /// resolves the case where nothing ever started.
    pub(crate) async fn wait_for_session_heal(&self) -> Result<(), Error> {
        let mut heals = self.heal.subscribe();
        let start = *heals.borrow_and_update();

        self.session_rejected.notify_one();

        loop {
            let waited = tokio::time::timeout(HEAL_TIMEOUT, heals.changed()).await;
            // The transport outlives every waiter in the app, so this is shutdown, not a heal.
            if matches!(waited, Ok(Err(_))) {
                return Err(Error::SessionExpired);
            }
            let now = *heals.borrow_and_update();

            if now.active == 0 && now.completed > start.completed {
                return Ok(());
            }
            if waited.is_err() && now.active == 0 {
                tracing::warn!("no answer from the session healer within {HEAL_TIMEOUT:?}");
                return Err(Error::SessionExpired);
            }
            if waited.is_err() {
                tracing::debug!("heal still running past {HEAL_TIMEOUT:?}, keeping the wait");
            }
        }
    }

    /// Mark a heal as under way. Dropping the guard records it as finished, which is what
    /// releases the waiters. `Drop` rather than an explicit call because the app's healer returns
    /// from a dozen places, and the one path that forgot to signal would be a hang.
    pub fn begin_heal(&self) -> Healing {
        self.heal.send_modify(|s| s.active += 1);
        Healing(self.heal.clone())
    }

    pub(crate) fn healing_suspended(&self) -> bool {
        NO_HEALING.try_with(|()| ()).is_ok()
    }
}

/// A heal in progress. See [`InnerTube::begin_heal`].
pub struct Healing(Arc<watch::Sender<HealState>>);

impl Drop for Healing {
    fn drop(&mut self) {
        // One `send_modify` for both halves: a waiter cannot see `active` reach zero without the
        // matching `completed`, and cannot miss the wakeup that goes with them. Overlapping
        // heals therefore release nobody until the last one ends.
        self.0.send_modify(|s| {
            s.active -= 1;
            s.completed += 1;
        });
    }
}

/// Build the playback-tracking GET URL. context/01 §registerPlayback. Pure — unit-tested. The
/// `base_url` already carries YouTube's own query params, so we chain onto it.
fn build_playback_url(
    base_url: &str,
    client_name: &str,
    cpn: &str,
    playlist_id: Option<&str>,
) -> String {
    let sep = if base_url.contains('?') { '&' } else { '?' };
    let mut url = format!(
        "{base_url}{sep}c={}&cpn={}&ver=2",
        urlencoding::encode(client_name),
        urlencoding::encode(cpn),
    );
    if let Some(list) = playlist_id {
        let enc = urlencoding::encode(list);
        url.push_str(&format!("&list={enc}&referrer={enc}"));
    }
    url
}

/// CPN alphabet — 64 URL-safe chars, exactly 6 bits each. context/01.
const CPN_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A fresh 16-char Content Playback Nonce for one playback. context/01 §registerPlayback.
// ponytail: time+counter-seeded xorshift, not crypto-rand — a CPN only needs to be unique per
// playback, not unpredictable; keeps the `rand` crate out of the tree.
pub fn generate_cpn() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut state = (nanos ^ bump).wrapping_add(0x1234_567);
    if state == 0 {
        state = 0xDEAD_BEEF;
    }
    let mut out = String::with_capacity(16);
    for _ in 0..16 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(CPN_CHARS[(state & 63) as usize] as char);
    }
    out
}

/// `Authorization: SAPISIDHASH <epoch>_<sha1(epoch SAPISID origin)>`. context/01.
pub fn sapisid_hash(sapisid: &str, origin: &str) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("SAPISIDHASH {epoch}_{}", sha1_hex(&format!("{epoch} {sapisid} {origin}")))
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

// ── stream: reliability ──

/// What `sw.js_data` tells an anonymous web client. See [`InnerTube::fetch_web_config`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebConfig {
    pub visitor_data: String,
    /// The live WEB_REMIX `clientVersion`, e.g. `1.20261110.04.00`.
    pub web_remix_version: Option<String>,
}

/// The user agent `sw.js_data` is fetched with: the WEB_REMIX one from `clients.json`.
const WEB_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";

/// `1.20261110.04.00`: four dot-separated numbers, the second a date.
pub fn is_version_like(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_digit()))
        && parts[1].len() == 8
}

/// The first web-client version string in `sw.js_data` (`1.YYYYMMDD.NN.NN`, the WEB_REMIX shape).
fn parse_web_remix_version(body: &str) -> Option<String> {
    let json_start = body.find('[')?;
    let value: serde_json::Value = serde_json::from_str(&body[json_start..]).ok()?;
    fn walk(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) if s.starts_with("1.") && is_version_like(s) => {
                Some(s.clone())
            }
            serde_json::Value::Array(a) => a.iter().find_map(walk),
            _ => None,
        }
    }
    walk(&value)
}

/// Requests whose repeat is harmless even if the first one landed.
fn idempotent(path: &str) -> bool {
    reliability::is_cacheable_read(path) || path == "player"
}
// ── end stream: reliability ──

/// The `sw.js_data` body starts with a 4–5 char junk prefix (`)]}'`); strip it, parse JSON,
/// and find the element matching `^Cg[ts]` in `[0][2]`. context/04 §A.
fn parse_visitor_data(body: &str) -> Result<String, Error> {
    // Drop everything up to and including the first newline or the `)]}'` guard.
    let json_start = body.find('[').ok_or(Error::VisitorDataNotFound)?;
    let value: serde_json::Value = serde_json::from_str(&body[json_start..])?;
    let arr = value
        .get(0)
        .and_then(|v| v.get(2))
        .and_then(|v| v.as_array())
        .ok_or(Error::VisitorDataNotFound)?;
    arr.iter()
        .filter_map(|v| v.as_str())
        .find(|s| s.starts_with("Cgt") || s.starts_with("Cgs"))
        .map(str::to_owned)
        .ok_or(Error::VisitorDataNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sapisid_hash_shape() {
        let h = sapisid_hash("MYSAPISID", ORIGIN);
        assert!(h.starts_with("SAPISIDHASH "));
        let rest = &h["SAPISIDHASH ".len()..];
        let (epoch, hash) = rest.split_once('_').unwrap();
        assert!(epoch.parse::<u64>().is_ok());
        assert_eq!(hash.len(), 40); // sha1 hex
    }

    #[test]
    fn parse_visitor_data_from_blob() {
        // Shape of sw.js_data: outer array; [0][2] holds the visitorData among other strings.
        let blob = r#")]}'
[["wrs","x",["junk","CgtABCDEFG1234567%3D%3D","more"]],null]"#;
        assert_eq!(parse_visitor_data(blob).unwrap(), "CgtABCDEFG1234567%3D%3D");
    }

    #[test]
    fn web_remix_version_from_sw_js_data() {
        let blob = r#")]}'
[["wrs",null,["junk","CgtABCDEFG1234567%3D%3D",["en","2.20261101.01.00x","1.20261110.04.00"]]]]"#;
        assert_eq!(parse_web_remix_version(blob).as_deref(), Some("1.20261110.04.00"));
        assert_eq!(parse_web_remix_version(")]}'\n[[1,2]]"), None);
        assert!(is_version_like("1.20261110.04.00"));
        assert!(!is_version_like("1.2026111.04.00") && !is_version_like("7.01.05"));
        assert!(!is_version_like("1.20261110.04.00\r\nx: y"));
    }

    #[test]
    fn a_live_client_version_replaces_the_bundled_one_everywhere() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let it = InnerTube::new(Session::default(), None).unwrap();
        assert_eq!(it.context_for(web).client.client_version, web.client_version);
        let clone = it.clone();
        it.set_client_version("WEB_REMIX", "1.20261110.04.00");
        it.set_client_version("WEB_REMIX", "garbage");
        assert_eq!(clone.context_for(web).client.client_version, "1.20261110.04.00");
        assert_eq!(clone.context_anonymous(web).client.client_version, "1.20261110.04.00");
        assert_eq!(
            clone.headers(web, false).get("x-youtube-client-version").unwrap(),
            "1.20261110.04.00"
        );
        let ios = clients.get("IOS_MUSIC").unwrap();
        assert_eq!(clone.context_for(ios).client.client_version, ios.client_version, "per client");
    }

    #[test]
    fn playback_url_appends_params() {
        // Base URL already has query params → chained with `&`; playlist adds list+referrer.
        let u = build_playback_url(
            "https://s.youtube.com/api/stats/playback?cl=1&docid=abc",
            "WEB_REMIX",
            "CPN1234567890AB",
            Some("RDAMVMxyz"),
        );
        assert!(u.contains("?cl=1&docid=abc&c=WEB_REMIX&cpn=CPN1234567890AB&ver=2"));
        assert!(u.contains("&list=RDAMVMxyz&referrer=RDAMVMxyz"));
        // No existing query → first param uses `?`, no playlist params.
        let u2 = build_playback_url("https://s.youtube.com/x", "IOS", "abc", None);
        assert_eq!(u2, "https://s.youtube.com/x?c=IOS&cpn=abc&ver=2");
    }

    #[test]
    fn cpn_is_16_url_safe_chars() {
        let cpn = generate_cpn();
        assert_eq!(cpn.len(), 16);
        assert!(cpn.bytes().all(|b| CPN_CHARS.contains(&b)));
        // Two calls in quick succession must differ (counter salt).
        assert_ne!(generate_cpn(), generate_cpn());
    }

    #[test]
    fn on_behalf_of_user_needs_a_cookie() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session { data_sync_id: Some("abc123".into()), ..Default::default() };

        let it = InnerTube::new(session, None).unwrap();
        assert_eq!(it.context_for(web).user.on_behalf_of_user, None, "no cookie ⇒ no obo (401)");

        it.set_cookie(Some("SAPISID=secret".into()));
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("abc123"));
    }

    /// The anonymous visitorData reaches the clients that never log in, body and header, and
    /// nothing else: WEB_REMIX keeps the session's.
    #[test]
    fn the_anonymous_visitor_goes_only_to_clients_that_never_log_in() {
        let clients = crate::clients::Clients::bundled();
        let (web, vis) = (clients.get("WEB_REMIX").unwrap(), clients.get("VISIONOS").unwrap());
        let session = Session { visitor_data: Some("signed-in".into()), ..Default::default() };
        let it = InnerTube::new(session, None).unwrap();
        let header = |c| it.headers(c, true).get("x-goog-visitor-id").cloned().unwrap();
        assert_eq!(it.context_for(vis).client.visitor_data.as_deref(), Some("signed-in"));

        it.set_anon_visitor_data(Some("anon".into()));
        assert_eq!(it.context_for(vis).client.visitor_data.as_deref(), Some("anon"));
        assert_eq!(header(vis), "anon");
        assert_eq!(it.context_for(web).client.visitor_data.as_deref(), Some("signed-in"));
        assert_eq!(header(web), "signed-in");
        assert_eq!(it.visitor_data().as_deref(), Some("signed-in"), "the session is untouched");
    }

    #[test]
    fn anonymous_context_carries_no_account() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session {
            cookie: Some("SAPISID=secret".into()),
            data_sync_id: Some("abc123".into()),
            visitor_data: Some("visitor".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();

        // Signed in, yet `search` must stay unattributable (#203).
        let ctx = it.context_anonymous(web);
        assert_eq!(ctx.user.on_behalf_of_user, None);
        assert_eq!(ctx.client.visitor_data.as_deref(), Some("visitor"), "still a session, no user");
    }

    #[test]
    fn identity_validation_context_does_not_mutate_the_committed_session() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session {
            cookie: Some("SAPISID=secret".into()),
            data_sync_id: Some("committed-id".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();

        assert_eq!(
            it.context_for_identity(web, "candidate-id").user.on_behalf_of_user.as_deref(),
            Some("candidate-id")
        );
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("committed-id"));
    }

    #[test]
    fn the_comments_identity_tracks_the_account_and_channel_not_the_rotating_cookies() {
        let it = |cookie: Option<&str>, dsid: Option<&str>| {
            InnerTube::new(
                Session {
                    cookie: cookie.map(str::to_owned),
                    data_sync_id: dsid.map(str::to_owned),
                    ..Default::default()
                },
                None,
            )
            .unwrap()
        };
        assert_eq!(it(None, Some("d")).comments_identity(), None, "signed out has none");
        let base = it(Some("SAPISID=a; __Secure-3PSIDTS=old"), Some("d")).comments_identity();
        assert!(base.is_some());
        assert_eq!(
            base,
            it(Some("__Secure-3PSIDTS=new; SAPISID=a"), Some("d")).comments_identity(),
            "a rotated cookie is the same identity"
        );
        assert_ne!(base, it(Some("SAPISID=b"), Some("d")).comments_identity(), "another account");
        assert_ne!(base, it(Some("SAPISID=a"), Some("e")).comments_identity(), "another channel");
        assert_ne!(base, it(Some("SAPISID=a"), None).comments_identity(), "no channel selected");
        let shown = base.unwrap();
        assert_eq!(shown.len(), 40, "a sha1 hex digest, not the secret");
    }

    /// A write is never sent twice by the transport, and a lost answer is "uncertain", not a plain
    /// failure. The control is the ordinary `post`, which does retry the same hang-up.
    #[tokio::test]
    async fn a_write_is_sent_once_and_a_lost_answer_is_uncertain() {
        let server = crate::test_server::MockServer::start(|_| (0, String::new())); // hang up
        let it = signed_in_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();

        let got = it.post_write("comment/x", web, &serde_json::json!({}), true).await;
        assert!(matches!(got, Err(Error::WriteUncertain)), "{got:?}");
        assert_eq!(server.requests().len(), 1, "exactly one request, no retry");

        // Control: the ordinary path retries the very same failure (3 attempts).
        let before = server.requests().len();
        let _ = it.post("comment/x", web, &serde_json::json!({}), true).await;
        assert!(server.requests().len() - before > 1, "post() was expected to retry");
    }

    /// What is and is not uncertain: an answer that arrived is never uncertain unless it is
    /// unreadable (it was a 200, so the write may have happened); nothing sent is a plain error.
    #[tokio::test]
    async fn only_an_unknown_outcome_is_uncertain() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();

        let refused = crate::test_server::MockServer::start(|_| (500, "{}".into()));
        let it = signed_in_against(&refused);
        let got = it.post_write("comment/x", web, &serde_json::json!({}), true).await;
        assert!(matches!(got, Err(Error::Http(_))), "a 500 is a plain failure: {got:?}");
        assert_eq!(refused.requests().len(), 1);

        let garbled = crate::test_server::MockServer::start(|_| (200, "not json".into()));
        let it = signed_in_against(&garbled);
        let got = it.post_write("comment/x", web, &serde_json::json!({}), true).await;
        assert!(matches!(got, Err(Error::WriteUncertain)), "accepted but unreadable: {got:?}");

        let ok = crate::test_server::MockServer::start(|_| (200, r#"{"ok":1}"#.into()));
        let it = signed_in_against(&ok);
        assert!(it.post_write("comment/x", web, &serde_json::json!({}), true).await.is_ok());

        // Nothing listening: the request was never sent, so it is not uncertain.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/youtubei/v1/", l.local_addr().unwrap())
        };
        let it = signed_in_against(&ok);
        it.set_base_url(&dead);
        let got = it.post_write("comment/x", web, &serde_json::json!({}), true).await;
        assert!(matches!(got, Err(Error::Http(_))), "a connect failure sent nothing: {got:?}");
    }

    #[tokio::test]
    async fn a_404_is_not_found_and_other_failures_are_not() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        for (status, gone) in [(404, true), (400, false), (403, false), (500, false)] {
            let server = crate::test_server::MockServer::start(move |_| (status, "{}".into()));
            let it = signed_in_against(&server);
            let got = it.post_write("comment/x", web, &serde_json::json!({}), false).await;
            let err = got.expect_err("a non-2xx is an error");
            assert_eq!(err.is_not_found(), gone, "{status}: {err:?}");
        }
        assert!(!Error::ActionRejected.is_not_found() && !Error::WriteUncertain.is_not_found());
    }

    #[test]
    fn sapisid_extracted_from_cookie() {
        let s = Session {
            cookie: Some("FOO=bar; SAPISID=secret123; OTHER=x".into()),
            ..Default::default()
        };
        assert_eq!(s.sapisid().as_deref(), Some("secret123"));
    }

    #[tokio::test]
    async fn only_the_end_of_a_heal_releases_a_waiter_not_a_cookie_write() {
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let healer = it.clone();
        let waiter = it.clone();

        let waiting = tokio::spawn(async move { waiter.wait_for_session_heal().await });
        it.session_rejected.notified().await;

        let heal = healer.begin_heal();
        // A cookie write on its own must not count. Sign-out and a rolled-back sign-in write one
        // too, and neither healed anything.
        healer.set_cookie(Some("SAPISID=healed".into()));
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "a cookie write is not the end of a heal");

        drop(heal);
        waiting.await.unwrap().expect("the heal ended, so the waiter must be released");
        assert_eq!(it.cookie().as_deref(), Some("SAPISID=healed"));
    }

    // Overlapping heals: `begin_heal` is public, so the last one out is what releases a waiter.
    // Releasing on the first would send it back at a session the remaining heal has not fixed
    // yet, and its second 401 is final.
    #[tokio::test]
    async fn overlapping_heals_release_the_waiter_only_once_the_last_one_ends() {
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let healer = it.clone();
        let waiter = it.clone();

        let waiting = tokio::spawn(async move { waiter.wait_for_session_heal().await });
        it.session_rejected.notified().await;

        let first = healer.begin_heal();
        let second = healer.begin_heal();
        drop(first);
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "the second heal is still running");

        drop(second);
        assert!(waiting.await.unwrap().is_ok());
    }

    // The burst case: the healer declines on its own cooldown and re-mints nothing, but it still
    // reports back. A waiter watching for a new cookie would hold the caller's spinner for the
    // full HEAL_TIMEOUT before returning the error it could have returned at once.
    #[tokio::test]
    async fn a_heal_that_re_minted_nothing_still_releases_the_waiter() {
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let healer = it.clone();
        tokio::spawn(async move {
            healer.session_rejected.notified().await;
            drop(healer.begin_heal());
        });

        let start = tokio::time::Instant::now();
        assert!(it.wait_for_session_heal().await.is_ok());
        assert!(start.elapsed() < HEAL_TIMEOUT, "released by the healer, not by the timeout");
    }

    // The #211 case the timeout would otherwise re-create: the refresh webview, the jar read and
    // the sign-in that follows have no shared ceiling, so a heal can outrun any number picked
    // here. While one is demonstrably in flight the waiter holds instead of crying expired.
    #[tokio::test]
    async fn a_heal_running_past_the_timeout_is_waited_out() {
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let healer = it.clone();
        let waiter = it.clone();

        let waiting = tokio::spawn(async move { waiter.wait_for_session_heal().await });
        it.session_rejected.notified().await;

        let heal = healer.begin_heal();
        tokio::time::advance(HEAL_TIMEOUT * 3).await;
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "a heal in flight outranks our own deadline");

        drop(heal);
        assert!(waiting.await.unwrap().is_ok());
    }

    // A heal that begins and ends entirely between two polls of the waiter. `active` is back to
    // zero by the time it looks, so only the completion count can tell this from "nothing ran".
    #[tokio::test]
    async fn a_heal_that_starts_and_ends_between_polls_still_releases_the_waiter() {
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let healer = it.clone();
        let waiter = it.clone();

        let waiting = tokio::spawn(async move { waiter.wait_for_session_heal().await });
        it.session_rejected.notified().await;

        drop(healer.begin_heal());
        assert!(waiting.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn wait_for_session_heal_times_out_when_no_heal_ever_starts() {
        // Virtual time: this asserts on HEAL_TIMEOUT without spending it.
        tokio::time::pause();
        let it = InnerTube::new(Session::default(), None).unwrap();
        let waiter = it.clone();

        let handle = tokio::spawn(async move { waiter.wait_for_session_heal().await });
        tokio::task::yield_now().await;
        tokio::time::advance(HEAL_TIMEOUT + Duration::from_secs(1)).await;

        assert!(matches!(handle.await.unwrap(), Err(Error::SessionExpired)));
    }

    // A 401 inside the healer's own chain must come straight back, and only inside it: the
    // transport is cloned everywhere, so a flag on it would silence healing for every request
    // that merely overlapped a heal.
    #[tokio::test]
    async fn suspending_healing_covers_the_task_and_nothing_else() {
        let it = InnerTube::new(Session::default(), None).unwrap();
        assert!(!it.healing_suspended());

        let inside = it.clone();
        without_healing(async move {
            assert!(inside.healing_suspended());
            let spawned = inside.clone();
            // `tokio::spawn` does not inherit it, so work sent off the auth path heals as usual.
            tokio::spawn(async move { assert!(!spawned.healing_suspended()) }).await.unwrap();
        })
        .await;

        assert!(!it.healing_suspended(), "and it is gone once the scope ends");
    }

    // The #165 regression: the rotated value has to land in the jar, in place, or the login dies
    // a few hours in.
    #[test]
    fn a_rotated_cookie_replaces_the_stored_one() {
        let merged = merge_set_cookie(
            "SAPISID=keep; __Secure-3PSIDTS=old; PREF=x",
            &["__Secure-3PSIDTS=new; Path=/; Secure; HttpOnly", "YSC=fresh; Path=/"],
        );
        assert_eq!(
            merged.as_deref(),
            Some("SAPISID=keep; __Secure-3PSIDTS=new; PREF=x; YSC=fresh")
        );
    }

    /// A signed-in `InnerTube` talking to `server`, as the account `SAPISID=fixture-sapisid`.
    fn signed_in_against(server: &crate::test_server::MockServer) -> InnerTube {
        let session = Session {
            cookie: Some("SAPISID=fixture-sapisid; PREF=x".into()),
            data_sync_id: Some("fixture-dsid".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();
        it.set_base_url(&server.base_url);
        it
    }

    /// The point of `post_no_heal`: a refusal comes back to the caller, and the healer (which
    /// would open a hidden webview and may remount the page) is never told. The same request
    /// through `post` is the control: it does wake it, so the silence is meaningful.
    #[tokio::test]
    async fn a_refusal_through_post_no_heal_never_wakes_the_healer() {
        let server = crate::test_server::MockServer::start(|_| (401, "{}".into()));
        let it = signed_in_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let rejected = it.session_rejected();

        let got = it.post_no_heal("next", web, &serde_json::json!({}), true).await;
        assert!(
            matches!(&got, Err(Error::Http(e)) if e.status() == Some(reqwest::StatusCode::UNAUTHORIZED)),
            "the refusal is handed back as it is: {got:?}"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(150), rejected.notified()).await.is_err(),
            "session_rejected was raised"
        );
        assert_eq!(*it.heal.borrow(), HealState::default(), "no heal was started or awaited");
        assert!(it.is_logged_in(), "and nothing touched the session");

        // It was sent as the account: cookie and SAPISIDHASH both.
        let sent = server.requests();
        assert_eq!(sent.len(), 1, "no retry on a refusal");
        assert!(sent[0].header("cookie").is_some_and(|c| c.contains("SAPISID=")));
        assert!(sent[0].header("authorization").is_some_and(|a| a.starts_with("SAPISIDHASH ")));

        // Control: the ordinary path raises the signal on the same answer.
        let ordinary = it.clone();
        let web = web.clone();
        let parked =
            tokio::spawn(
                async move { ordinary.post("next", &web, &serde_json::json!({}), true).await },
            );
        tokio::time::timeout(Duration::from_secs(5), rejected.notified())
            .await
            .expect("post() raises session_rejected on a 401");
        parked.abort();
    }

    // ── stream: reliability ──

    const SORRY: &str = "<html><title>Sorry...</title>automated queries</html>";

    fn anonymous_against(server: &crate::test_server::MockServer) -> InnerTube {
        let it = InnerTube::new(Session::default(), None).unwrap();
        it.set_base_url(&server.base_url);
        it
    }

    /// `n` bot-wall answers, then 200s.
    fn sorry_then_ok(n: usize) -> crate::test_server::MockServer {
        let count = std::sync::atomic::AtomicUsize::new(0);
        crate::test_server::MockServer::start(move |_| {
            if count.fetch_add(1, Ordering::SeqCst) < n {
                (403, SORRY.into())
            } else {
                (200, r#"{"ok":1}"#.into())
            }
        })
    }

    /// The bug the soak test found: the bot wall's HTML 403 came straight back to the user.
    #[tokio::test]
    async fn the_bot_wall_is_retried_until_it_lets_the_request_through() {
        let server = sorry_then_ok(3);
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let got = it.post("search", web, &serde_json::json!({"q": 1}), false).await.unwrap();
        assert_eq!(got, serde_json::json!({"ok": 1}));
        assert_eq!(server.requests().len(), 4);
        let st = it.reliability_stats();
        assert_eq!((st.throttled, st.retries, st.gave_up), (3, 3, 0));
    }

    #[tokio::test]
    async fn a_user_action_gives_up_after_its_budget() {
        let server = sorry_then_ok(usize::MAX);
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let start = std::time::Instant::now();
        let got = it.post("next", web, &serde_json::json!({}), false).await;
        assert!(matches!(&got, Err(Error::Http(e)) if e.status().map(|s| s.as_u16()) == Some(403)));
        let budget = RetryPolicy::for_priority(crate::Priority::Foreground).attempts as usize;
        assert_eq!(server.requests().len(), budget);
        assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
        assert_eq!(it.reliability_stats().gave_up, 1);
    }

    /// The wall answers its 403 only after 5-10 s. An attempt that slow is abandoned and resent
    /// instead of waited out.
    #[tokio::test]
    async fn a_tarpitted_attempt_is_cut_short_and_resent() {
        let count = std::sync::atomic::AtomicUsize::new(0);
        let server = crate::test_server::MockServer::start(move |_| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_secs(4));
                (403, SORRY.into())
            } else {
                (200, r#"{"ok":1}"#.into())
            }
        });
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let got = it.post("search", web, &serde_json::json!({}), false).await.unwrap();
        assert_eq!(got["ok"], 1);
        // The mock serves one connection at a time, so the wall-clock win does not show here;
        // that the slow attempt was abandoned (not answered) is what this checks.
        assert_eq!(it.reliability_stats().cut_short, 1);
        assert_eq!(it.reliability_stats().gave_up, 0);
    }

    /// InnerTube's own JSON 403 is an answer, not a wall: resending gets the same one.
    #[tokio::test]
    async fn a_json_403_is_not_retried() {
        let server = crate::test_server::MockServer::start(|_| (403, "{}".into()));
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        assert!(it.post("search", web, &serde_json::json!({}), false).await.is_err());
        assert_eq!(server.requests().len(), 1);
    }

    /// Signed in, the wall's 403 is about the IP: it must not wake the healer (a hidden webview,
    /// a re-mint) nor sign anyone out. Before, it did both.
    #[tokio::test]
    async fn the_bot_wall_never_heals_or_expires_a_signed_in_session() {
        let server = sorry_then_ok(2);
        let it = signed_in_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let rejected = it.session_rejected();
        let got = it.post("browse", web, &serde_json::json!({}), true).await;
        assert!(got.is_ok(), "{got:?}");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rejected.notified()).await.is_err(),
            "the healer was woken"
        );
        assert!(it.is_logged_in());
    }

    /// A 5xx on a write may have landed: not resent. On a read it is.
    #[tokio::test]
    async fn a_server_error_is_resent_for_reads_only() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let server = crate::test_server::MockServer::start(|_| (503, "{}".into()));
        let it = anonymous_against(&server);
        assert!(it.post("like/like", web, &serde_json::json!({}), false).await.is_err());
        assert_eq!(server.requests().len(), 1, "a write is sent once");
        assert!(it.post("next", web, &serde_json::json!({}), false).await.is_err());
        assert!(server.requests().len() > 2, "a read is resent");
    }

    #[tokio::test]
    async fn identical_reads_in_flight_share_one_request_and_are_kept() {
        let server = crate::test_server::MockServer::start(|_| {
            std::thread::sleep(Duration::from_millis(150));
            (200, r#"{"ok":1}"#.into())
        });
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let body = serde_json::json!({"query": "abba"});
        let calls = (0..5).map(|_| it.post("search", web, &body, false));
        let got = futures_util::future::join_all(calls).await;
        assert!(got.iter().all(|r| r.as_ref().is_ok_and(|v| v["ok"] == 1)));
        assert_eq!(server.requests().len(), 1, "one request for five callers");
        // Kept: the same search a moment later is not sent again; a different one is.
        it.post("search", web, &body, false).await.unwrap();
        assert_eq!(server.requests().len(), 1);
        it.post("search", web, &serde_json::json!({"query": "abc"}), false).await.unwrap();
        assert_eq!(server.requests().len(), 2);
        let st = it.reliability_stats();
        assert_eq!((st.deduped, st.cache_hits), (4, 1));
        // A write is never shared or kept.
        it.post("like/like", web, &body, false).await.unwrap();
        it.post("like/like", web, &body, false).await.unwrap();
        assert_eq!(server.requests().len(), 4);
    }

    /// Signed-in reads are shared while in flight but never kept: a playlist edited a moment ago
    /// must not come back from a cache.
    #[tokio::test]
    async fn signed_in_reads_are_not_kept() {
        let server = crate::test_server::MockServer::start(|_| (200, r#"{"ok":1}"#.into()));
        let it = signed_in_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let body = serde_json::json!({"browseId": "VLPLx"});
        it.post("browse", web, &body, true).await.unwrap();
        it.post("browse", web, &body, true).await.unwrap();
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn signing_in_or_out_drops_kept_answers() {
        let server = crate::test_server::MockServer::start(|_| (200, r#"{"ok":1}"#.into()));
        let it = anonymous_against(&server);
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let body = serde_json::json!({"query": "x"});
        it.post("search", web, &body, false).await.unwrap();
        it.set_cookie(Some("SAPISID=a".into()));
        it.set_cookie(None);
        it.post("search", web, &body, false).await.unwrap();
        assert_eq!(server.requests().len(), 2);
    }
    // ── end stream: reliability ──

    #[test]
    fn nothing_new_means_no_rewrite_and_no_deletions_applied() {
        // Same values back, plus a deletion we deliberately ignore: no change, so no disk write
        // and no chance of dropping the login.
        assert_eq!(
            merge_set_cookie("SAPISID=keep; PREF=x", &["PREF=x; Path=/", "SAPISID=; Max-Age=0"]),
            None
        );
    }
}
