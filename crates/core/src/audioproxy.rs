//! Loopback HTTP proxy for the *audio* stream: mpv fetches its bytes from here, never straight from
//! googlevideo.
//!
//! **Why this exists.** googlevideo throttles a *streaming* download (the open-ended
//! `Range: bytes=X-`, or a plain GET, that ffmpeg sends) to roughly 2x realtime. A *bounded* range
//! (`bytes=X-Y`) is served at full speed instead: measured against the same URL, the middle of a
//! 60 MB mix came back at 32 KB/s open-ended and 9.4 MB/s for a 4 MiB bounded range, and a whole
//! 122 MB mix pulled in chunks took 51 s (~2.4 MB/s) instead of hours. So the throttle is on the *request shape*, not the client: VISIONOS throttles exactly
//! like WEB_REMIX, and the URLs carry no `n`-parameter to blame.
//!
//! ffmpeg's HTTP source cannot be told to chunk its reads. This module can: mpv opens
//! `http://127.0.0.1:<ephemeral>/<token>/a/<id>`, and the proxy answers its open-ended request by
//! fetching bounded chunks upstream and streaming them back. Seeks become new loopback requests
//! that are served at full speed (a few tens of ms for the first ~40 KB a WebM needs to restart)
//! instead of waiting ~1.3 s for the same bytes at 2x realtime, and mpv's forward cache fills in
//! seconds rather than at playback rate.
//!
//! Same shape as [`crate::videoproxy`]: loopback-only, a random per-launch token in the path so no
//! other local process can drive it, and upstream headers (User-Agent, and the cookie an upload
//! needs) attached here rather than by mpv.
//!
//! Set `LIMUSIC_NO_AUDIO_PROXY=1` to fall back to handing mpv the googlevideo URL directly.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, StreamBody};
use hyper::body::{Bytes, Frame, Incoming};
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::sync::mpsc;

use crate::loopback::{Health, Loopback};

/// Streamed, never buffered: only one upstream chunk lives in RAM at a time, so a two-hour mix
/// costs the same few MB as a three-minute song.
type ProxyBody = BoxBody<Bytes, io::Error>;

/// How much of the file to ask upstream for in one range request. Large enough to amortise the
/// per-request round trip and land in the "served at full speed" regime, small enough that a seek
/// abandons little work when mpv drops the connection.
const CHUNK: u64 = 4 * 1024 * 1024;

/// How long one upstream read may make no progress before the chunk is declared dead.
///
/// It has to be here: `http::client()` sets no timeout on purpose (they are per call site) and
/// reqwest's read timeout is client-wide, so nothing else bounds this. Without it a connection that
/// goes quiet without closing hangs the pump forever, and mpv is left holding a loopback socket
/// that neither errors nor delivers a byte, with no track-failed event and so no recovery: the
/// silent stall issue #188 is about, moved inside our own process.
///
/// A stall timeout, not a total one. A whole-request deadline would have to be generous enough for
/// a 4 MiB chunk on a slow connection, which is long enough to be useless as a hang guard; this
/// fires on no progress, so a slow-but-moving download is never cut off.
const STALL: Duration = Duration::from_secs(20);

/// `send()` (or one body read) with the stall guard on it.
pub async fn no_stall<F: std::future::Future<Output = T>, T>(f: F) -> Result<T, io::Error> {
    tokio::time::timeout(STALL, f).await.map_err(|_| io::Error::other("upstream stalled"))
}

/// Keep at most this many stream URLs registered. A registration is the URL plus its headers (a
/// cookie on an upload), so this is a few KB; the FIFO only has to be deeper than the current and
/// gapless-next tracks, which are always the two most recent.
const MAX_REGISTRATIONS: usize = 64;

/// The running server (its port can change: [`ensure_alive`]). Unset if the first bind failed,
/// which just means [`register`] returns `None` and callers keep the direct URL.
static SERVER: OnceLock<Loopback> = OnceLock::new();

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

/// What a resolved URL answers: its total length and content type. Learned from the first chunk
/// response's `Content-Range` and reused for every later connection, so a seek does not pay a
/// separate round trip just to learn the size.
#[derive(Clone)]
struct Meta {
    total: u64,
    content_type: Option<String>,
}

struct Registration {
    url: String,
    headers: HashMap<String, String>,
    meta: Mutex<Option<Meta>>,
    /// stream: firstaudio. The stream's first bytes, fetched ahead by [`warm`] (or in flight).
    head: Option<HeadCell>,
}

