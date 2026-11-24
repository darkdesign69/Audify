//! Device sync: pair a Mac and an iPhone once, then every change reaches the other device a few
//! seconds later, directly over the local network or through an end-to-end encrypted mailbox on
//! opena.tech. No accounts. The full design is in `docs/SYNC.md`.
//!
//! - [`store`]: the change log (SQLite triggers), the merge and the initial snapshot.
//! - [`crypto`]: XChaCha20-Poly1305 boxes and key derivation.
//! - [`net`]: the LAN wire format, the LAN server, pairing (SPAKE2) and the sync exchange.
//! - [`relay`]: the opena.tech mailbox client.
//! - [`mdns`]: `_spotless-sync._tcp` advertising and browsing (not on iOS: Swift's Bonjour there).
//! - this file: the group state, the engine loop and the commands both bridges call.

pub mod crypto;
#[cfg(not(target_os = "ios"))]
mod mdns;
pub mod net;
pub mod relay;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};

use crate::db::{now_secs, Db};
use crate::host::{Host, HostExt};

pub use store::{Applied, Op};

/// Fields per LAN message.
pub const LAN_BATCH: usize = 5_000;
/// How long a pairing code stays valid, and how many tries it gets (each connection is one try).
pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);
pub const PAIRING_TRIES: u8 = 3;
/// The port the LAN server prefers; any free port when it is taken.
pub const LAN_PORT: u16 = 47615;
/// Debounce after a local change before pushing it.
const DEBOUNCE: Duration = Duration::from_millis(1000);
/// LAN exchange and relay poll while the app is in front; relay poll in the background.
const LAN_POLL: Duration = Duration::from_secs(3);
const RELAY_POLL_ACTIVE: Duration = Duration::from_secs(5);
const RELAY_POLL_IDLE: Duration = Duration::from_secs(60);
/// The keychain item the group key is sealed under.
const KEY_SECRET: &str = "sync_group_key";

/// The group this device belongs to (at most one).
#[derive(Clone, Debug)]
pub struct Group {
    pub pair_id: String,
    pub key: Vec<u8>,
    pub relay_sent: i64,
    pub relay_recv: i64,
    pub relay_full: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub platform: String,
    #[serde(skip)]
    pub addr: Option<String>,
    #[serde(skip)]
    pub port: Option<u16>,
    #[serde(skip)]
    pub sent: i64,
    #[serde(skip)]
    pub recv: i64,
    pub last_seen: i64,
}

