//! Fetch one track to disk.
//!
//! 1. Resolve with the player's own chain (`Orchestrator::resolve`), reusing a still-valid cached
//!    stream URL first. RSS episodes download their enclosure.
//! 2. Pull bounded `Range` chunks (the `audioproxy` regime: googlevideo serves `bytes=X-Y` at full
//!    speed and throttles open-ended reads to ~2x realtime; yt-dlp's `http_chunk_size` does the
//!    same) into `tracks/<name>.part`, resuming from its length after a crash or restart.
//! 3. A 403/410 mid-way is an expired URL: evict it, re-resolve, carry on from the same byte.
//! 4. Check the length against `Content-Range`, then rename into place. The container is kept
//!    as served (opus/webm or m4a; mp3 for most RSS): mpv plays it, and EQ and loudness gain are
//!    mpv filters, so nothing is transcoded.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use futures_util::StreamExt;
use innertube::{AudioQuality, SongItem};

use super::store::Finished;
use crate::state::AppState;

/// Bounded request size, same as the audio proxy's.
const CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum JobError {
    /// No network (or YouTube unreachable): don't count it, wait for the network.
    Network(String),
    /// Stopped on purpose: removed, paused, metered, offline mode.
    Stopped,
    /// This track failed; retry later with backoff.
    Failed(String),
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Network(e) => write!(f, "network: {e}"),
            JobError::Stopped => write!(f, "stopped"),
            JobError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// A file-system-safe stem for an id. YouTube ids already are; an RSS id (a URL) is hashed.
pub fn file_stem(video_id: &str) -> String {
    let ok = !video_id.is_empty()
        && video_id.len() <= 64
        && video_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        return video_id.to_owned();
    }
    use sha1::Digest;
    let h = sha1::Sha1::digest(video_id.as_bytes());
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// The extension for what the server sent: content type first, then the itag, then the URL.
pub fn extension(content_type: Option<&str>, itag: i64, url: &str) -> &'static str {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.contains("webm") || ct.contains("opus") {
        return "webm";
    }
    if ct.contains("mp4") || ct.contains("m4a") || ct.contains("aac") {
        return "m4a";
    }
    if ct.contains("mpeg") || ct.contains("mp3") {
        return "mp3";
    }
    if ct.contains("ogg") {
        return "ogg";
    }
    match itag {
        249 | 250 | 251 | 774 => return "webm",
        139 | 140 | 141 => return "m4a",
        _ => {}
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    for ext in ["mp3", "m4a", "aac", "ogg", "opus", "webm"] {
        if path.to_ascii_lowercase().ends_with(&format!(".{ext}")) {
            return match ext {
                "aac" => "m4a",
                "opus" => "ogg",
                e => e,
            };
        }
    }
    "audio"
}

/// The total from `Content-Range: bytes 0-0/12345`.
pub fn total_from_content_range(v: &str) -> Option<u64> {
    v.rsplit_once('/')?.1.trim().parse().ok()
}

/// A bigger version of a Google thumbnail for the cached artwork.
pub fn art_url(url: &str) -> String {
    let re_wh = regex::Regex::new(r"=w\d+-h\d+").unwrap();
    if re_wh.is_match(url) {
        return re_wh.replace(url, "=w544-h544").into_owned();
    }
    let re_s = regex::Regex::new(r"=s\d+$").unwrap();
    if re_s.is_match(url) {
        return re_s.replace(url, "=s544").into_owned();
    }
    url.to_owned()
}

#[derive(Debug)]
pub struct Source {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub itag: i64,
    pub loudness_db: Option<f64>,
}

fn quality(state: &AppState) -> AudioQuality {
    match state.db.get_setting("download_quality").as_deref() {
        Some("LOW") => AudioQuality::Low,
        _ => AudioQuality::High,
    }
}

async fn resolve(state: &AppState, video_id: &str, fresh: bool) -> Result<Source, JobError> {
    if let Some(url) = crate::podcasts::rss_url(video_id) {
        return Ok(Source { url: url.to_owned(), headers: HashMap::new(), itag: 0, loudness_db: None });
    }
    let now = crate::db::now_secs();
    if !fresh {
        // A URL the player resolved minutes ago is as good as a new one, at High only (the
        // cache holds whatever the streaming quality was).
        if quality(state) == AudioQuality::High && state.quality() != AudioQuality::Low {
            if let Some(c) = state.db.get_stream(video_id, now + 1800) {
                let headers = match c.client.as_deref() {
                    Some(k) => state.orchestrator.headers_for(k, false),
                    None => HashMap::new(),
                };
                return Ok(Source { url: c.url, headers, itag: c.itag, loudness_db: c.loudness_db });
            }
        }
    } else {
        state.db.evict_stream(video_id);
    }
    let data = state
        .orchestrator
        .resolve(video_id, false, quality(state), &state.disabled_clients())
        .await
        .map_err(|e| {
            if e.affects_every_track() {
                JobError::Network(e.to_string())
            } else {
                JobError::Failed(e.to_string())
            }
        })?;
    if data.live {
        return Err(JobError::Failed("live broadcasts can't be downloaded".into()));
    }
    Ok(Source {
        url: data.stream_url,
        headers: data.headers,
        itag: data.itag,
        loudness_db: data.loudness_db,
    })
}

