//! The LAN side of sync: length-prefixed JSON frames over TCP (docs/SYNC.md §5).
//!
//! One connection carries either a pairing (SPAKE2 with the 6-digit code, then the group key sent
//! inside the PAKE key) or any number of sync exchanges (each one an encrypted request with the
//! client's new fields and an encrypted reply with the server's). Everything after pairing is
//! sealed with the group key, so the server answers nobody outside the group, and a replayed
//! message changes nothing (ops are idempotent).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::crypto::{self, BoxKey};
use super::store::{self, Op};
use super::{upsert_peer, Engine, Group, Peer, LAN_BATCH};

const MAX_FRAME: usize = 64 << 20;
const IO_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const ID_JOIN: &[u8] = b"spotless-sync join";
const ID_HOST: &[u8] = b"spotless-sync host";

/// Who a device is, as it introduces itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub dev: String,
    pub name: String,
    pub platform: String,
    /// Its LAN server's port.
    #[serde(default)]
    pub port: Option<u16>,
    /// Its last known address (in a peer list handed to a new device).
    #[serde(default)]
    pub addr: Option<String>,
}

/// What a joining device receives once the code checked out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    pub pair_id: String,
    /// The group key, hex.
    pub key: String,
    /// Where the host's relay reading is, so the joiner doesn't re-read the mailbox history
    /// (it gets the full state over the LAN instead).
    pub relay_recv: i64,
    pub host: Hello,
    pub peers: Vec<Hello>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Wire {
    Pair1 { spake: String },
    Pair2 { spake: String },
    Pair3 { confirm: String, #[serde(rename = "box")] sealed: String },
    Pair4 { confirm: String, #[serde(rename = "box")] sealed: String },
    Sync { pair: String, from: String, #[serde(rename = "box")] sealed: String },
    Err { error: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncReq {
    leave: bool,
    hello: Hello,
    /// The server's seq the client has everything up to.
    since: i64,
    ops: Vec<Op>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncResp {
    hello: Hello,
    ops: Vec<Op>,
    /// The client is up to date with the server up to here.
    cursor: i64,
    more: bool,
}

async fn write(s: &mut TcpStream, w: &Wire) -> Result<(), String> {
    let body = serde_json::to_vec(w).map_err(|e| e.to_string())?;
    let fut = async {
        s.write_all(&(body.len() as u32).to_be_bytes()).await?;
        s.write_all(&body).await?;
        s.flush().await
    };
    tokio::time::timeout(IO_TIMEOUT, fut).await.map_err(|_| "timeout".to_string())?.map_err(|e| e.to_string())
}

async fn read(s: &mut TcpStream) -> Result<Option<Wire>, String> {
    let fut = async {
        let mut len = [0u8; 4];
        match s.read_exact(&mut len).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.to_string()),
        }
        let n = u32::from_be_bytes(len) as usize;
        if n > MAX_FRAME {
            return Err("frame too large".into());
        }
        let mut body = vec![0u8; n];
        s.read_exact(&mut body).await.map_err(|e| e.to_string())?;
        serde_json::from_slice(&body).map(Some).map_err(|e| e.to_string())
    };
    tokio::time::timeout(IO_TIMEOUT, fut).await.map_err(|_| "timeout".to_string())?
}

async fn expect(s: &mut TcpStream) -> Result<Wire, String> {
    match read(s).await? {
        Some(Wire::Err { error }) => Err(error),
        Some(w) => Ok(w),
        None => Err("closed".into()),
    }
}

fn aad(pair: &str, from: &str, kind: &str) -> String {
    format!("{pair}|{from}|{kind}")
}

/// Accept connections until the task is dropped.
pub async fn serve(engine: Arc<Engine>, listener: TcpListener) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else { continue };
        let e = engine.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(e, stream, peer.ip().to_string()).await {
                tracing::debug!(error = %err, "sync: LAN connection ended");
            }
        });
    }
}