// ── stream: firstaudio ── the head of a stream, fetched before mpv asks for it.
//
// mpv's first request is the one a click waits on, and without this it costs an upstream round
// trip (two before the first-chunk change below) before ffmpeg sees a byte. ExoPlayer and
// Metrolist hide it by pre-buffering the start of likely tracks; this is that, at its smallest:
// the first [`HEAD`] bytes of a URL, fetched while the resolve validates it, while a hovered row
// waits to be clicked, or while the current track plays the next one's lookahead. The open is
// then served from memory and the pump carries on upstream from where the head ends.

/// How much of a stream's start to fetch ahead: ~8 s of 128 kbit/s Opus, far more than ffmpeg reads
/// to open the file and start decoding, and one round trip at full speed.
const HEAD: u64 = 128 * 1024;

/// Heads kept. A few hover hints plus the playing and next tracks; 1 MiB at most.
const MAX_HEADS: usize = 8;

/// A head is only worth its URL, and those live ~6 h; 30 min keeps the cache honest.
const HEAD_TTL: Duration = Duration::from_secs(30 * 60);

struct Head {
    bytes: Bytes,
    meta: Meta,
}

/// Filled once by whoever gets there first: the warm task, or the request that needs it.
type HeadCell = Arc<tokio::sync::OnceCell<Option<Arc<Head>>>>;

struct Warm {
    url: String,
    headers: HashMap<String, String>,
    cell: HeadCell,
    at: std::time::Instant,
}

static HEADS: Mutex<VecDeque<Warm>> = Mutex::new(VecDeque::new());

/// Whether a stream URL would go through this proxy at all (and so is worth warming).
fn proxied(url: &str) -> bool {
    crate::orchestrator::is_youtube_stream(url)
        && std::env::var_os("LIMUSIC_NO_AUDIO_PROXY").is_none()
        && !crate::http::has_proxy()
        && SERVER.get().is_some()
}

/// Start fetching the first [`HEAD`] bytes of `url` in the background, so a later [`register`] of
/// the same URL is served from memory. A no-op for a URL already warm, or one the proxy would not
/// carry. Costs one bounded range request to googlevideo, never an InnerTube call.
pub fn warm(url: &str, headers: &HashMap<String, String>) {
    if !proxied(url) {
        return;
    }
    let cell: HeadCell = {
        let Ok(mut heads) = HEADS.lock() else { return };
        heads.retain(|w| w.at.elapsed() < HEAD_TTL);
        if heads.iter().any(|w| w.url == url) {
            return;
        }
        while heads.len() >= MAX_HEADS {
            heads.pop_front();
        }
        let cell = HeadCell::default();
        heads.push_back(Warm {
            url: url.to_owned(),
            headers: headers.clone(),
            cell: cell.clone(),
            at: std::time::Instant::now(),
        });
        cell
    };
    let (url, headers) = (url.to_owned(), headers.clone());
    crate::rt::spawn(async move {
        cell.get_or_init(|| fetch_head(url, headers)).await;
    });
}

tokio::task_local! {
    /// Set (by `AppState::resolve`) around a resolve whose stream is about to be played: the
    /// orchestrator then warms the chosen URL's head while it validates it. A download's resolve
    /// runs without it and costs nothing extra.
    pub static WANT_HEAD: bool;
}

/// Whether the resolve running on this task wants its stream's head warmed.
pub fn wants_head() -> bool {
    WANT_HEAD.try_with(|w| *w).unwrap_or(false)
}

/// The warm head of `url`, if one was started (it may still be in flight).
fn head_for(url: &str) -> Option<(HeadCell, HashMap<String, String>)> {
    let heads = HEADS.lock().ok()?;
    heads
        .iter()
        .find(|w| w.url == url && w.at.elapsed() < HEAD_TTL)
        .map(|w| (w.cell.clone(), w.headers.clone()))
}

/// Forget a URL's head: it was refused (expired, 403), so its bytes describe nothing playable.
fn drop_head(url: &str) {
    if let Ok(mut heads) = HEADS.lock() {
        heads.retain(|w| w.url != url);
    }
}

async fn fetch_head(url: String, headers: HashMap<String, String>) -> Option<Arc<Head>> {
    let resp = no_stall(upstream_request_to(&url, &headers, 0, HEAD - 1).send()).await.ok()?.ok()?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        tracing::debug!(status = %resp.status(), "audio proxy: head refused");
        drop_head(&url);
        return None;
    }
    let total = resp
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(total_from_content_range)?;
    let meta = Meta { total, content_type: content_type(&resp) };
    let mut buf = Vec::with_capacity(HEAD as usize);
    let mut body = resp.bytes_stream();
    while let Some(chunk) = no_stall(body.next()).await.ok()? {
        buf.extend_from_slice(&chunk.ok()?);
    }
    // A short head is fine only when it is the whole file.
    if (buf.len() as u64) < HEAD.min(total) {
        return None;
    }
    Some(Arc::new(Head { bytes: Bytes::from(buf), meta }))
}

