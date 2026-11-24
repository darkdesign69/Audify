//! Offline downloads (stream: downloads). See `docs/streams/downloads.md` for the research and
//! the decision; in short:
//!
//! - The user toggles *collections* (a playlist, an album, Liked Songs, an episode). Their tracks
//!   are stored once each (refcounted in [`store`]) as native opus/m4a/mp3 files under the
//!   downloads folder, with the loudness value, artwork, and lyrics prefetched into `lyrics_cache`.
//! - A durable queue runs [`MAX_PARALLEL`] downloads at a time ([`fetch`]: the player's own
//!   extraction chain + bounded ranges + `.part` resume), retries with backoff, resumes after a
//!   restart, and waits while offline, in offline mode, or on a metered network ([`net`]).
//! - Downloaded playlists follow their source: Liked Songs and own playlists on every
//!   `library-changed`, YouTube playlists and albums at launch and every 6 h.
//! - Playback: `AppState::resolve` asks [`playback_data`] first, so a downloaded track plays its
//!   file with no network. In offline mode (or with no network) [`offline_blocks`] makes playback
//!   step over everything that isn't downloaded, and the UI greys it (`SkipReason::offline`).

pub mod fetch;
pub mod net;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use innertube::SongItem;
use serde::Serialize;
use crate::host::HostExt;

use crate::orchestrator::PlaybackData;
use crate::state::AppState;
use store::{Collection, Orphan};

/// Tracks downloading at once (InnerTune / RiMusic `maxParallelDownloads = 3`).
pub const MAX_PARALLEL: usize = 3;
/// YouTube playlists and albums are re-read this often for new and removed tracks.
const REMOTE_SYNC_EVERY: Duration = Duration::from_secs(6 * 3600);

const SET_OFFLINE: &str = "offline_mode";
const SET_METERED: &str = "download_on_metered";
const SET_DIR: &str = "downloads_dir";
pub const SET_QUALITY: &str = "download_quality";

#[derive(Debug, Clone)]
struct Entry {
    file: PathBuf,
    loudness_db: Option<f64>,
}

#[derive(Default)]
struct Running {
    cancel: Arc<AtomicBool>,
    done: u64,
    total: u64,
}

/// Whether the queue may run, from the network and the user's switches. Pure, for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetState {
    /// The OS says there is a path out.
    pub online: bool,
    /// Cellular / hotspot / Low Data Mode.
    pub metered: bool,
    /// A download just failed with "unreachable" and the probe hasn't seen YouTube since.
    pub unreachable: bool,
    /// Settings › Offline mode.
    pub offline_mode: bool,
    /// Settings › Download using metered connections.
    pub allow_metered: bool,
}

impl NetState {
    /// Only downloaded content plays: the user's switch, or no network at all.
    pub fn offline(&self) -> bool {
        self.offline_mode || !self.online
    }
    /// May the queue fetch right now?
    pub fn can_download(&self) -> bool {
        !self.offline() && !self.unreachable && (!self.metered || self.allow_metered)
    }
    /// How many new downloads to start.
    pub fn free_slots(&self, running: usize) -> usize {
        if self.can_download() {
            MAX_PARALLEL.saturating_sub(running)
        } else {
            0
        }
    }
}

/// Does offline playback have to step over this id? Local files and downloads play; nothing else.
pub fn blocked_offline(offline: bool, video_id: &str, downloaded: bool) -> bool {
    offline && !downloaded && crate::local::song_path(video_id).is_none()
}

pub struct Manager {
    state: Arc<AppState>,
    dir: RwLock<PathBuf>,
    index: RwLock<HashMap<String, Entry>>,
    running: Mutex<HashMap<String, Running>>,
    net: Mutex<NetState>,
    wake: tokio::sync::Notify,
    sync_lock: tokio::sync::Mutex<()>,
}

static MGR: OnceLock<Arc<Manager>> = OnceLock::new();

pub fn mgr() -> Option<&'static Arc<Manager>> {
    MGR.get()
}

// ── Hooks for the player (state.rs, polish::explicit) ──────────────────────────────────────────

