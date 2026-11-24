//! Loopback HTTP proxy for the player view's music video: the `<video>` element fetches its bytes
//! from here, never from googlevideo (context/11: no YouTube shapes past the command boundary).
//!
//! **Why a socket and not a `limusicvideo://` custom scheme.** The scheme was the plan, and it does
//! work for `fetch`, XHR and an iframe: measured on WebKitGTK, a registered scheme answers a
//! textbook `206` with `Content-Range: bytes 0-1445/35729196` and `Content-Type: video/webm`. A
//! `<video>` still refuses it with `MEDIA_ERR_SRC_NOT_SUPPORTED` and never asks again, because the
//! element hands the URI to GStreamer, whose source element only claims `http`, `https` and `blob`.
//! The same bytes over `https` played at 1280x720. So the boundary is kept with a loopback socket
//! instead: `http://127.0.0.1:<ephemeral>/<token>/<videoId>`, bound to localhost, with a random
//! per-launch token in the path so nothing else on the machine can guess a URL.
//!
//! `video_stream` has already put the real googlevideo URL in [`AppState`] under that videoId; the
//! bytes are fetched by the audio proxy's chunked pump ([`crate::audioproxy::serve_url`]), so the
//! element's open-ended ranges never reach googlevideo in the shape it throttles.

use std::convert::Infallible;
use std::io;
use std::sync::{Arc, OnceLock};

use http_body_util::{combinators::BoxBody, BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper::header;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};

use crate::loopback::{Health, Loopback};
use crate::state::AppState;

/// Streamed, never buffered: a 35 MB video passes through in chunks, so the proxy costs a few KB of
/// RAM no matter how long the track is.
type ProxyBody = BoxBody<Bytes, io::Error>;

/// The running server (its port can change: [`ensure_alive`]). Unset if the first bind failed,
/// which just means video mode never finds a URL and the view keeps the artwork.
static SERVER: OnceLock<Loopback> = OnceLock::new();

/// Bind the loopback listener and start serving. Binds synchronously so [`url_for`] is usable the
/// moment this returns, then hands the socket to tokio.
pub fn start(state: Arc<AppState>) {
    let accept: crate::loopback::Accept = Arc::new(move |stream| {
        let state = state.clone();
        crate::rt::spawn(async move {
            let svc = service_fn(move |req| serve(req, state.clone()));
            // No connection cap: the three timeouts here remove the unbounded cases, and a
            // semaphore is real book-keeping for a proxy that serves one <video> element.
            // A dropped connection is what a seek looks like from here, so errors are expected.
            let _ = http1::Builder::new()
                // hyper 1.x panics the moment it arms a timeout with no timer installed, so
                // without this every single connection to the proxy died on arrival and took a
                // tokio worker with it. That is why music videos never played.
                .timer(TokioTimer::new())
                .header_read_timeout(std::time::Duration::from_secs(15))
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    });
    match Loopback::start("video proxy", accept) {
        Ok(s) => {
            let _ = SERVER.set(s);
        }
        Err(e) => tracing::warn!(error = %e, "video proxy: bind failed (music videos disabled)"),
    }
}

/// Check the listener and rebuild it if the system took it away. `None`: it never came up.
pub fn ensure_alive() -> Option<Health> {
    SERVER.get().map(Loopback::ensure_alive)
}

/// Debug hook: close the listener as iOS would.
pub fn kill() {
    if let Some(s) = SERVER.get() {
        s.kill();
    }
}

/// The URL the UI puts in `<video src>`, or `None` if the server never came up.
pub fn url_for(video_id: &str) -> Option<String> {
    let server = SERVER.get()?;
    server.ensure_alive();
    Some(format!("http://127.0.0.1:{}/{}/{video_id}", server.port()?, server.token()))
}

/// `/<token>/<videoId>` to the videoId, once the token matches this launch's.
fn video_id_from<'a>(path: &'a str, token: &str) -> Option<&'a str> {
    let (t, id) = path.trim_start_matches('/').split_once('/')?;
    (t == token && !id.is_empty()).then_some(id)
}

fn empty(status: StatusCode) -> Response<ProxyBody> {
    let mut r = Response::new(Empty::<Bytes>::new().map_err(|e| match e {}).boxed());
    *r.status_mut() = status;
    r
}

async fn serve(
    req: Request<Incoming>,
    state: Arc<AppState>,
) -> Result<Response<ProxyBody>, Infallible> {
    Ok(handle(req, state).await.unwrap_or_else(empty))
}

async fn handle(
    req: Request<Incoming>,
    state: Arc<AppState>,
) -> Result<Response<ProxyBody>, StatusCode> {
    // The page's ambient light reads the picture into WebGL, which only works on a CORS load: the
    // element then asks with `crossorigin` and every answer has to allow it. `*` gives nothing away
    // the token doesn't already guard. A preflight is answered for engines that send one for Range.
    if req.method() == Method::OPTIONS {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "range")
            .body(Empty::<Bytes>::new().map_err(|e| match e {}).boxed())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let token = SERVER.get().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?.token();
    let video_id = video_id_from(req.uri().path(), token).ok_or(StatusCode::NOT_FOUND)?;
    // Nothing resolved this id: a stale element, or a `video_stream` that returned None.
    let upstream = state.video_url(video_id).ok_or(StatusCode::NOT_FOUND)?;

    // Through the audio proxy's chunked fetch: googlevideo throttles the open-ended range a
    // `<video>` element sends to about realtime, so forwarding it untouched (as this used to)
    // left the picture buffering behind the music. Its stall guard covers this path too.
    let mut resp = crate::audioproxy::serve_url(&upstream, &req).await?;
    resp.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, header::HeaderValue::from_static("*"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token is the only thing keeping another local process off the proxy, so a wrong or
    /// missing one has to miss.
    #[test]
    fn only_the_right_token_routes() {
        assert_eq!(video_id_from("/abc123/dQw4w9WgXcQ", "abc123"), Some("dQw4w9WgXcQ"));
        assert_eq!(video_id_from("/wrong/dQw4w9WgXcQ", "abc123"), None);
        assert_eq!(video_id_from("/dQw4w9WgXcQ", "abc123"), None);
        assert_eq!(video_id_from("/abc123/", "abc123"), None);
    }
}