/// The registration's head, waiting for it if it is still in flight (or fetching it here if the
/// warm task has not started yet: the cell makes either one the only fetch).
async fn head_of(reg: &Registration) -> Option<Arc<Head>> {
    let cell = reg.head.as_ref()?;
    let (url, headers) = (reg.url.clone(), reg.headers.clone());
    cell.get_or_init(|| fetch_head(url, headers)).await.clone()
}
// ── end stream: firstaudio ──

struct Registry {
    map: HashMap<String, Arc<Registration>>,
    order: VecDeque<String>,
}

/// Bind the loopback listener and start serving. Binds synchronously so [`register`] is usable the
/// moment this returns, then hands the socket to tokio.
pub fn start() {
    let accept: crate::loopback::Accept = Arc::new(|stream| {
        crate::rt::spawn(async move {
            let svc = service_fn(serve);
            let _ = http1::Builder::new()
                // hyper 1.x panics when it arms a timeout without a timer installed.
                .timer(TokioTimer::new())
                .header_read_timeout(Duration::from_secs(15))
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    });
    let server = match Loopback::start("audio proxy", accept) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "audio proxy: bind failed (streams go direct)");
            return;
        }
    };
    let _ = REGISTRY.set(Mutex::new(Registry { map: HashMap::new(), order: VecDeque::new() }));
    let _ = SERVER.set(server);
}

/// Check the listener and rebuild it if the system took it away (iOS does, from a suspended app).
/// `None`: the proxy never came up, so there is nothing to check.
pub fn ensure_alive() -> Option<Health> {
    SERVER.get().map(Loopback::ensure_alive)
}

/// Debug hook: the next [`register`] hands its URL out and then kills the listener, so mpv opens a
/// URL on a dead port: the on-failure path end to end (`spotless://debug?audio=kill-proxy-on-load`).
pub static KILL_ON_NEXT_LOAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Debug hook: close the listener as iOS would (`spotless://debug?audio=kill-proxy`).
pub fn kill() {
    if let Some(s) = SERVER.get() {
        s.kill();
    }
}

/// Register one YouTube stream URL (with the headers it needs) and return the loopback URL mpv
/// should be handed. `None` means the direct URL must be used: the proxy never came up, the
/// kill-switch is set, or the URL is not one of YouTube's stream hosts (an RSS-feed podcast's
/// enclosure is not throttled, and may not honour ranges, #294).
pub fn register(url: &str, headers: &HashMap<String, String>) -> Option<String> {
    if !crate::orchestrator::is_youtube_stream(url)
        || std::env::var_os("LIMUSIC_NO_AUDIO_PROXY").is_some()
    {
        return None;
    }
    let server = SERVER.get()?;
    // The health check before every load: a URL on a dead port would fail as a "bad track".
    server.ensure_alive();
    let (port, token) = (server.port()?, server.token());
    let id = format!("{:016x}", rand::random::<u64>());
    let reg = Arc::new(Registration {
        url: url.to_owned(),
        headers: headers.clone(),
        meta: Mutex::new(None),
        // stream: firstaudio. Only a head fetched with the same headers stands in for this fetch.
        head: head_for(url).filter(|(_, h)| h == headers).map(|(cell, _)| cell),
    });
    {
        let mut r = REGISTRY.get()?.lock().ok()?;
        while r.order.len() >= MAX_REGISTRATIONS {
            if let Some(old) = r.order.pop_front() {
                r.map.remove(&old);
            }
        }
        r.map.insert(id.clone(), reg);
        r.order.push_back(id.clone());
    }
    if KILL_ON_NEXT_LOAD.swap(false, std::sync::atomic::Ordering::SeqCst) {
        server.kill();
    }
    Some(format!("http://127.0.0.1:{port}/{token}/a/{id}"))
}

/// `/<token>/<kind>/<id>` to `(kind, id)`, once the token matches this launch's.
fn route<'a>(path: &'a str, token: &str) -> Option<(&'a str, &'a str)> {
    let (t, rest) = path.strip_prefix('/')?.split_once('/')?;
    let (kind, id) = rest.split_once('/')?;
    (t == token && !id.is_empty()).then_some((kind, id))
}

