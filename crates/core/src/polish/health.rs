//! Playback health: does stream extraction still work?
//!
//! YouTube changes break extraction for everyone at once, and the only symptom inside the app is
//! every track being skipped. [`check`] resolves a known video through the normal client chain
//! (no cache, nothing played), then reads the first KiB of the stream to be sure googlevideo
//! actually serves it. The UI shows "Playback is broken right now, an update may fix it" on
//! `failed`; `offline` means there was no network to judge by.
//!
//! [`start`] runs it once in the background shortly after launch and emits `playback-health`
//! (the same [`Health`] payload) only when it failed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use crate::host::HostExt;

use crate::orchestrator::ResolveError;
use crate::state::AppState;

/// Long-lived official uploads, tried in order: a second one rules out a single video being
/// taken down or region-blocked.
const PROBES: [&str; 2] = ["dQw4w9WgXcQ", "kJQP7kiw5Fk"];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok` | `failed` | `offline`.
    pub status: &'static str,
    pub ok: bool,
    /// The client that produced the working stream (`WEB_REMIX`, `TVHTML5`, `rustypipe`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub video_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
    pub checked_at: i64,
}

/// Classify one probe's outcome.
pub fn verdict(resolved: &Result<String, ResolveError>, served: Option<bool>) -> &'static str {
    match resolved {
        Ok(_) if served != Some(false) => "ok",
        Ok(_) => "failed",
        Err(ResolveError::Unreachable(_)) => "offline",
        Err(_) => "failed",
    }
}

/// GET the first KiB of the stream. `None` when the request itself never got an answer.
async fn served(url: &str, headers: &std::collections::HashMap<String, String>) -> Option<bool> {
    let mut req = crate::http::client()
        .get(url)
        .header("Range", "bytes=0-1023")
        .header("Accept-Encoding", "identity")
        .timeout(Duration::from_secs(8));
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.ok()?;
    Some(resp.status().is_success())
}

async fn probe(st: &AppState, video_id: &str) -> Health {
    let started = Instant::now();
    let res = tokio::time::timeout(
        Duration::from_secs(25),
        st.orchestrator.resolve(video_id, false, st.quality(), &st.disabled_clients()),
    )
    .await
    .unwrap_or_else(|_| Err(ResolveError::Unreachable("timed out".into())));
    let (resolved, served_ok, client) = match res {
        Ok(d) => {
            // A live/HLS answer would be a playlist, not media; the probes are plain uploads.
            let ok = served(&d.stream_url, &d.headers).await;
            (Ok(d.stream_client.clone()), ok, Some(d.stream_client))
        }
        Err(e) => (Err(e), None, None),
    };
    let status = verdict(&resolved, served_ok);
    Health {
        status,
        ok: status == "ok",
        client,
        video_id: video_id.to_owned(),
        error: match (&resolved, served_ok) {
            (Err(e), _) => Some(e.to_string()),
            (Ok(_), Some(false)) => Some("the stream URL was refused".into()),
            _ => None,
        },
        elapsed_ms: started.elapsed().as_millis() as u64,
        checked_at: super::now_secs(),
    }
}

pub async fn check(st: &AppState) -> Health {
    let mut last = None;
    for id in PROBES {
        let h = probe(st, id).await;
        if h.status != "failed" {
            return h;
        }
        last = Some(h);
    }
    last.expect("at least one probe")
}

/// The startup check: once, in the background, after the cipher and PoToken have had time to
/// warm up. Emits `playback-health` only on failure.
pub fn start(st: Arc<AppState>) {
    crate::rt::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let h = check(&st).await;
        if h.status == "failed" {
            tracing::warn!(error = ?h.error, "playback health check failed");
            let _ = st.host.emit("playback-health", &h);
        } else {
            tracing::info!(status = h.status, client = ?h.client, ms = h.elapsed_ms, "playback health");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(verdict(&Ok("WEB_REMIX".into()), Some(true)), "ok");
        assert_eq!(
            verdict(&Ok("WEB_REMIX".into()), None),
            "ok",
            "probe GET got no answer: trust resolve"
        );
        assert_eq!(verdict(&Ok("WEB_REMIX".into()), Some(false)), "failed", "googlevideo said 403");
        assert_eq!(verdict(&Err(ResolveError::Unreachable("dns".into())), None), "offline");
    }
}