/// Is a transport error the network being down, or just this request?
async fn classify(e: impl std::fmt::Display) -> JobError {
    if crate::http::network_reachable().await {
        JobError::Failed(e.to_string())
    } else {
        JobError::Network(e.to_string())
    }
}

/// Download `song` into `dir`. `progress(done, total)` is called as bytes land; `stop()` is polled
/// between reads and ends the job with [`JobError::Stopped`], keeping the `.part` for a resume.
pub async fn download(
    state: &Arc<AppState>,
    dir: &Path,
    song: &SongItem,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<Finished, JobError> {
    let id = song.video_id.as_str();
    let stem = file_stem(id);
    let src = resolve(state, id, false).await?;
    let can_refresh = !crate::podcasts::is_rss_id(id);
    let refresh = || async move {
        if can_refresh {
            Some(resolve(state, id, true).await)
        } else {
            None
        }
    };
    let (file, bytes, src) = fetch_to_file(dir, &stem, src, refresh, CHUNK, progress, stop).await?;
    let art = match song.thumbnail.as_deref() {
        Some(t) => save_art(dir, &stem, t).await,
        None => None,
    };
    Ok(Finished {
        file,
        art,
        bytes: bytes as i64,
        itag: Some(src.itag).filter(|i| *i > 0),
        loudness_db: src.loudness_db,
    })
}

/// The transfer itself, independent of how the URL was found: bounded `chunk`-sized ranges into
/// `tracks/<stem>.part` (resumed from its length), `refresh()` once on 403/410 (`None` = can't),
/// then a length check and the rename. Returns `(relative file, bytes, the source used)`.
pub async fn fetch_to_file<F, Fut>(
    dir: &Path,
    stem: &str,
    mut src: Source,
    refresh: F,
    chunk: u64,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    stop: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(String, u64, Source), JobError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<Result<Source, JobError>>>,
{
    let tracks = dir.join("tracks");
    std::fs::create_dir_all(&tracks).map_err(|e| JobError::Failed(e.to_string()))?;
    let part = tracks.join(format!("{stem}.part"));
    let mut refreshed = false;
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut total: Option<u64> = None;
    let mut content_type: Option<String> = None;
    let mut restarted = false;

    loop {
        if stop() {
            return Err(JobError::Stopped);
        }
        if let Some(t) = total {
            if have >= t {
                break;
            }
        }
        let end = match total {
            Some(t) => (have + chunk - 1).min(t - 1),
            None => have + chunk - 1,
        };
        let mut req = crate::http::client()
            .get(&src.url)
            .header("Range", format!("bytes={have}-{end}"))
            .header("Accept-Encoding", "identity");
        for (k, v) in &src.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = match crate::audioproxy::no_stall(req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(classify(e).await),
            Err(e) => return Err(classify(e).await),
        };
        let status = resp.status().as_u16();
        match status {
            206 => {
                if let Some(t) = resp
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    .and_then(total_from_content_range)
                {
                    total = Some(t);
                }
            }
            200 => {
                // The server ignored the range (some podcast hosts): take the whole body, from 0.
                if have > 0 {
                    have = 0;
                    let _ = std::fs::remove_file(&part);
                }
                total = resp.content_length();
            }
            416 => {
                // Asked past the end: the `.part` is complete if we knew the size, else start over.
                if total.is_some_and(|t| have >= t) {
                    break;
                }
                if restarted {
                    return Err(JobError::Failed("range not satisfiable".into()));
                }
                restarted = true;
                let _ = std::fs::remove_file(&part);
                have = 0;
                continue;
            }
            403 | 410 if !refreshed => {
                refreshed = true;
                match refresh().await {
                    Some(fresh) => {
                        tracing::info!(stem, status, "download: stream URL expired, re-resolved");
                        src = fresh?;
                        continue;
                    }
                    None => return Err(JobError::Failed(format!("HTTP {status}"))),
                }
            }
            s => return Err(JobError::Failed(format!("HTTP {s}"))),
        }
        if content_type.is_none() {
            content_type = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
        }
        let whole = status == 200;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part)
            .map_err(|e| JobError::Failed(e.to_string()))?;
        let mut body = resp.bytes_stream();
        loop {
            let next = match crate::audioproxy::no_stall(body.next()).await {
                Ok(n) => n,
                Err(e) => {
                    let _ = file.flush();
                    return Err(classify(e).await);
                }
            };
            match next {
                Some(Ok(bytes)) => {
                    file.write_all(&bytes).map_err(|e| JobError::Failed(format!("disk: {e}")))?;
                    have += bytes.len() as u64;
                    progress(have, total.unwrap_or(0));
                    if stop() {
                        let _ = file.flush();
                        return Err(JobError::Stopped);
                    }
                }
                Some(Err(e)) => {
                    let _ = file.flush();
                    return Err(classify(e).await);
                }
                None => break,
            }
        }
        file.flush().map_err(|e| JobError::Failed(e.to_string()))?;
        if whole {
            total = Some(have);
            break;
        }
    }

    let total = total.unwrap_or(have);
    if have != total || have == 0 {
        let _ = std::fs::remove_file(&part);
        return Err(JobError::Failed(format!("incomplete: {have} of {total} bytes")));
    }
    let ext = extension(content_type.as_deref(), src.itag, &src.url);
    let name = format!("{stem}.{ext}");
    std::fs::rename(&part, tracks.join(&name)).map_err(|e| JobError::Failed(e.to_string()))?;
    Ok((format!("tracks/{name}"), have, src))
}

/// Save artwork as `art/<stem>.jpg`; returns its relative path. Best effort.
pub async fn save_art(dir: &Path, stem: &str, url: &str) -> Option<String> {
    if !url.starts_with("http") {
        return None;
    }
    let art_dir: PathBuf = dir.join("art");
    std::fs::create_dir_all(&art_dir).ok()?;
    let rel = format!("art/{stem}.jpg");
    let path = dir.join(&rel);
    if path.is_file() {
        return Some(rel);
    }
    let resp = crate::audioproxy::no_stall(crate::http::client().get(art_url(url)).send())
        .await
        .ok()?
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = crate::audioproxy::no_stall(resp.bytes()).await.ok()?.ok()?;
    std::fs::write(&path, &bytes).ok()?;
    Some(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_safe() {
        assert_eq!(file_stem("dQw4w9WgXcQ"), "dQw4w9WgXcQ");
        let h = file_stem("PODURL:https://x.com/a.mp3?b=1");
        assert_eq!(h.len(), 40);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extensions_follow_type_then_itag_then_url() {
        assert_eq!(extension(Some("audio/webm"), 0, ""), "webm");
        assert_eq!(extension(Some("audio/mp4"), 0, ""), "m4a");
        assert_eq!(extension(Some("audio/mpeg"), 0, ""), "mp3");
        assert_eq!(extension(None, 251, ""), "webm");
        assert_eq!(extension(Some("application/octet-stream"), 140, ""), "m4a");
        assert_eq!(extension(None, 0, "https://cdn/x/ep.MP3?t=1"), "mp3");
        assert_eq!(extension(None, 0, "https://cdn/x"), "audio");
    }

    #[test]
    fn content_range_and_art_urls() {
        assert_eq!(total_from_content_range("bytes 0-0/3456"), Some(3456));
        assert_eq!(total_from_content_range("bytes 0-0/*"), None);
        assert_eq!(art_url("https://lh3.g/x=w60-h60-l90-rj"), "https://lh3.g/x=w544-h544-l90-rj");
        assert_eq!(art_url("https://yt3.g/x=s120"), "https://yt3.g/x=s544");
        assert_eq!(art_url("https://i.ytimg.com/vi/a/hq.jpg"), "https://i.ytimg.com/vi/a/hq.jpg");
    }

    /// Live: fetch a real track the way a download does (bounded ranges into a `.part`), stop it
    /// half way, resume from the `.part`, and check the result is the whole, playable file.
    /// Uses the rustypipe resolver, the only one that runs without the app's cipher webview.
    ///   LIBRARY_PATH=/opt/homebrew/lib cargo test -p limusic-app --lib downloads::fetch::tests::live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_download_resumes_and_completes() {
        const ID: &str = "jNQXAC9IVRw"; // "Me at the zoo", 19 s
        let resolve = || async {
            let c = innertube::rustypipe_fallback::resolve(ID, true).await.expect("resolve");
            Source { url: c.url, headers: HashMap::new(), itag: c.itag as i64, loudness_db: None }
        };
        let dir = std::env::temp_dir().join(format!("encore-dl-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seen = std::sync::atomic::AtomicU64::new(0);
        let progress = |done: u64, _t: u64| seen.store(done, std::sync::atomic::Ordering::SeqCst);
        // First pass: stop after the first 64 KiB chunk lands.
        let stop_early = || seen.load(std::sync::atomic::Ordering::SeqCst) >= 64 * 1024;
        let r = fetch_to_file(&dir, ID, resolve().await, || async { None }, 64 * 1024, &progress, &stop_early).await;
        assert!(matches!(r, Err(JobError::Stopped)), "{r:?}");
        let part = std::fs::metadata(dir.join(format!("tracks/{ID}.part"))).unwrap().len();
        assert!(part >= 64 * 1024, "part kept: {part}");
        // Second pass: resume (fresh URL, as after a restart) to the end.
        let never = || false;
        let (file, bytes, src) = fetch_to_file(&dir, ID, resolve().await, || async { None }, 256 * 1024, &progress, &never)
            .await
            .expect("resume");
        let data = std::fs::read(dir.join(&file)).unwrap();
        eprintln!("downloaded {file}: {bytes} bytes, itag {}", src.itag);
        assert_eq!(data.len() as u64, bytes);
        assert!(bytes > 100_000);
        let webm = data.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]);
        let mp4 = data.get(4..8) == Some(b"ftyp");
        assert!(webm || mp4, "a real audio container");
        assert!(!dir.join(format!("tracks/{ID}.part")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