/// `/<token>/a/<id>` to the registration id.
fn id_from<'a>(path: &'a str, token: &str) -> Option<&'a str> {
    route(path, token).filter(|(kind, _)| *kind == "a").map(|(_, id)| id)
}

fn empty(status: StatusCode) -> Response<ProxyBody> {
    let mut r = Response::new(Empty::<Bytes>::new().map_err(|e| match e {}).boxed());
    *r.status_mut() = status;
    r
}

async fn serve(req: Request<Incoming>) -> Result<Response<ProxyBody>, Infallible> {
    Ok(handle(req).await.unwrap_or_else(empty))
}

async fn handle(req: Request<Incoming>) -> Result<Response<ProxyBody>, StatusCode> {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let token = SERVER.get().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?.token();
    let id = id_from(req.uri().path(), token).ok_or(StatusCode::NOT_FOUND)?;
    let reg = {
        let r = REGISTRY
            .get()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        r.map.get(id).cloned()
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    crate::firstaudio::mark("proxy:request"); // stream: firstaudio
    serve_registration(reg, &req).await
}

/// Registrations the video proxy made, newest last. The `<video>` element opens a stream with
/// several range requests and reopens it on every seek, so one registration per URL learns the
/// length once for all of them.
static BY_URL: Mutex<VecDeque<Arc<Registration>>> = Mutex::new(VecDeque::new());

/// The video proxy's way in (videoproxy.rs): serve `url` to a `<video>` element exactly as mpv's
/// audio is served, in bounded upstream chunks. Forwarding the element's open-ended
/// `Range: bytes=X-` untouched, as that proxy used to, is the request shape googlevideo throttles
/// to about realtime (see the top of this file), which is what left podcast video stuck buffering
/// on the Mac while its audio, through here, played on.
pub(crate) async fn serve_url(
    url: &str,
    req: &Request<Incoming>,
) -> Result<Response<ProxyBody>, StatusCode> {
    let reg = {
        let mut regs = BY_URL.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        match regs.iter().find(|r| r.url == url) {
            Some(r) => r.clone(),
            None => {
                let r = Arc::new(Registration {
                    url: url.to_owned(),
                    headers: HashMap::new(),
                    meta: Mutex::new(None),
                    head: None,
                });
                if regs.len() >= 4 {
                    regs.pop_front();
                }
                regs.push_back(r.clone());
                r
            }
        }
    };
    serve_registration(reg, req).await
}

/// Serve one request for `reg`: the client's range, fetched upstream in bounded chunks.
async fn serve_registration(
    reg: Arc<Registration>,
    req: &Request<Incoming>,
) -> Result<Response<ProxyBody>, StatusCode> {
    let want = parse_range(req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()));
    let is_head = req.method() == Method::HEAD;

    // ── stream: firstaudio ── where the opening bytes come from, cheapest first: the warm head in
    // memory; else, for a registration whose length is not known yet, the first chunk itself
    // (its `Content-Range` carries the length, which used to cost a separate `bytes=0-0` probe and
    // so a second serial round trip before mpv saw a byte); else the probe, as before.
    let head = head_of(&reg).await;
    let mut first: Option<reqwest::Response> = None;
    let meta = match (reg.meta.lock().ok().and_then(|g| g.clone()), &head) {
        (Some(m), _) => m,
        (None, Some(h)) => h.meta.clone(),
        (None, None) => match first_start(want).filter(|_| !is_head) {
            Some((s, last)) => match first_chunk(&reg, s, last).await? {
                Some((m, resp)) => {
                    first = Some(resp);
                    m
                }
                None => meta_for(&reg).await?,
            },
            None => meta_for(&reg).await?,
        },
    };
    if let Ok(mut g) = reg.meta.lock() {
        g.get_or_insert_with(|| meta.clone());
    }
    crate::firstaudio::mark("proxy:meta"); // stream: firstaudio
    // ── end stream: firstaudio ──
    let Some((start, end)) = resolve_window(want, meta.total) else {
        // The only malformed-but-well-formed case: a range that starts past the end of the file.
        let mut r = empty(StatusCode::RANGE_NOT_SATISFIABLE);
        if let Ok(v) = format!("bytes */{}", meta.total).parse() {
            r.headers_mut().insert(header::CONTENT_RANGE, v);
        }
        return Ok(r);
    };

    let ranged = !matches!(want, Want::Full);
    let status = if ranged { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    let len = end - start + 1;

    let mut builder = Response::builder()
        .status(status.as_u16())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len);
    if let Some(ct) = meta.content_type.as_deref() {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    if ranged {
        builder =
            builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{}", meta.total));
    }

    let body = if is_head {
        Empty::<Bytes>::new().map_err(|e| match e {}).boxed()
    } else {
        // Pull upstream in a task and hand the bytes over a bounded channel: the client body then
        // holds only the receiver (Send + Sync, which `boxed` requires), and when mpv drops the
        // connection the receiver drops, the send fails, and the task stops fetching.
        let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(16);
        let pump = reg.clone();
        // stream: firstaudio. What is already in hand goes out first: the part of the warm head
        // inside the window, or the first chunk's response (which starts exactly at `start`).
        let prefix = head
            .filter(|h| start < h.bytes.len() as u64)
            .map(|h| h.bytes.slice(start as usize..h.bytes.len().min(end as usize + 1)));
        if prefix.is_some() {
            crate::firstaudio::mark("proxy:head"); // stream: firstaudio
        }
        crate::rt::spawn(async move {
            pump_chunks(pump, start, end, tx, prefix, first).await
        });
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item.map(Frame::data), rx))
        });
        BodyExt::boxed(StreamBody::new(stream))
    };
    builder.body(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// The cached [`Meta`], probing upstream once per registration if nothing has learned it yet.
async fn meta_for(reg: &Registration) -> Result<Meta, StatusCode> {
    if let Some(m) = reg.meta.lock().ok().and_then(|g| g.clone()) {
        return Ok(m);
    }
    let m = probe(reg).await?;
    if let Ok(mut g) = reg.meta.lock() {
        *g = Some(m.clone());
    }
    Ok(m)
}

/// Ask upstream for a single byte (`bytes=0-0`) just to read the total length and content type off
/// the answer. `meta_for` keeps the result, so only the first connection to a registration pays it.
async fn probe(reg: &Registration) -> Result<Meta, StatusCode> {
    let resp = upstream_get(reg, 0, 0).await?;
    let status = resp.status();
    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        let total = resp
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(total_from_content_range)
            .ok_or(StatusCode::BAD_GATEWAY)?;
        Ok(Meta { total, content_type: content_type(&resp) })
    } else {
        // Anything but a 206 is a dead end here, including a 200 from a server that ignored the
        // range: every byte this proxy serves comes from a bounded range (`pump_chunks` accepts
        // only 206), so taking the whole-file answer would just move the failure from this handled
        // spot to an I/O error mid-body. Only googlevideo URLs are ever registered and those
        // honour ranges, so in practice this is an expired URL (they last ~6h) or a refusal. mpv
        // errors and the app re-resolves.
        tracing::debug!(status = %status, "audio proxy: upstream refused");
        Err(StatusCode::BAD_GATEWAY)
    }
}