/// The downloaded file for `video_id`, as a stream: no network, the stored loudness.
pub fn playback_data(video_id: &str) -> Option<PlaybackData> {
    let m = mgr()?;
    let entry = m.index.read().ok()?.get(video_id).cloned()?;
    if !entry.file.is_file() {
        tracing::warn!(video_id, path = %entry.file.display(), "downloads: file is gone, re-queueing");
        m.index.write().ok()?.remove(video_id);
        store::mark_missing(&m.state.db.conn(), video_id);
        m.emit_changed(vec![], vec![video_id.to_owned()]);
        m.wake.notify_one();
        return None;
    }
    let path = entry.file.to_string_lossy().into_owned();
    tracing::info!(video_id, path = %path, "downloads: playing the downloaded file (no stream resolve)");
    Some(PlaybackData {
        video_id: video_id.to_owned(),
        stream_url: path,
        itag: 0,
        headers: HashMap::new(),
        expires_in_seconds: i64::MAX / 2,
        loudness_db: entry.loudness_db,
        playback_ping: None,
        title: None,
        artists: None,
        duration: None,
        thumbnail: None,
        is_video: None,
        is_episode: None,
        live: false,
        stream_client: "download".to_owned(),
    })
}

pub fn is_downloaded(video_id: &str) -> bool {
    mgr().and_then(|m| m.index.read().ok().map(|i| i.contains_key(video_id))).unwrap_or(false)
}

// ── stream: wave2 (File › Offline Mode in the menu bar) ──
/// The user's Offline Mode setting (not the network's state).
pub fn offline_mode() -> bool {
    mgr().is_some_and(|m| m.net().offline_mode)
}

/// Offline Mode, or no network at all (stream: video-pods: no podcast picture then).
pub fn offline() -> bool {
    mgr().is_some_and(|m| m.net().offline())
}

/// Turn Offline Mode on or off, as Settings › Storage does.
pub fn set_offline_mode(on: bool) {
    if let Some(m) = mgr() {
        m.set_offline_mode(on);
    }
}
// ── end stream: wave2 ──

/// Playback must skip `video_id`: offline and not downloaded (local files always play).
pub fn offline_blocks(video_id: &str) -> bool {
    let Some(m) = mgr() else { return false };
    let offline = m.net().offline();
    offline && blocked_offline(true, video_id, is_downloaded(video_id))
}

// ── Lifecycle ──────────────────────────────────────────────────────────────────────────────────

pub fn default_dir(host: &dyn crate::host::Host) -> PathBuf {
    host.data_dir().join("Downloads")
}

/// Start the manager: tables, index, network monitor, queue, sync. Once, after `AppState`.
pub fn init(state: Arc<AppState>) {
    let host = state.host.clone();
    let db = state.db.clone();
    {
        let conn = db.conn();
        store::migrate(&conn);
        store::reset_on_launch(&conn);
    }
    let dir = db.get_setting(SET_DIR).map(PathBuf::from).unwrap_or_else(|| default_dir(&*host));
    let _ = std::fs::create_dir_all(&dir);
    host.allow_asset_dir(&dir);
    let net = NetState {
        online: true,
        offline_mode: db.get_setting(SET_OFFLINE).as_deref() == Some("true"),
        allow_metered: db.get_setting(SET_METERED).as_deref() == Some("true"),
        ..Default::default()
    };
    let m = Arc::new(Manager {
        state,
        dir: RwLock::new(dir),
        index: RwLock::new(HashMap::new()),
        running: Mutex::new(HashMap::new()),
        net: Mutex::new(net),
        wake: tokio::sync::Notify::new(),
        sync_lock: tokio::sync::Mutex::new(()),
    });
    m.reload_index();
    if MGR.set(m.clone()).is_err() {
        return;
    }

    host.start_network_monitor(Box::new(|online, metered| {
        if let Some(m) = mgr() {
            m.set_net(|n| {
                n.online = online;
                n.metered = metered;
                if online {
                    n.unreachable = false;
                }
            });
        }
    }));

    let me = m.clone();
    crate::rt::spawn(async move { me.scheduler().await });
    let me = m.clone();
    crate::rt::spawn(async move { me.ticker().await });
    let me = m.clone();
    crate::rt::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            innertube::background(me.sync_all(true)).await; // stream: reliability
            tokio::time::sleep(REMOTE_SYNC_EVERY).await;
        }
    });
    // Liked Songs and own playlists follow every library edit.
    crate::host::listen("library-changed", move |payload| {
        let scope = serde_json::from_str::<serde_json::Value>(payload)
            .ok()
            .and_then(|v| v.get("scope").and_then(|s| s.as_str()).map(str::to_owned))
            .unwrap_or_default();
        if !matches!(scope.as_str(), "liked" | "playlists" | "all" | "") {
            return;
        }
        let me = m.clone();
        crate::rt::spawn(innertube::background(async move { me.sync_all(false).await })); // stream: reliability
    });
}