async fn handle(engine: Arc<Engine>, mut s: TcpStream, ip: String) -> Result<(), String> {
    let _ = s.set_nodelay(true);
    while let Some(msg) = read(&mut s).await? {
        match msg {
            Wire::Pair1 { spake } => return host_pairing(&engine, s, &spake, ip).await,
            Wire::Sync { pair, from, sealed } => {
                let reply = serve_sync(&engine, &pair, &from, &sealed, &ip);
                let done = !matches!(reply, Wire::Sync { .. });
                write(&mut s, &reply).await?;
                if done {
                    return Ok(());
                }
            }
            _ => {
                write(&mut s, &Wire::Err { error: "unexpected".into() }).await?;
                return Ok(());
            }
        }
    }
    Ok(())
}

fn serve_sync(engine: &Arc<Engine>, pair: &str, from: &str, sealed: &str, ip: &str) -> Wire {
    let Some(group) = engine.group().filter(|g| g.pair_id == pair) else {
        return Wire::Err { error: "unknown_pair".into() };
    };
    let key = BoxKey::for_group(&group.key);
    let Some(req) = key.open(&aad(pair, from, "req"), sealed).and_then(|b| serde_json::from_slice::<SyncReq>(&b).ok())
    else {
        return Wire::Err { error: "auth".into() };
    };
    let me = engine.device_id();
    if req.hello.dev != from || from == me {
        return Wire::Err { error: "auth".into() };
    }
    engine.apply(&req.ops);
    if req.leave {
        engine.peer_left(from);
        if engine.group().is_none() {
            // The last other device left: nobody will read the mailbox again.
            let e = engine.clone();
            tokio::spawn(async move {
                if let Some(mb) = &e.mailbox {
                    let _ = mb.delete(&group).await;
                }
            });
        }
        let resp = SyncResp { hello: engine.hello(), ops: vec![], cursor: req.since, more: false };
        return Wire::Sync { pair: pair.into(), from: me.clone(), sealed: key.seal(&aad(pair, &me, "resp"), &serde_json::to_vec(&resp).unwrap_or_default()) };
    }
    let (ops, cursor, more) = {
        let conn = engine.db.conn();
        upsert_peer(&conn, from, Some(&req.hello.name), Some(&req.hello.platform), Some(ip), req.hello.port);
        let upto = store::head(&conn);
        store::ops_since(&conn, req.since, upto, Some(from), None, LAN_BATCH).unwrap_or((vec![], req.since, false))
    };
    let resp = SyncResp { hello: engine.hello(), ops, cursor, more };
    Wire::Sync { pair: pair.into(), from: me.clone(), sealed: key.seal(&aad(pair, &me, "resp"), &serde_json::to_vec(&resp).unwrap_or_default()) }
}