/// A bounded upstream range request with the registration's headers. Range and content-encoding do
/// not mix: without the explicit `identity` reqwest offers gzip/br and the server could answer 200
/// with an encoded whole file, which is exactly the throttled shape this proxy exists to avoid.
fn upstream_request(reg: &Registration, start: u64, end: u64) -> reqwest::RequestBuilder {
    upstream_request_to(&reg.url, &reg.headers, start, end)
}

fn upstream_request_to(
    url: &str,
    headers: &HashMap<String, String>,
    start: u64,
    end: u64,
) -> reqwest::RequestBuilder {
    let mut req = crate::http::client()
        .get(url)
        .header(header::RANGE.as_str(), format!("bytes={start}-{end}"))
        .header(header::ACCEPT_ENCODING.as_str(), "identity");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    req
}

/// stream: firstaudio. The first chunk a request with no known length would fetch: from its start,
/// one [`CHUNK`] or to its own end. `None` for a suffix range, which needs the length first.
fn first_start(want: Want) -> Option<(u64, u64)> {
    match want {
        Want::Full => Some((0, CHUNK - 1)),
        Want::From(s) => Some((s, s + CHUNK - 1)),
        Want::Bounded(s, e) => Some((s, e.min(s + CHUNK - 1))),
        Want::Suffix(_) => None,
    }
}

/// stream: firstaudio. Fetch `start..=last` and learn the length from its `Content-Range`, in one
/// round trip. `Ok(None)` when the answer is not a usable 206 (a 416 for a start past the end, a
/// refusal), which the caller hands to the old probe so those keep their exact old answers.
async fn first_chunk(
    reg: &Registration,
    start: u64,
    last: u64,
) -> Result<Option<(Meta, reqwest::Response)>, StatusCode> {
    let resp = upstream_get(reg, start, last).await?;
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Ok(None);
    }
    let Some(total) = resp
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(total_from_content_range)
    else {
        return Ok(None);
    };
    Ok(Some((Meta { total, content_type: content_type(&resp) }, resp)))
}