impl Manager {
    fn net(&self) -> NetState {
        *self.net.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_net(&self, f: impl FnOnce(&mut NetState)) {
        let (before, after) = {
            let mut n = self.net.lock().unwrap_or_else(|e| e.into_inner());
            let before = *n;
            f(&mut n);
            (before, *n)
        };
        if before == after {
            return;
        }
        tracing::info!(?after, "downloads: network state");
        if after.offline_mode != before.offline_mode {
            self.state.host.offline_mode_changed(after.offline_mode); // stream: wave2
        }
        if after.offline() != before.offline() {
            if !after.offline() {
                // Back online: give failed tracks another go.
                store::retry_failed(&self.state.db.conn());
            }
            // Home is built from downloads offline (`smart::home::offline_home`): rebuild it.
            crate::smart::home::emit_changed(&*self.state.host, "network", None);
        }
        let _ = self.state.host.emit("downloads:network", self.net_status());
        self.wake.notify_one();
    }

    fn dir(&self) -> PathBuf {
        self.dir.read().map(|d| d.clone()).unwrap_or_default()
    }

    fn reload_index(&self) {
        let dir = self.dir();
        let rows = store::done_index(&self.state.db.conn());
        let mut idx = HashMap::with_capacity(rows.len());
        for (id, file, loudness_db, _) in rows {
            idx.insert(id, Entry { file: dir.join(file), loudness_db });
        }
        if let Ok(mut i) = self.index.write() {
            *i = idx;
        }
    }

    fn net_status(&self) -> serde_json::Value {
        let n = self.net();
        serde_json::json!({
            "online": n.online && !n.unreachable,
            "metered": n.metered,
            "offline": n.offline(),
            "offlineMode": n.offline_mode,
            "allowMetered": n.allow_metered,
            "paused": !n.can_download(),
        })
    }

    /// Thumbnail URL → local artwork file, for the UI to show while offline.
    fn art_map(&self) -> HashMap<String, String> {
        let dir = self.dir();
        let conn = self.state.db.conn();
        let mut out = HashMap::new();
        if let Ok(mut st) = conn.prepare(
            "SELECT json_extract(song_json, '$.thumbnail'), art FROM dl_tracks
             WHERE state = 'done' AND art IS NOT NULL",
        ) {
            if let Ok(rows) = st.query_map([], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
            }) {
                for (url, art) in rows.flatten() {
                    if let Some(url) = url {
                        out.insert(url, dir.join(art).to_string_lossy().into_owned());
                    }
                }
            }
        }
        for c in store::collections(&conn) {
            if let Some(url) = c.thumbnail.filter(|u| u.starts_with("http")) {
                let p = dir.join(format!("art/c_{}.jpg", fetch::file_stem(&c.id)));
                if p.is_file() {
                    out.insert(url, p.to_string_lossy().into_owned());
                }
            }
        }
        out
    }

    fn summary(&self) -> serde_json::Value {
        let conn = self.state.db.conn();
        let (count, bytes, queued, failed) = store::totals(&conn);
        serde_json::json!({
            "collections": store::statuses(&conn),
            "count": count,
            "bytes": bytes,
            "queued": queued,
            "failed": failed,
        })
    }