/// The client side of one sync with `peer` at `addr`: send everything it hasn't acknowledged,
/// take everything new it has, until both sides are drained. `leave` says goodbye instead.
pub async fn exchange(engine: &Arc<Engine>, group: &Group, peer: &Peer, addr: &str, leave: bool) -> Result<(), String> {
    let mut s = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    let key = BoxKey::for_group(&group.key);
    let me = engine.device_id();
    let (mut sent, mut recv) = (peer.sent, peer.recv);
    for _ in 0..10_000 {
        let (ops, my_cursor, my_more) = if leave {
            (vec![], sent, false)
        } else {
            let conn = engine.db.conn();
            let upto = store::head(&conn);
            store::ops_since(&conn, sent, upto, Some(&peer.id), None, LAN_BATCH).map_err(|e| e.to_string())?
        };
        let req = SyncReq { leave, hello: engine.hello(), since: recv, ops };
        let sealed = key.seal(&aad(&group.pair_id, &me, "req"), &serde_json::to_vec(&req).map_err(|e| e.to_string())?);
        write(&mut s, &Wire::Sync { pair: group.pair_id.clone(), from: me.clone(), sealed }).await?;
        let Wire::Sync { pair, from, sealed } = expect(&mut s).await? else { return Err("unexpected".into()) };
        if pair != group.pair_id {
            return Err("unknown_pair".into());
        }
        let resp: SyncResp = key
            .open(&aad(&pair, &from, "resp"), &sealed)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .ok_or("auth")?;
        if resp.hello.dev != from || from == me {
            return Err("auth".into());
        }
        if leave {
            return Ok(());
        }
        engine.apply(&resp.ops);
        sent = my_cursor;
        recv = resp.cursor;
        {
            let conn = engine.db.conn();
            // The address answered as `from`; normally that is `peer` itself.
            let (host, _) = split_addr(addr);
            upsert_peer(&conn, &from, Some(&resp.hello.name), Some(&resp.hello.platform), Some(&host), resp.hello.port);
            let _ = conn.execute(
                "UPDATE sync_peers SET sent = MAX(sent, ?2), recv = ?3 WHERE dev = ?1",
                rusqlite::params![from, sent, recv],
            );
        }
        if !my_more && !resp.more {
            return Ok(());
        }
    }
    Ok(())
}

// ── pairing ──────────────────────────────────────────────────────────────────────────────────

struct PakeKeys {
    confirm_join: [u8; 32],
    confirm_host: [u8; 32],
    sealed: BoxKey,
}

fn pake_keys(k: &[u8]) -> PakeKeys {
    PakeKeys {
        confirm_join: crypto::derive(k, "spotless-sync v1 confirm join"),
        confirm_host: crypto::derive(k, "spotless-sync v1 confirm host"),
        sealed: BoxKey::raw(crypto::derive(k, "spotless-sync v1 pairing box")),
    }
}

/// The device showing the code. Each connection costs the code one of its tries, so an attacker
/// gets [`super::PAIRING_TRIES`] guesses at a 1-in-a-million code, and SPAKE2 gives them nothing
/// to test further guesses against offline.
async fn host_pairing(engine: &Arc<Engine>, mut s: TcpStream, spake_a: &str, ip: String) -> Result<(), String> {
    let code = match engine.take_try() {
        Ok(c) => c,
        Err(e) => {
            write(&mut s, &Wire::Err { error: e.clone() }).await?;
            return Err(e);
        }
    };
    let (state, msg_b) =
        Spake2::<Ed25519Group>::start_b(&Password::new(code.as_bytes()), &Identity::new(ID_JOIN), &Identity::new(ID_HOST));
    write(&mut s, &Wire::Pair2 { spake: crypto::b64(&msg_b) }).await?;
    let k = crypto::unb64(spake_a).and_then(|a| state.finish(&a).ok());
    let Wire::Pair3 { confirm, sealed } = expect(&mut s).await? else { return Err("unexpected".into()) };
    let Some(k) = k else {
        let left = engine.wrong_code();
        write(&mut s, &Wire::Err { error: format!("wrong_code:{left}") }).await?;
        return Err("pake".into());
    };
    let keys = pake_keys(&k);
    let ok = crypto::unb64(&confirm).is_some_and(|c| crypto::same(&c, &keys.confirm_join));
    let hello = keys.sealed.open("pair|join", &sealed).and_then(|b| serde_json::from_slice::<Hello>(&b).ok());
    let (true, Some(hello)) = (ok, hello) else {
        // The joiner shows how many tries are left (`wrong_code:<n>`); none left ends the window.
        let left = engine.wrong_code();
        write(&mut s, &Wire::Err { error: format!("wrong_code:{left}") }).await?;
        return Err("wrong_code".into());
    };
    let welcome = engine.admit(&hello, Some(ip))?;
    let body = serde_json::to_vec(&welcome).map_err(|e| e.to_string())?;
    write(&mut s, &Wire::Pair4 { confirm: crypto::b64(&keys.confirm_host), sealed: keys.sealed.seal("pair|host", &body) }).await?;
    engine.pairing_done();
    engine.mark_changed();
    engine.emit_changed(None);
    Ok(())
}