async fn upstream_get(
    reg: &Registration,
    start: u64,
    end: u64,
) -> Result<reqwest::Response, StatusCode> {
    let sent = no_stall(upstream_request(reg, start, end).send()).await.map_err(|e| {
        tracing::debug!(error = %e, "audio proxy: upstream stalled");
        StatusCode::GATEWAY_TIMEOUT
    })?;
    sent.map_err(|e| {
        tracing::debug!(error = %e, "audio proxy: upstream failed");
        StatusCode::BAD_GATEWAY
    })
}

fn content_type(resp: &reqwest::Response) -> Option<String> {
    resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

fn total_from_content_range(value: &str) -> Option<u64> {
    // `bytes 0-0/122414474`, or `bytes 0-0/*` when the length is unknown.
    value.rsplit_once('/')?.1.trim().parse().ok()
}

/// The last byte to ask for in a chunk that starts at `pos`, clamped to the window's last byte.
fn chunk_end(pos: u64, end: u64) -> u64 {
    (pos + CHUNK - 1).min(end)
}

/// Walk `pos` to `end` inclusive in bounded upstream ranges, forwarding each response's bytes into
/// the channel. Ends when the window is done or the receiver is gone (a seek dropped the
/// connection), which is also what cancels the in-flight upstream request.
async fn pump_chunks(
    reg: Arc<Registration>,
    start: u64,
    end: u64,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
    prefix: Option<Bytes>,
    mut first: Option<reqwest::Response>,
) {
    let mut pos = start;
    // stream: firstaudio. Bytes already in memory (the warm head) go out before any upstream I/O.
    if let Some(p) = prefix {
        pos += p.len() as u64;
        if tx.send(Ok(p)).await.is_err() {
            return;
        }
    }
    while pos <= end {
        let last = chunk_end(pos, end);
        // stream: firstaudio. The first chunk may already be in flight (fetched to learn the length).
        let sent = match first.take() {
            Some(r) => Ok(Ok(r)),
            None => no_stall(upstream_request(&reg, pos, last).send()).await,
        };
        let resp = match sent {
            Ok(Ok(r)) if r.status() == reqwest::StatusCode::PARTIAL_CONTENT => r,
            Ok(Ok(r)) => {
                let _ = tx
                    .send(Err(io::Error::other(format!(
                        "audio proxy: upstream status {}",
                        r.status()
                    ))))
                    .await;
                return;
            }
            Ok(Err(e)) => {
                let _ = tx.send(Err(io::Error::other(e))).await;
                return;
            }
            Err(e) => {
                let _ = tx.send(Err(e)).await;
                return;
            }
        };
        pos = last + 1;
        crate::firstaudio::mark("proxy:chunk-headers"); // stream: firstaudio
        let mut body = resp.bytes_stream();
        loop {
            let item = match no_stall(body.next()).await {
                Ok(Some(item)) => item.map_err(io::Error::other),
                Ok(None) => break,
                Err(e) => Err(e),
            };
            let failed = item.is_err();
            if tx.send(item).await.is_err() {
                return; // client gone, stop pulling and drop the connection upstream
            }
            if failed {
                return;
            }
        }
    }
}

/// What the client asked for, once parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// No `Range` header, or one this proxy does not understand: the whole file.
    Full,
    /// `bytes=X-`
    From(u64),
    /// `bytes=X-Y`
    Bounded(u64, u64),
    /// `bytes=-N` (the last N bytes)
    Suffix(u64),
}

fn parse_range(value: Option<&str>) -> Want {
    let Some(v) = value else { return Want::Full };
    let Some(spec) = v.strip_prefix("bytes=") else { return Want::Full };
    // A multi-range request is legal but nothing here (or in ffmpeg) sends one; take the first.
    let spec = spec.split(',').next().unwrap_or(spec).trim();
    let Some((a, b)) = spec.split_once('-') else { return Want::Full };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        return match b.parse() {
            Ok(n) => Want::Suffix(n),
            Err(_) => Want::Full,
        };
    }
    let Ok(start) = a.parse() else { return Want::Full };
    if b.is_empty() {
        Want::From(start)
    } else {
        match b.parse::<u64>() {
            Ok(end) if end >= start => Want::Bounded(start, end),
            _ => Want::Full,
        }
    }
}