    fn emit_changed(&self, added: Vec<String>, removed: Vec<String>) {
        let mut v = self.summary();
        v["added"] = serde_json::json!(added);
        v["removed"] = serde_json::json!(removed);
        let _ = self.state.host.emit("downloads:changed", v);
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let mut v = self.summary();
        let mut done: Vec<String> =
            self.index.read().map(|i| i.keys().cloned().collect()).unwrap_or_default();
        done.sort();
        v["done"] = serde_json::json!(done);
        v["active"] = serde_json::json!(self.progress());
        v["network"] = self.net_status();
        v["dir"] = serde_json::json!(self.dir().to_string_lossy());
        v["art"] = serde_json::json!(self.art_map());
        v["quality"] = serde_json::json!(self
            .state
            .db
            .get_setting(SET_QUALITY)
            .unwrap_or_else(|| "HIGH".into()));
        v
    }

    fn progress(&self) -> Vec<serde_json::Value> {
        let r = self.running.lock().unwrap_or_else(|e| e.into_inner());
        r.iter()
            .map(|(id, p)| serde_json::json!({ "videoId": id, "done": p.done, "total": p.total }))
            .collect()
    }

    /// Progress for the rings, 3× a second while anything runs.
    async fn ticker(self: Arc<Self>) {
        let mut was_running = false;
        loop {
            tokio::time::sleep(Duration::from_millis(330)).await;
            let p = self.progress();
            if p.is_empty() && !was_running {
                continue;
            }
            was_running = !p.is_empty();
            let _ = self.state.host.emit("downloads:progress", p);
        }
    }

    // ── Queue ──────────────────────────────────────────────────────────────────────────────

    async fn scheduler(self: Arc<Self>) {
        loop {
            let net = self.net();
            let running: HashSet<String> = {
                let r = self.running.lock().unwrap_or_else(|e| e.into_inner());
                // Paused (offline, metered): stop what runs; the `.part` files resume later.
                if !net.can_download() {
                    for p in r.values() {
                        p.cancel.store(true, Ordering::SeqCst);
                    }
                }
                r.keys().cloned().collect()
            };
            let slots = net.free_slots(running.len());
            let now = crate::db::now_secs();
            let jobs = store::next_jobs(&self.state.db.conn(), now, slots, &running);
            for job in jobs {
                let cancel = Arc::new(AtomicBool::new(false));
                store::set_state(&self.state.db.conn(), &job.video_id, "downloading");
                self.running
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(job.video_id.clone(), Running { cancel: cancel.clone(), ..Default::default() });
                let me = self.clone();
                // ── stream: reliability ── a queued download is resolved at background priority:
                // whatever the user is playing or opening goes first.
                crate::rt::spawn(innertube::background(async move {
                    me.run(job.song, job.attempts, cancel).await
                }));
            }
            // Sleep until something changes, or a backed-off track comes due (at most 30 s).
            let wait = store::next_due(&self.state.db.conn())
                .map(|due| (due - crate::db::now_secs()).clamp(1, 30) as u64)
                .unwrap_or(30);
            let _ = tokio::time::timeout(Duration::from_secs(wait), self.wake.notified()).await;
        }
    }