pub fn load_group(conn: &Connection) -> Option<Group> {
    let row: Option<(String, String, i64, i64, i64)> = conn
        .query_row(
            "SELECT pair_id, key, relay_sent, relay_recv, relay_full FROM sync_group WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .ok()
        .flatten();
    let (pair_id, stored, relay_sent, relay_recv, full) = row?;
    let key = crate::secrets::unseal(KEY_SECRET, stored).and_then(|h| crypto::unhex(&h))?;
    Some(Group { pair_id, key, relay_sent, relay_recv, relay_full: full != 0 })
}

fn save_group(conn: &Connection, pair_id: &str, key: &[u8], relay_recv: i64) -> rusqlite::Result<()> {
    let stored = crate::secrets::seal(KEY_SECRET, &crypto::hex(key));
    conn.execute(
        "INSERT OR REPLACE INTO sync_group(id, pair_id, key, created_at, relay_sent, relay_recv, relay_full)
         VALUES(1, ?1, ?2, ?3, 0, ?4, 0)",
        params![pair_id, stored, now_secs(), relay_recv],
    )?;
    Ok(())
}

pub fn peers(conn: &Connection) -> Vec<Peer> {
    let Ok(mut st) = conn.prepare("SELECT dev, name, platform, addr, port, sent, recv, last_seen FROM sync_peers ORDER BY dev") else {
        return vec![];
    };
    let rows = st.query_map([], |r| {
        Ok(Peer {
            id: r.get(0)?,
            name: r.get(1)?,
            platform: r.get(2)?,
            addr: r.get(3)?,
            port: r.get::<_, Option<i64>>(4)?.map(|p| p as u16),
            sent: r.get(5)?,
            recv: r.get(6)?,
            last_seen: r.get(7)?,
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Add or refresh a peer. `None` fields keep what is stored.
pub fn upsert_peer(conn: &Connection, dev: &str, name: Option<&str>, platform: Option<&str>, addr: Option<&str>, port: Option<u16>) {
    let _ = conn.execute(
        "INSERT INTO sync_peers(dev, name, platform, addr, port, last_seen) VALUES(?1, COALESCE(?2, ''), COALESCE(?3, ''), ?4, ?5, ?6)
         ON CONFLICT(dev) DO UPDATE SET name = COALESCE(?2, name), platform = COALESCE(?3, platform),
             addr = COALESCE(?4, addr), port = COALESCE(?5, port), last_seen = ?6",
        params![dev, name, platform, addr, port.map(|p| p as i64), now_secs()],
    );
}

pub fn device_name() -> String {
    if cfg!(target_os = "ios") {
        return "iPhone".into();
    }
    let h = gethostname::gethostname().to_string_lossy().into_owned();
    let h = h.trim_end_matches(".local").replace('-', " ");
    if h.is_empty() { "Mac".into() } else { h }
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        std::env::consts::OS
    }
}

/// A pairing in progress on the device that shows the code.
#[derive(Clone, Debug)]
pub struct Pairing {
    pub code: String,
    pub expires: Instant,
    pub expires_at: i64,
    pub tries_left: u8,
    pub qr: String,
}

#[derive(Default)]
struct Runtime {
    pairing: Option<Pairing>,
    lan_port: Option<u16>,
    last_sync: Option<i64>,
    last_error: Option<String>,
    relay_ok: Option<bool>,
    lan_online: BTreeSet<String>,
    active: bool,
    syncing: bool,
    pushed_head: i64,
    changed_at: Option<Instant>,
    last_lan: Option<Instant>,
    last_relay: Option<Instant>,
}

type AppliedHook = Box<dyn Fn(&Applied) + Send + Sync>;

/// One device's sync engine. The app has one ([`engine`]); tests make several.
pub struct Engine {
    pub db: Arc<Db>,
    host: Option<Arc<dyn Host>>,
    pub mailbox: Option<relay::Mailbox>,
    rt: Mutex<Runtime>,
    kick: tokio::sync::Notify,
    on_applied: Mutex<Option<AppliedHook>>,
    server: Mutex<Option<tokio::task::JoinHandle<()>>>,
    #[cfg(not(target_os = "ios"))]
    mdns: Mutex<Option<mdns::Mdns>>,
}

impl Engine {
    pub fn new(db: Arc<Db>, host: Option<Arc<dyn Host>>, mailbox: Option<relay::Mailbox>) -> Arc<Self> {
        if let Err(e) = store::install(&db.conn()) {
            tracing::error!(error = %e, "sync: installing the change log failed");
        }
        Arc::new(Self {
            db,
            host,
            mailbox,
            rt: Mutex::new(Runtime { active: true, ..Default::default() }),
            kick: tokio::sync::Notify::new(),
            on_applied: Mutex::new(None),
            server: Mutex::new(None),
            #[cfg(not(target_os = "ios"))]
            mdns: Mutex::new(None),
        })
    }

    fn rt(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.rt.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_on_applied(&self, f: impl Fn(&Applied) + Send + Sync + 'static) {
        *self.on_applied.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    pub fn device_id(&self) -> String {
        store::device_id(&self.db.conn())
    }

    pub fn group(&self) -> Option<Group> {
        load_group(&self.db.conn())
    }

    pub fn peers(&self) -> Vec<Peer> {
        peers(&self.db.conn())
    }

    /// Merge remote ops, then tell the app what changed.
    pub fn apply(&self, ops: &[Op]) -> Applied {
        if ops.is_empty() {
            return Applied::default();
        }
        let applied = match store::apply(&self.db.conn(), ops) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(error = %e, "sync: applying remote changes failed");
                return Applied::default();
            }
        };
        if !applied.entities.is_empty() {
            if let Some(f) = self.on_applied.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                f(&applied);
            }
            self.emit_changed(Some(&applied));
        }
        applied
    }

    pub fn emit_changed(&self, applied: Option<&Applied>) {
        let Some(host) = &self.host else { return };
        let entities: Vec<&String> = applied.map(|a| a.entities.iter().collect()).unwrap_or_default();
        let _ = host.emit("sync-changed", json!({ "status": self.status(), "entities": entities }));
    }

    // ── status ───────────────────────────────────────────────────────────────────────────────

    pub fn status(&self) -> Value {
        let (group, peers, me, pending) = {
            let conn = self.db.conn();
            let group = load_group(&conn);
            let peers = peers(&conn);
            let me = store::device_id(&conn);
            let delivered = group.as_ref().map_or(0, |g| {
                let lan = peers.iter().map(|p| p.sent).min().unwrap_or(0);
                g.relay_sent.max(lan)
            });
            let pending: i64 = conn
                .query_row("SELECT COUNT(*) FROM sync_field WHERE seq > ?1 AND dev = ?2", params![delivered, me], |r| r.get(0))
                .unwrap_or(0);
            let pending = if group.is_some() { pending } else { 0 };
            (group, peers, me, pending)
        };
        let rt = self.rt();
        let pairing = rt.pairing.as_ref().filter(|p| p.expires > Instant::now()).map(|p| {
            json!({ "code": p.code, "qr": p.qr, "expiresAt": p.expires_at })
        });
        let peers: Vec<Value> = peers
            .iter()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap_or_default();
                v["lan"] = json!(rt.lan_online.contains(&p.id));
                v
            })
            .collect();
        json!({
            "paired": group.is_some(),
            "deviceId": me,
            "deviceName": device_name(),
            "platform": platform(),
            "pairId": group.as_ref().map(|g| g.pair_id.clone()),
            "peers": peers,
            "pending": pending,
            "lastSync": rt.last_sync,
            "lastError": rt.last_error,
            "relay": { "enabled": self.mailbox.is_some(), "ok": rt.relay_ok },
            "lanPort": rt.lan_port,
            "pairing": pairing,
            "syncing": rt.syncing,
            "active": rt.active,
        })
    }

    // ── pairing ──────────────────────────────────────────────────────────────────────────────

    /// Show a code: start (or restart) a pairing window. Makes the group if there is none yet.
    pub async fn start_pairing(self: &Arc<Self>) -> Result<Value, String> {
        let port = self.ensure_server().await?;
        let code = format!("{:06}", rand::random::<u32>() % 1_000_000);
        let dev = self.device_id();
        let addrs = net::local_addrs();
        let qr = net::qr_payload(&code, &addrs, port, &dev, &device_name());
        let ttl = PAIRING_TTL.as_secs() as i64;
        self.rt().pairing = Some(Pairing {
            code: code.clone(),
            expires: Instant::now() + PAIRING_TTL,
            expires_at: now_secs() + ttl,
            tries_left: PAIRING_TRIES,
            qr: qr.clone(),
        });
        self.refresh_mdns();
        self.emit_changed(None);
        Ok(json!({ "code": code, "qr": qr, "expiresAt": now_secs() + ttl, "port": port, "addrs": addrs }))
    }

    pub fn cancel_pairing(&self) {
        self.rt().pairing = None;
        self.refresh_mdns();
    }

    /// Called by the LAN server for each pairing attempt: consumes one try.
    pub(crate) fn take_try(&self) -> Result<String, String> {
        let mut rt = self.rt();
        let Some(p) = rt.pairing.as_mut() else { return Err("not_pairing".into()) };
        if p.expires <= Instant::now() || p.tries_left == 0 {
            rt.pairing = None;
            return Err("not_pairing".into());
        }
        p.tries_left -= 1;
        Ok(p.code.clone())
    }

    /// A wrong code was tried: the tries left (already counted by [`Self::take_try`]). At none
    /// the code is withdrawn, so the device showing it sees the pairing end.
    pub(crate) fn wrong_code(&self) -> u8 {
        let left = {
            let mut rt = self.rt();
            let left = rt.pairing.as_ref().map_or(0, |p| p.tries_left);
            if left == 0 {
                rt.pairing = None;
            }
            left
        };
        if left == 0 {
            self.refresh_mdns();
        }
        self.emit_changed(None);
        left
    }

    pub(crate) fn pairing_done(&self) {
        self.rt().pairing = None;
    }

    /// The host side of a successful pairing: make the group if needed and add the new peer.
    pub(crate) fn admit(&self, joiner: &net::Hello, addr: Option<String>) -> Result<net::Welcome, String> {
        let host = self.hello(); // before the lock: `hello` reads the device id itself
        let conn = self.db.conn();
        let group = match load_group(&conn) {
            Some(g) => g,
            None => {
                let pair_id = crypto::hex(&crypto::random_bytes::<16>());
                let key = crypto::random_bytes::<32>().to_vec();
                save_group(&conn, &pair_id, &key, 0).map_err(|e| e.to_string())?;
                store::enable(&conn, 1).map_err(|e| e.to_string())?;
                load_group(&conn).ok_or("group not saved")?
            }
        };
        if !store::capturing(&conn) {
            store::enable(&conn, 1).map_err(|e| e.to_string())?;
        }
        upsert_peer(&conn, &joiner.dev, Some(&joiner.name), Some(&joiner.platform), addr.as_deref(), joiner.port);
        // A re-paired device starts over: everything goes to it again.
        let _ = conn.execute("UPDATE sync_peers SET sent = 0, recv = 0 WHERE dev = ?1", [&joiner.dev]);
        let others: Vec<net::Hello> = peers(&conn)
            .into_iter()
            .filter(|p| p.id != joiner.dev)
            .map(|p| net::Hello { dev: p.id, name: p.name, platform: p.platform, port: p.port, addr: p.addr })
            .collect();
        Ok(net::Welcome {
            pair_id: group.pair_id,
            key: crypto::hex(&group.key),
            relay_recv: group.relay_recv,
            host,
            peers: others,
        })
    }

    pub(crate) fn hello(&self) -> net::Hello {
        net::Hello {
            dev: self.device_id(),
            name: device_name(),
            platform: platform().into(),
            port: self.rt().lan_port,
            addr: None,
        }
    }

    /// The joining side: run the PAKE against `addrs` (from the QR, Bonjour or mDNS) with `code`.
    pub async fn join(self: &Arc<Self>, code: &str, addrs: &[String]) -> Result<Value, String> {
        let code: String = code.chars().filter(char::is_ascii_digit).collect();
        if code.len() != 6 {
            return Err("bad_code".into());
        }
        let _ = self.ensure_server().await;
        let mut last = "no_host".to_string();
        for addr in addrs {
            match net::join(self, addr, &code).await {
                Ok(welcome) => {
                    self.joined(welcome, addr)?;
                    self.refresh_mdns();
                    self.sync_round(true).await;
                    self.emit_changed(None);
                    return Ok(self.status());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn joined(&self, w: net::Welcome, addr: &str) -> Result<(), String> {
        let key = crypto::unhex(&w.key).filter(|k| k.len() == 32).ok_or("bad_key")?;
        let conn = self.db.conn();
        // Leaving a previous group: its peers and log go; this device's data stays and merges.
        let _ = conn.execute("DELETE FROM sync_peers", []);
        save_group(&conn, &w.pair_id, &key, w.relay_recv).map_err(|e| e.to_string())?;
        store::enable(&conn, 0).map_err(|e| e.to_string())?;
        let (host, port) = net::split_addr(addr);
        upsert_peer(&conn, &w.host.dev, Some(&w.host.name), Some(&w.host.platform), Some(&host), w.host.port.or(port));
        for p in &w.peers {
            upsert_peer(&conn, &p.dev, Some(&p.name), Some(&p.platform), p.addr.as_deref(), p.port);
        }
        Ok(())
    }

    /// Leave the group on this device. Peers are told (LAN and relay); when the last other device
    /// is gone the mailbox is deleted. The synced data stays on both devices.
    pub async fn unpair(self: &Arc<Self>) -> Value {
        if let Some(g) = self.group() {
            let me = self.device_id();
            for p in self.peers() {
                if let Some(addr) = net::peer_addr(&p) {
                    let _ = net::exchange(self, &g, &p, &addr, true).await;
                }
            }
            if let Some(mb) = &self.mailbox {
                let alone = self.peers().is_empty();
                let _ = relay::post_msg(mb, &g, &me, &relay::Msg::leave()).await;
                if alone {
                    let _ = mb.delete(&g).await;
                }
            }
        }
        self.forget_group();
        self.emit_changed(None);
        self.status()
    }

    pub(crate) fn forget_group(&self) {
        let conn = self.db.conn();
        let _ = store::disable(&conn);
        let _ = conn.execute_batch("DELETE FROM sync_group; DELETE FROM sync_peers;");
        crate::secrets::forget(KEY_SECRET);
        drop(conn);
        self.rt().lan_online.clear();
        self.refresh_mdns();
    }

    /// A peer said it left. Alone now: this device leaves too.
    pub(crate) fn peer_left(&self, dev: &str) {
        let conn = self.db.conn();
        let _ = conn.execute("DELETE FROM sync_peers WHERE dev = ?1", [dev]);
        let alone = peers(&conn).is_empty();
        drop(conn);
        if alone {
            self.forget_group();
        }
        self.emit_changed(None);
    }

    // ── the loop ─────────────────────────────────────────────────────────────────────────────

    /// The app is in front (`true`) or in the background. Polls slow down in the background.
    pub fn set_active(&self, active: bool) {
        self.rt().active = active;
        if active {
            self.kick.notify_one();
        }
    }

    pub fn kick(&self) {
        self.kick.notify_one();
    }

    /// One full round: LAN with every reachable peer, then the relay.
    pub async fn sync_round(self: &Arc<Self>, force_relay: bool) {
        let Some(group) = self.group() else { return };
        self.rt().syncing = true;
        let mut errors = Vec::new();
        let mut online = BTreeSet::new();
        for p in self.peers() {
            let Some(addr) = net::peer_addr(&p) else { continue };
            match net::exchange(self, &group, &p, &addr, false).await {
                Ok(()) => {
                    online.insert(p.id.clone());
                }
                Err(e) => tracing::debug!(peer = %p.id, error = %e, "sync: LAN exchange failed"),
            }
        }
        let relay_due = {
            let rt = self.rt();
            let every = if rt.active { RELAY_POLL_ACTIVE } else { RELAY_POLL_IDLE };
            rt.last_relay.map_or(true, |t| t.elapsed() >= every) || rt.changed_at.is_some() || force_relay
        };
        let mut relay_ok = None;
        if let (Some(mb), true) = (&self.mailbox, relay_due) {
            match relay::round(self, mb).await {
                Ok(()) => relay_ok = Some(true),
                Err(e) => {
                    relay_ok = Some(false);
                    errors.push(e);
                }
            }
        }
        let head = store::head(&self.db.conn());
        let mut rt = self.rt();
        rt.syncing = false;
        rt.lan_online = online;
        rt.last_lan = Some(Instant::now());
        if relay_ok.is_some() {
            rt.last_relay = Some(Instant::now());
            rt.relay_ok = relay_ok;
        }
        rt.pushed_head = head;
        rt.changed_at = None;
        if errors.is_empty() {
            rt.last_sync = Some(now_secs());
            rt.last_error = None;
        } else {
            rt.last_error = errors.pop();
        }
    }

    /// The background loop: push a second after local changes settle, poll the LAN every few
    /// seconds and the relay every 5 s in front (60 s behind), and run at once on [`Self::kick`].
    pub async fn run(self: Arc<Self>) {
        let mut last_status = Value::Null;
        loop {
            let kicked = tokio::select! {
                _ = self.kick.notified() => true,
                _ = tokio::time::sleep(Duration::from_secs(1)) => false,
            };
            if self.group().is_none() {
                continue;
            }
            let head = store::head(&self.db.conn());
            let due = {
                let mut rt = self.rt();
                if head != rt.pushed_head && rt.changed_at.is_none() {
                    rt.changed_at = Some(Instant::now());
                }
                let settled = rt.changed_at.is_some_and(|t| t.elapsed() >= DEBOUNCE);
                let lan_due = rt.active && rt.last_lan.map_or(true, |t| t.elapsed() >= LAN_POLL);
                let every = if rt.active { RELAY_POLL_ACTIVE } else { RELAY_POLL_IDLE };
                let relay_due = rt.last_relay.map_or(true, |t| t.elapsed() >= every);
                kicked || settled || lan_due || relay_due
            };
            if due {
                self.sync_round(kicked).await;
                let s = self.status();
                if s != last_status {
                    last_status = s;
                    self.emit_changed(None);
                }
            }
        }
    }

    /// Start the LAN server (idempotent). Returns its port.
    pub async fn ensure_server(self: &Arc<Self>) -> Result<u16, String> {
        if let Some(p) = self.rt().lan_port {
            return Ok(p);
        }
        let listener = match tokio::net::TcpListener::bind(("0.0.0.0", LAN_PORT)).await {
            Ok(l) => l,
            Err(_) => tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.map_err(|e| e.to_string())?,
        };
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        self.rt().lan_port = Some(port);
        let me = self.clone();
        let task = tokio::spawn(async move { net::serve(me, listener).await });
        *self.server.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        self.refresh_mdns();
        Ok(port)
    }

    /// Bind loopback only (tests).
    #[cfg(test)]
    pub async fn ensure_server_loopback(self: &Arc<Self>) -> u16 {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        self.rt().lan_port = Some(port);
        let me = self.clone();
        tokio::spawn(async move { net::serve(me, listener).await });
        port
    }

    /// Advertise (paired or pairing) or stop advertising, and browse for the group's peers.
    fn refresh_mdns(&self) {
        #[cfg(not(target_os = "ios"))]
        {
            if self.host.is_none() {
                return; // tests: no multicast
            }
            let port = self.rt().lan_port;
            let pairing = self.rt().pairing.is_some();
            let tag = self.group().map(|g| crypto::lan_tag(&g.key));
            let mut slot = self.mdns.lock().unwrap_or_else(|e| e.into_inner());
            *slot = None; // unregisters the old one
            if let (Some(port), true) = (port, pairing || tag.is_some()) {
                *slot = mdns::Mdns::start(self.db.clone(), &self.device_id(), port, tag, pairing);
            }
        }
    }

    pub(crate) fn mark_changed(&self) {
        self.rt().changed_at.get_or_insert(Instant::now());
    }
}

// ── the app's engine ─────────────────────────────────────────────────────────────────────────

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// Start sync for the app: install the change log, start the loop, and, when paired, the LAN
/// server. Called from `boot::start_services` after every module made its tables.
pub fn start(state: &Arc<crate::state::AppState>) {
    let mailbox = relay::Mailbox::http(relay::default_base());
    let engine = Engine::new(state.db.clone(), Some(state.host.clone()), Some(mailbox));
    let st = Arc::downgrade(state);
    engine.set_on_applied(move |a| {
        if let Some(st) = st.upgrade() {
            remote_changed(&st, a);
        }
    });
    if ENGINE.set(engine.clone()).is_err() {
        return;
    }
    crate::rt::spawn(async move {
        if engine.group().is_some() {
            let _ = engine.ensure_server().await;
        }
        engine.run().await;
    });
}

pub fn engine() -> Option<&'static Arc<Engine>> {
    ENGINE.get()
}

fn need() -> Result<&'static Arc<Engine>, String> {
    engine().ok_or_else(|| "sync_not_started".to_string())
}

/// Bring the running app up to date with rows another device changed.
fn remote_changed(st: &Arc<crate::state::AppState>, a: &Applied) {
    let lib = [
        "liked_songs", "library_items", "podcast_follows", "lib_folders", "lib_folder_items", "lib_pins",
        "lib_played", "lib_touched", "hidden_songs", "local_playlists", "local_playlist_tracks",
    ];
    if a.entities.iter().any(|e| lib.contains(&e.as_str())) {
        let _ = st.host.emit("library-changed", json!({ "scope": "all" }));
    }
    if a.entities.contains(store::BLOCKED) {
        st.it.set_blocked(crate::blocked::block_list(&st.db));
    }
    if a.entities.iter().any(|e| e.contains("listen") || e == "smart_events") {
        let _ = st.host.emit("home-changed", json!({ "reason": "sync" }));
    }
    let st = st.clone();
    let settings = a.settings.clone();
    crate::rt::spawn(async move {
        for key in settings {
            if let Some(v) = st.db.get_setting(&key) {
                crate::api::setting_applied(&st, &key, &v, true).await;
            }
        }
    });
}

// ── commands (both bridges) ──────────────────────────────────────────────────────────────────

pub async fn sync_status() -> Result<Value, String> {
    Ok(need()?.status())
}

/// `{ code, qr, expiresAt, port, addrs }`: the code to show and the QR payload to draw.
pub async fn sync_start_pairing() -> Result<Value, String> {
    need()?.start_pairing().await
}

/// Join with the 6-digit `code`, at the address in `qr` (a `spotless://pair` link) or at
/// `host`/`port` (Bonjour on the phone). With neither, the Mac looks on the LAN by itself.
pub async fn sync_join(code: Option<String>, qr: Option<String>, host: Option<String>, port: Option<u16>) -> Result<Value, String> {
    let e = need()?;
    let parsed = qr.as_deref().and_then(net::parse_qr);
    let code = code.filter(|c| !c.trim().is_empty()).or_else(|| parsed.as_ref().map(|p| p.code.clone())).ok_or("bad_code")?;
    let mut addrs: Vec<String> = parsed.map(|p| p.addrs).unwrap_or_default();
    if let Some(h) = host.filter(|h| !h.is_empty()) {
        addrs.insert(0, net::join_addr(&h, port.unwrap_or(LAN_PORT)));
    }
    #[cfg(not(target_os = "ios"))]
    if addrs.is_empty() {
        addrs = mdns::find_pairing_hosts(Duration::from_secs(4)).await;
    }
    e.join(&code, &addrs).await
}

pub async fn sync_unpair() -> Result<Value, String> {
    Ok(need()?.unpair().await)
}

pub async fn sync_now() -> Result<Value, String> {
    let e = need()?;
    e.sync_round(true).await;
    e.emit_changed(None);
    Ok(e.status())
}

/// The app came to the front (`true`) or went to the background. iOS calls it on scene phase,
/// the desktop on window focus. Coming to the front syncs at once.
pub async fn sync_set_active(active: bool) -> Result<Value, String> {
    let e = need()?;
    e.set_active(active);
    Ok(e.status())
}

pub async fn sync_cancel_pairing() -> Result<Value, String> {
    let e = need()?;
    e.cancel_pairing();
    e.emit_changed(None);
    Ok(e.status())
}