/// The inclusive window to serve for `want`, or `None` when the request cannot be satisfied.
fn resolve_window(want: Want, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    match want {
        Want::Full => Some((0, total - 1)),
        Want::From(s) => (s < total).then_some((s, total - 1)),
        Want::Bounded(s, e) => (s < total).then(|| (s, e.min(total - 1))),
        Want::Suffix(n) => (n > 0).then(|| (total.saturating_sub(n), total - 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_right_token_and_kind_route() {
        assert_eq!(id_from("/abc/a/deadbeef", "abc"), Some("deadbeef"));
        assert_eq!(id_from("/wrong/a/deadbeef", "abc"), None);
        assert_eq!(id_from("/abc/x/deadbeef", "abc"), None);
        assert_eq!(id_from("/abc/a/", "abc"), None);
        assert_eq!(id_from("/abc/a", "abc"), None);
        assert_eq!(route("/abc/k/beef", "abc"), Some(("k", "beef")));
        assert_eq!(id_from("/abc/k/beef", "abc"), None, "a stream id is not an audio one");
    }

    #[test]
    fn ranges_parse_the_ways_ffmpeg_sends_them() {
        assert_eq!(parse_range(None), Want::Full);
        assert_eq!(parse_range(Some("bytes=0-")), Want::From(0));
        assert_eq!(parse_range(Some("bytes=1234-")), Want::From(1234));
        assert_eq!(parse_range(Some("bytes=10-19")), Want::Bounded(10, 19));
        assert_eq!(parse_range(Some("bytes=0-0")), Want::Bounded(0, 0));
        assert_eq!(parse_range(Some("bytes=-500")), Want::Suffix(500));
        // Anything unusable is treated as no range, per the spec's ignore-unknown rule.
        assert_eq!(parse_range(Some("items=0-")), Want::Full);
        assert_eq!(parse_range(Some("bytes=oops")), Want::Full);
        assert_eq!(parse_range(Some("bytes=19-10")), Want::Full);
    }

    #[test]
    fn windows_clamp_and_reject() {
        assert_eq!(resolve_window(Want::Full, 1000), Some((0, 999)));
        assert_eq!(resolve_window(Want::From(0), 1000), Some((0, 999)));
        assert_eq!(resolve_window(Want::From(500), 1000), Some((500, 999)));
        assert_eq!(resolve_window(Want::Bounded(0, 5000), 1000), Some((0, 999)));
        assert_eq!(resolve_window(Want::Suffix(100), 1000), Some((900, 999)));
        assert_eq!(resolve_window(Want::From(1000), 1000), None);
        assert_eq!(resolve_window(Want::Suffix(0), 1000), None);
        assert_eq!(resolve_window(Want::Full, 0), None);
    }

    /// The chunks have to tile the window exactly: no gap to stall on, no overlap to duplicate.
    #[test]
    fn chunks_tile_the_window() {
        assert_eq!(chunk_end(0, 10 * 1024 * 1024), CHUNK - 1);
        assert_eq!(chunk_end(CHUNK, 10 * 1024 * 1024), 2 * CHUNK - 1);
        // Last chunk is clamped to the window's inclusive last byte, not one before it.
        assert_eq!(chunk_end(8 * 1024 * 1024, 10 * 1024 * 1024), 10 * 1024 * 1024);
        assert_eq!(chunk_end(0, 99), 99);

        let mut pos = 0u64;
        let end = 9 * 1024 * 1024 + 123;
        let mut seen = 0u64;
        while pos <= end {
            let last = chunk_end(pos, end);
            assert!(last <= end);
            seen += last - pos + 1;
            pos = last + 1;
        }
        assert_eq!(pos, end + 1);
        assert_eq!(seen, end + 1);
    }

    #[test]
    fn the_first_chunk_starts_where_mpv_asked() {
        assert_eq!(first_start(Want::Full), Some((0, CHUNK - 1)));
        assert_eq!(first_start(Want::From(500)), Some((500, 500 + CHUNK - 1)));
        assert_eq!(first_start(Want::Bounded(10, 19)), Some((10, 19)));
        assert_eq!(first_start(Want::Bounded(0, 10 * CHUNK)), Some((0, CHUNK - 1)));
        assert_eq!(first_start(Want::Suffix(100)), None, "needs the length first");
    }

    /// A loopback stand-in for googlevideo: answers `bytes=a-b` with a 206 of the pattern file.
    async fn range_server(file: Arc<Vec<u8>>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut c, _)) = l.accept().await else { return };
                let file = file.clone();
                tokio::spawn(async move {
                    loop {
                        let mut req = Vec::new();
                        let mut b = [0u8; 1];
                        while !req.ends_with(b"\r\n\r\n") {
                            if c.read(&mut b).await.unwrap_or(0) == 0 {
                                return;
                            }
                            req.push(b[0]);
                        }
                        let req = String::from_utf8_lossy(&req).to_lowercase();
                        let range = req.lines().find_map(|l| l.strip_prefix("range: bytes=")).unwrap();
                        let (a, z) = range.trim().split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let z = z.parse::<usize>().unwrap().min(file.len() - 1);
                        let head = format!(
                            "HTTP/1.1 206 Partial Content\r\ncontent-length: {}\r\ncontent-range: bytes {a}-{z}/{}\r\ncontent-type: audio/webm\r\n\r\n",
                            z - a + 1,
                            file.len()
                        );
                        c.write_all(head.as_bytes()).await.unwrap();
                        c.write_all(&file[a..=z]).await.unwrap();
                    }
                });
            }
        });
        format!("http://{addr}/videoplayback")
    }

    async fn drain(mut rx: mpsc::Receiver<Result<Bytes, io::Error>>) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some(b) = rx.recv().await {
            out.extend_from_slice(&b.unwrap());
        }
        out
    }

    /// The warm head, then the pump from where it ends: byte for byte the file, from any start.
    #[tokio::test]
    async fn a_warm_head_and_the_pump_tile_the_file() {
        let file: Arc<Vec<u8>> = Arc::new((0..300_000u32).map(|i| (i * 7 % 251) as u8).collect());
        let url = range_server(file.clone()).await;
        let head = fetch_head(url.clone(), HashMap::new()).await.expect("head");
        assert_eq!(head.meta.total, file.len() as u64);
        assert_eq!(head.meta.content_type.as_deref(), Some("audio/webm"));
        assert_eq!(&head.bytes[..], &file[..HEAD as usize]);

        for start in [0u64, 1000, HEAD - 1, HEAD, 200_000] {
            let reg = Arc::new(Registration {
                url: url.clone(),
                headers: HashMap::new(),
                meta: Mutex::new(Some(head.meta.clone())),
                head: None,
            });
            let end = file.len() as u64 - 1;
            let prefix = (start < HEAD).then(|| head.bytes.slice(start as usize..));
            let (tx, rx) = mpsc::channel(16);
            tokio::spawn(pump_chunks(reg, start, end, tx, prefix, None));
            assert_eq!(drain(rx).await, file[start as usize..], "start {start}");
        }
    }

    /// The first chunk fetched to learn the length is served, not fetched twice or skipped.
    #[tokio::test]
    async fn the_length_comes_with_the_first_chunk() {
        let file: Arc<Vec<u8>> = Arc::new((0..50_000u32).map(|i| (i % 253) as u8).collect());
        let url = range_server(file.clone()).await;
        let reg = Arc::new(Registration {
            url,
            headers: HashMap::new(),
            meta: Mutex::new(None),
            head: None,
        });
        let (s, last) = first_start(Want::From(123)).unwrap();
        let (meta, resp) = first_chunk(&reg, s, last).await.unwrap().expect("206");
        assert_eq!(meta.total, 50_000);
        let (start, end) = resolve_window(Want::From(123), meta.total).unwrap();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(pump_chunks(reg, start, end, tx, None, Some(resp)));
        assert_eq!(drain(rx).await, file[123..]);
    }

    // Regression for the owner's phone: kill the listener the way iOS does, then register the next
    // track. The registration's health check rebuilds the proxy, and the URL it hands out answers
    // (the route is reached: an unknown id is a 404, not a refused connection).
    #[test]
    fn a_killed_proxy_is_rebuilt_before_the_next_url_is_handed_out() {
        start();
        let server = SERVER.get().expect("proxy up");
        let port = server.port().unwrap();
        kill();
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        for _ in 0..100 {
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let url = register("https://rr1---sn-abc.googlevideo.com/videoplayback?id=1", &HashMap::new())
            .expect("registered");
        let new_port = server.port().unwrap();
        assert!(url.starts_with(&format!("http://127.0.0.1:{new_port}/")));
        let probe = format!("http://127.0.0.1:{new_port}/{}/a/unknown", server.token());
        let status = crate::rt::block_on(async { reqwest::get(&probe).await.map(|r| r.status()) });
        assert_eq!(status.expect("proxy answers").as_u16(), 404);
        assert_eq!(ensure_alive(), Some(Health::Alive));
    }

    #[test]
    fn total_is_read_from_content_range() {
        assert_eq!(total_from_content_range("bytes 0-0/122414474"), Some(122414474));
        assert_eq!(total_from_content_range("bytes 0-0/*"), None);
        assert_eq!(total_from_content_range("nonsense"), None);
    }
}