/// The device typing the code.
pub async fn join(engine: &Arc<Engine>, addr: &str, code: &str) -> Result<Welcome, String> {
    let mut s = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timeout".to_string())?
        .map_err(|e| e.to_string())?;
    let (state, msg_a) =
        Spake2::<Ed25519Group>::start_a(&Password::new(code.as_bytes()), &Identity::new(ID_JOIN), &Identity::new(ID_HOST));
    write(&mut s, &Wire::Pair1 { spake: crypto::b64(&msg_a) }).await?;
    let Wire::Pair2 { spake } = expect(&mut s).await? else { return Err("unexpected".into()) };
    let k = crypto::unb64(&spake).and_then(|b| state.finish(&b).ok()).ok_or("pake")?;
    let keys = pake_keys(&k);
    let hello = serde_json::to_vec(&engine.hello()).map_err(|e| e.to_string())?;
    write(&mut s, &Wire::Pair3 { confirm: crypto::b64(&keys.confirm_join), sealed: keys.sealed.seal("pair|join", &hello) }).await?;
    let Wire::Pair4 { confirm, sealed } = expect(&mut s).await? else { return Err("unexpected".into()) };
    if !crypto::unb64(&confirm).is_some_and(|c| crypto::same(&c, &keys.confirm_host)) {
        return Err("wrong_code".into());
    }
    keys.sealed
        .open("pair|host", &sealed)
        .and_then(|b| serde_json::from_slice::<Welcome>(&b).ok())
        .ok_or_else(|| "wrong_code".into())
}

// ── addresses and the QR ─────────────────────────────────────────────────────────────────────

/// This device's LAN address (the interface the default route uses). No packet is sent.
pub fn local_addrs() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.0.2.1:9").is_ok() {
            if let Ok(a) = sock.local_addr() {
                if !a.ip().is_unspecified() && !a.ip().is_loopback() {
                    out.push(a.ip().to_string());
                }
            }
        }
    }
    out
}

pub fn join_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// `host:port` → (host, port).
pub fn split_addr(addr: &str) -> (String, Option<u16>) {
    match addr.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() && (!h.contains(':') || h.starts_with('[')) => {
            (h.trim_start_matches('[').trim_end_matches(']').to_string(), p.parse().ok())
        }
        _ => (addr.to_string(), None),
    }
}

pub fn peer_addr(p: &Peer) -> Option<String> {
    Some(join_addr(p.addr.as_deref()?, p.port?))
}

/// What the QR code holds: `spotless://pair?v=1&code=…&addr=ip:port,…&dev=…&name=…`.
pub fn qr_payload(code: &str, addrs: &[String], port: u16, dev: &str, name: &str) -> String {
    let addr: Vec<String> = addrs.iter().map(|a| join_addr(a, port)).collect();
    format!(
        "spotless://pair?v=1&code={code}&addr={}&dev={dev}&name={}",
        urlencoding::encode(&addr.join(",")),
        urlencoding::encode(name)
    )
}

#[derive(Debug, PartialEq)]
pub struct Qr {
    pub code: String,
    pub addrs: Vec<String>,
    pub dev: Option<String>,
}

pub fn parse_qr(s: &str) -> Option<Qr> {
    let q = s.trim().strip_prefix("spotless://pair?")?;
    let mut code = None;
    let mut addrs = Vec::new();
    let mut dev = None;
    for kv in q.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_default();
        match k {
            "code" => code = Some(v),
            "addr" => addrs = v.split(',').filter(|a| !a.is_empty()).map(str::to_owned).collect(),
            "dev" => dev = Some(v),
            _ => {}
        }
    }
    Some(Qr { code: code?, addrs, dev })
}