    async fn run(self: Arc<Self>, song: SongItem, attempts: i64, cancel: Arc<AtomicBool>) {
        let id = song.video_id.clone();
        let dir = self.dir();
        let me = self.clone();
        let pid = id.clone();
        let progress = move |done: u64, total: u64| {
            if let Some(p) = me.running.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&pid) {
                p.done = done;
                p.total = total;
            }
        };
        let me = self.clone();
        let c2 = cancel.clone();
        let stop = move || c2.load(Ordering::SeqCst) || !me.net().can_download();
        tracing::info!(video_id = %id, attempts, "downloads: start");
        let result = fetch::download(&self.state, &dir, &song, &progress, &stop).await;
        self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        let now = crate::db::now_secs();
        match result {
            Ok(fin) => {
                let still_wanted = store::mark_done(&self.state.db.conn(), &id, &fin, now).unwrap_or(false);
                if !still_wanted {
                    // Removed while it downloaded.
                    remove_files(&dir, Some(&fin.file), fin.art.as_deref());
                } else {
                    tracing::info!(video_id = %id, file = %fin.file, bytes = fin.bytes, "downloads: done");
                    if let Ok(mut i) = self.index.write() {
                        i.insert(id.clone(), Entry { file: dir.join(&fin.file), loudness_db: fin.loudness_db });
                    }
                    self.emit_changed(vec![id.clone()], vec![]);
                    let me = self.clone();
                    crate::rt::spawn(async move { me.prefetch_lyrics(song).await });
                }
            }
            Err(fetch::JobError::Stopped) => {
                if cancel.load(Ordering::SeqCst) && !store::has_track(&self.state.db.conn(), &id) {
                    let _ = std::fs::remove_file(dir.join("tracks").join(format!("{}.part", fetch::file_stem(&id))));
                } else {
                    store::requeue(&self.state.db.conn(), &id);
                }
            }
            Err(fetch::JobError::Network(e)) => {
                tracing::info!(video_id = %id, error = %e, "downloads: network down, queue waits");
                store::requeue(&self.state.db.conn(), &id);
                self.set_net(|n| n.unreachable = true);
                let me = self.clone();
                crate::rt::spawn(async move { me.probe_until_back().await });
            }
            Err(fetch::JobError::Failed(e)) => {
                let gave_up = store::mark_retry(&self.state.db.conn(), &id, &e, now);
                tracing::warn!(video_id = %id, error = %e, gave_up, "downloads: failed");
                if gave_up {
                    self.emit_changed(vec![], vec![]);
                }
            }
        }
        self.wake.notify_one();
    }

    async fn probe_until_back(self: Arc<Self>) {
        static PROBING: AtomicBool = AtomicBool::new(false);
        if PROBING.swap(true, Ordering::SeqCst) {
            return;
        }
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            if !self.net().unreachable {
                break;
            }
            if crate::http::network_reachable().await {
                self.set_net(|n| n.unreachable = false);
                break;
            }
        }
        PROBING.store(false, Ordering::SeqCst);
    }

    async fn prefetch_lyrics(&self, song: SongItem) {
        if song.is_episode || crate::podcasts::is_rss_id(&song.video_id) {
            return;
        }
        let req = crate::lyrics::LyricsRequest {
            video_id: song.video_id.clone(),
            title: song.title.clone(),
            artists: song.artists.clone(),
            album: song.album.clone(),
            duration: crate::library::duration_secs(song.duration.as_deref()).map(|s| s as f64),
            searched: false,
        };
        let _ = crate::lyrics::get_lyrics(&self.state, req, None).await;
    }

    // ── Collections ────────────────────────────────────────────────────────────────────────

    /// Download a collection. `songs` is the track list for fixed items (an episode); lists that
    /// change (playlists, Liked Songs, albums) are read here, so sync reads them the same way.
    pub async fn add(self: &Arc<Self>, c: Collection, songs: Option<Vec<SongItem>>) -> Result<(), String> {
        let now = crate::db::now_secs();
        store::upsert_collection(&self.state.db.conn(), &c, now).map_err(|e| e.to_string())?;
        let list = match songs.filter(|s| !s.is_empty()) {
            Some(s) if !syncs(&c) => s,
            given => match self.fetch_list(&c).await {
                Ok(l) => l,
                Err(e) => match given {
                    Some(s) => s,
                    None => {
                        store::remove_collection(&self.state.db.conn(), &c.id).ok();
                        return Err(e);
                    }
                },
            },
        };
        self.apply(&c.id, &list, now);
        if c.kind == "episode" {
            let ep = episode_of(&c, list.first());
            if let Err(e) = crate::podcasts::store::save_downloaded(&self.state.db.podcasts_conn(), &ep) {
                tracing::warn!(episode = %c.id, error = %e, "downloads: could not save to Your Episodes");
            }
        }
        if let Some(t) = c.thumbnail.clone() {
            let dir = self.dir();
            let stem = format!("c_{}", fetch::file_stem(&c.id));
            crate::rt::spawn(async move {
                fetch::save_art(&dir, &stem, &t).await;
            });
        }
        Ok(())
    }

    fn apply(&self, id: &str, list: &[SongItem], now: i64) {
        let res = store::set_members(&self.state.db.conn(), id, list, now);
        match res {
            Ok((queued, orphans)) => {
                let removed = self.drop_orphans(orphans);
                if !queued.is_empty() || !removed.is_empty() {
                    tracing::info!(collection = id, queued = queued.len(), removed = removed.len(), "downloads: synced");
                }
                self.emit_changed(vec![], removed);
                self.wake.notify_one();
            }
            Err(e) => tracing::warn!(collection = id, error = %e, "downloads: could not store the list"),
        }
    }

    pub fn remove(&self, id: &str) {
        let orphans = store::remove_collection(&self.state.db.conn(), id).unwrap_or_default();
        let _ = std::fs::remove_file(self.dir().join(format!("art/c_{}.jpg", fetch::file_stem(id))));
        let removed = self.drop_orphans(orphans);
        self.emit_changed(vec![], removed);
    }

    pub fn remove_all(&self) {
        let orphans = store::remove_all(&self.state.db.conn()).unwrap_or_default();
        let removed = self.drop_orphans(orphans);
        // Whatever is left (collection art, stray parts) goes too.
        let dir = self.dir();
        let _ = std::fs::remove_dir_all(dir.join("art"));
        let _ = std::fs::remove_dir_all(dir.join("tracks"));
        self.emit_changed(vec![], removed);
    }

    pub fn retry(&self) {
        store::retry_failed(&self.state.db.conn());
        self.set_net(|n| n.unreachable = false);
        self.emit_changed(vec![], vec![]);
        self.wake.notify_one();
    }

    /// Delete orphans' files, stop their downloads. Returns the ids that were downloaded.
    fn drop_orphans(&self, orphans: Vec<Orphan>) -> Vec<String> {
        let dir = self.dir();
        let mut removed = Vec::new();
        for o in orphans {
            if let Some(p) = self.running.lock().unwrap_or_else(|e| e.into_inner()).get(&o.video_id) {
                p.cancel.store(true, Ordering::SeqCst);
            }
            remove_files(&dir, o.file.as_deref(), o.art.as_deref());
            let _ = std::fs::remove_file(dir.join("tracks").join(format!("{}.part", fetch::file_stem(&o.video_id))));
            if let Ok(mut i) = self.index.write() {
                i.remove(&o.video_id);
            }
            if o.was_done {
                removed.push(o.video_id);
            }
        }
        removed
    }

    /// Re-read every collection that follows a source. `remote` also re-reads YouTube.
    pub async fn sync_all(self: &Arc<Self>, remote: bool) {
        let _g = self.sync_lock.lock().await;
        let cols = store::collections(&self.state.db.conn());
        for c in cols {
            if !syncs(&c) || (is_remote(&c) && !remote) {
                continue;
            }
            if is_remote(&c) && self.net().offline() {
                continue;
            }
            // A local playlist that was deleted takes its download with it.
            if ids::is_local_playlist(&c.id)
                && crate::library::playlist(&self.state.db.conn(), &c.id).is_none()
            {
                self.remove(&c.id);
                continue;
            }
            match self.fetch_list(&c).await {
                Ok(list) => self.apply(&c.id, &list, crate::db::now_secs()),
                Err(e) => tracing::info!(collection = %c.id, error = %e, "downloads: sync skipped"),
            }
        }
    }

    async fn fetch_list(&self, c: &Collection) -> Result<Vec<SongItem>, String> {
        let st = &self.state;
        if c.id == crate::library::LIKED_ID || c.kind == "liked" {
            return Ok(crate::library::liked_songs(&st.db.conn()).into_iter().map(|e| e.song).collect());
        }
        if ids::is_local_playlist(&c.id) {
            return crate::library::entries(&st.db.conn(), &c.id)
                .map(|v| v.into_iter().map(|e| e.song).collect())
                .map_err(|e| e.to_string());
        }
        let client = st.clients.get(innertube::METADATA_CLIENT).ok_or("metadata client missing")?;
        if c.kind == "album" {
            let page = st.it.album(client, &c.id).await.map_err(|e| e.to_string())?;
            let video = crate::state::wants_video(&st.db);
            let mut items = page.items;
            for i in &mut items {
                crate::state::prefer_version(i, video);
            }
            return Ok(items);
        }
        if c.kind == "playlist" {
            let page = st.it.playlist(client, &c.id, None).await.map_err(|e| e.to_string())?;
            let mut items = page.items;
            let mut token = page.continuation;
            while let Some(t) = token.take() {
                if items.len() >= 10_000 {
                    break;
                }
                let more = st.it.playlist_continuation(client, &t).await.map_err(|e| e.to_string())?;
                if more.items.is_empty() {
                    break;
                }
                items.extend(more.items);
                token = more.continuation;
            }
            return Ok(items);
        }
        // Fixed items (an episode, a song): what was stored.
        let conn = st.db.conn();
        Ok(store::members(&conn, &c.id).iter().filter_map(|id| store::song(&conn, id)).collect())
    }

    // ── Settings ───────────────────────────────────────────────────────────────────────────

    pub fn set_offline_mode(&self, on: bool) {
        self.state.db.set_setting(SET_OFFLINE, if on { "true" } else { "false" });
        self.set_net(|n| n.offline_mode = on);
    }

    pub fn set_allow_metered(&self, on: bool) {
        self.state.db.set_setting(SET_METERED, if on { "true" } else { "false" });
        self.set_net(|n| n.allow_metered = on);
    }

    /// Move every download to `to` and keep going from there.
    pub fn move_to(&self, to: PathBuf) -> Result<(), String> {
        let from = self.dir();
        if to == from {
            return Ok(());
        }
        std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
        // Stop running jobs; they resume (from scratch: their .part stays behind) afterwards.
        for p in self.running.lock().unwrap_or_else(|e| e.into_inner()).values() {
            p.cancel.store(true, Ordering::SeqCst);
        }
        for sub in ["tracks", "art"] {
            let src = from.join(sub);
            if src.is_dir() {
                move_dir(&src, &to.join(sub)).map_err(|e| e.to_string())?;
            }
        }
        if let Ok(mut d) = self.dir.write() {
            *d = to.clone();
        }
        self.state.db.set_setting(SET_DIR, &to.to_string_lossy());
        self.state.host.allow_asset_dir(&to);
        self.reload_index();
        self.emit_changed(vec![], vec![]);
        self.wake.notify_one();
        Ok(())
    }
}

/// The "Your Episodes" row for a downloaded episode the podcasts store hasn't seen (a search
/// result, Now Playing): what the download request carries.
fn episode_of(c: &Collection, song: Option<&SongItem>) -> crate::podcasts::Episode {
    let rss = crate::podcasts::rss_url(&c.id);
    crate::podcasts::Episode {
        id: c.id.clone(),
        show_title: c.subtitle.clone().or_else(|| song.map(|s| s.artists.clone())).unwrap_or_default(),
        title: song.map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| c.title.clone()),
        duration_secs: crate::library::duration_secs(song.and_then(|s| s.duration.as_deref())),
        thumbnail: c.thumbnail.clone().or_else(|| song.and_then(|s| s.thumbnail.clone())),
        video_id: if rss.is_none() { Some(c.id.clone()) } else { None },
        enclosure_url: rss.map(str::to_owned),
        ..Default::default()
    }
}

/// Lists that change after they're downloaded, and so are re-read.
fn syncs(c: &Collection) -> bool {
    matches!(c.kind.as_str(), "playlist" | "album" | "liked")
}

/// Lists that come from YouTube (re-read on the timer, not on library edits).
fn is_remote(c: &Collection) -> bool {
    !(c.kind == "liked" || c.id == crate::library::LIKED_ID || ids::is_local_playlist(&c.id))
}

fn remove_files(dir: &Path, file: Option<&str>, art: Option<&str>) {
    for rel in [file, art].into_iter().flatten() {
        let _ = std::fs::remove_file(dir.join(rel));
    }
}

/// Rename, or copy + delete across volumes.
fn move_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        std::fs::copy(e.path(), &to)?;
    }
    std::fs::remove_dir_all(src)
}

/// Collection ids the library uses (kept here so this module doesn't reach into commands.rs).
mod ids {
    pub fn is_local_playlist(id: &str) -> bool {
        id.starts_with("LOCALPLAYLIST:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_runs_only_online_unmetered_and_not_in_offline_mode() {
        let mut n = NetState { online: true, ..Default::default() };
        assert_eq!(n.free_slots(0), 3);
        assert_eq!(n.free_slots(2), 1);
        assert_eq!(n.free_slots(5), 0);
        n.metered = true;
        assert_eq!(n.free_slots(0), 0, "metered pauses");
        n.allow_metered = true;
        assert_eq!(n.free_slots(0), 3, "unless allowed");
        n.offline_mode = true;
        assert!(n.offline() && !n.can_download());
        n.offline_mode = false;
        n.unreachable = true;
        assert!(!n.offline(), "a failed probe pauses downloads but doesn't grey the library");
        assert!(!n.can_download());
        n.unreachable = false;
        n.online = false;
        assert!(n.offline(), "no network = offline");
    }

    #[test]
    fn offline_blocks_everything_but_downloads_and_local_files() {
        assert!(!blocked_offline(false, "abc", false));
        assert!(blocked_offline(true, "abc", false));
        assert!(!blocked_offline(true, "abc", true));
        assert!(!blocked_offline(true, "LOCAL:/Users/me/a.mp3", false));
    }

    #[test]
    fn a_downloaded_episode_becomes_its_your_episodes_row() {
        let c = Collection {
            id: "PODURL:https://x/1.mp3".into(),
            kind: "episode".into(),
            title: "Ep 1".into(),
            subtitle: Some("The Show".into()),
            thumbnail: None,
        };
        let song = SongItem {
            video_id: c.id.clone(),
            title: "Ep 1".into(),
            artists: "The Show".into(),
            duration: Some("1:02:05".into()),
            thumbnail: Some("https://x/a.jpg".into()),
            is_episode: true,
            ..Default::default()
        };
        let e = episode_of(&c, Some(&song));
        assert_eq!((e.title.as_str(), e.show_title.as_str()), ("Ep 1", "The Show"));
        assert_eq!(e.duration_secs, Some(3725));
        assert_eq!(e.enclosure_url.as_deref(), Some("https://x/1.mp3"));
        assert_eq!(e.video_id, None);
        assert_eq!(e.thumbnail.as_deref(), Some("https://x/a.jpg"));
        let yt = episode_of(&Collection { id: "dQw4w9WgXcQ".into(), ..c }, None);
        assert_eq!(yt.video_id.as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(yt.title, "Ep 1");
    }

    #[test]
    fn which_collections_sync_and_where_from() {
        let c = |id: &str, kind: &str| Collection {
            id: id.into(),
            kind: kind.into(),
            title: String::new(),
            subtitle: None,
            thumbnail: None,
        };
        assert!(syncs(&c("VLLM", "liked")) && !is_remote(&c("VLLM", "liked")));
        assert!(syncs(&c("LOCALPLAYLIST:3", "playlist")) && !is_remote(&c("LOCALPLAYLIST:3", "playlist")));
        assert!(syncs(&c("VLPL1", "playlist")) && is_remote(&c("VLPL1", "playlist")));
        assert!(syncs(&c("MPREb_x", "album")));
        assert!(!syncs(&c("abc", "episode")));
    }

    #[test]
    fn moving_a_folder_keeps_its_files() {
        let base = std::env::temp_dir().join(format!("encore-dl-move-{}", std::process::id()));
        let src = base.join("a/tracks");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("x.webm"), b"hi").unwrap();
        move_dir(&src, &base.join("b/tracks")).unwrap();
        assert_eq!(std::fs::read(base.join("b/tracks/x.webm")).unwrap(), b"hi");
        assert!(!src.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
