//! The opena.tech mailbox (docs/SYNC.md §5.2): for devices that can't reach each other directly.
//!
//! `POST /api/sync/<pair_id>` appends `{from, box}`; `GET ?after=<cursor>` reads what came after.
//! Each box is a [`Msg`] sealed with the group key, with `pair|from|relay` as associated data, so
//! the relay (or anyone who learns the pair id) can neither read nor forge one. The bearer token is
//! derived from the group key; the relay stores its hash on first use.
//!
//! Each device posts only its own changes (every device reads the mailbox, so nothing needs
//! forwarding), and skips posting what every peer already took over the LAN. A device that was
//! away longer than the relay keeps messages (30 days) sees a gap and posts `resync`; the others
//! answer with their full state.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::crypto::{self, BoxKey};
use super::store::{self, Op};
use super::{load_group, upsert_peer, Engine, Group};

/// Plaintext per POST before sealing; base64 and the envelope keep a box under the relay's 1 MiB.
const CHUNK_BYTES: usize = 600_000;
const TIMEOUT: Duration = Duration::from_secs(15);

pub fn default_base() -> String {
    std::env::var("SPOTLESS_SYNC_RELAY").unwrap_or_else(|_| "https://opena.tech/api/sync".into())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: i64,
    pub from: String,
    #[serde(rename = "box")]
    pub sealed: String,
    #[serde(default)]
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub items: Vec<Item>,
    pub next: i64,
    pub oldest: Option<i64>,
    #[serde(default)]
    pub more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Msg {
    /// `ops` | `resync` | `leave`
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub ops: Vec<Op>,
}

impl Msg {
    pub fn leave() -> Self {
        Self { kind: "leave".into(), ..Default::default() }
    }
}

/// An in-process mailbox with the relay's semantics (tests).
#[derive(Default)]
pub struct Memory {
    pub items: Vec<Item>,
    pub next_id: i64,
}

impl Memory {
    /// Forget everything up to `id` (the relay's 30-day expiry).
    pub fn expire_through(&mut self, id: i64) {
        self.items.retain(|i| i.id > id);
    }
}

pub enum Mailbox {
    Http { base: String },
    Memory(Arc<std::sync::Mutex<Memory>>),
}

impl Mailbox {
    pub fn http(base: String) -> Self {
        Mailbox::Http { base: base.trim_end_matches('/').to_string() }
    }

    fn url(base: &str, g: &Group) -> String {
        format!("{base}/{}", g.pair_id)
    }

    pub async fn post(&self, g: &Group, from: &str, sealed: String) -> Result<i64, String> {
        match self {
            Mailbox::Http { base } => {
                let r = crate::http::client()
                    .post(Self::url(base, g))
                    .bearer_auth(crypto::relay_token(&g.key))
                    .json(&serde_json::json!({ "from": from, "box": sealed }))
                    .timeout(TIMEOUT)
                    .send()
                    .await
                    .map_err(|e| format!("relay: {e}"))?;
                if !r.status().is_success() {
                    return Err(format!("relay: {}", r.status()));
                }
                let v: serde_json::Value = r.json().await.map_err(|e| format!("relay: {e}"))?;
                Ok(v["id"].as_i64().unwrap_or(0))
            }
            Mailbox::Memory(m) => {
                let mut m = m.lock().unwrap();
                m.next_id += 1;
                let id = m.next_id;
                m.items.push(Item { id, from: from.into(), sealed, at: 0 });
                Ok(id)
            }
        }
    }

    pub async fn get(&self, g: &Group, after: i64) -> Result<Page, String> {
        match self {
            Mailbox::Http { base } => {
                let r = crate::http::client()
                    .get(format!("{}?after={after}&limit=100", Self::url(base, g)))
                    .bearer_auth(crypto::relay_token(&g.key))
                    .timeout(TIMEOUT)
                    .send()
                    .await
                    .map_err(|e| format!("relay: {e}"))?;
                if !r.status().is_success() {
                    return Err(format!("relay: {}", r.status()));
                }
                r.json().await.map_err(|e| format!("relay: {e}"))
            }
            Mailbox::Memory(m) => {
                let m = m.lock().unwrap();
                let items: Vec<Item> = m.items.iter().filter(|i| i.id > after).take(100).cloned().collect();
                let more = m.items.iter().filter(|i| i.id > after).count() > items.len();
                Ok(Page {
                    next: items.last().map_or(after, |i| i.id),
                    oldest: m.items.first().map(|i| i.id),
                    more,
                    items,
                })
            }
        }
    }

    pub async fn delete(&self, g: &Group) -> Result<(), String> {
        match self {
            Mailbox::Http { base } => {
                crate::http::client()
                    .delete(Self::url(base, g))
                    .bearer_auth(crypto::relay_token(&g.key))
                    .timeout(TIMEOUT)
                    .send()
                    .await
                    .map_err(|e| format!("relay: {e}"))?;
                Ok(())
            }
            Mailbox::Memory(m) => {
                m.lock().unwrap().items.clear();
                Ok(())
            }
        }
    }
}

fn aad(g: &Group, from: &str) -> String {
    format!("{}|{from}|relay", g.pair_id)
}

pub async fn post_msg(mb: &Mailbox, g: &Group, me: &str, msg: &Msg) -> Result<i64, String> {
    let body = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    mb.post(g, me, BoxKey::for_group(&g.key).seal(&aad(g, me), &body)).await
}

/// Split ops into messages that fit a POST.
fn chunks(ops: Vec<Op>) -> Vec<Vec<Op>> {
    let mut out = vec![];
    let mut cur = vec![];
    let mut size = 0;
    for op in ops {
        let n = serde_json::to_vec(&op).map(|v| v.len()).unwrap_or(0) + 1;
        if size + n > CHUNK_BYTES && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            size = 0;
        }
        size += n;
        cur.push(op);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Read the mailbox, then post this device's changes.
pub async fn round(engine: &Arc<Engine>, mb: &Mailbox) -> Result<(), String> {
    pull(engine, mb).await?;
    push(engine, mb).await
}

async fn pull(engine: &Arc<Engine>, mb: &Mailbox) -> Result<(), String> {
    let me = engine.device_id();
    for page_no in 0..50 {
        let Some(g) = engine.group() else { return Ok(()) };
        let page = mb.get(&g, g.relay_recv).await?;
        // Messages this device never saw are gone (it was away past the relay's TTL).
        if page_no == 0 && g.relay_recv > 0 && page.oldest.is_some_and(|o| o > g.relay_recv + 1) {
            tracing::info!("sync: relay history expired past our cursor, asking for a resync");
            let ask = Msg { kind: "resync".into(), name: super::device_name(), platform: super::platform().into(), ops: vec![] };
            post_msg(mb, &g, &me, &ask).await?;
        }
        let key = BoxKey::for_group(&g.key);
        for item in &page.items {
            if item.from == me {
                continue;
            }
            let Some(msg) = key.open(&aad(&g, &item.from), &item.sealed).and_then(|b| serde_json::from_slice::<Msg>(&b).ok())
            else {
                tracing::warn!("sync: a relay message failed to open; skipped");
                continue;
            };
            match msg.kind.as_str() {
                "leave" => {
                    engine.peer_left(&item.from);
                    if engine.group().is_none() {
                        // The last other device left: nobody will read the mailbox again.
                        let _ = mb.delete(&g).await;
                        return Ok(());
                    }
                    continue;
                }
                "resync" => {
                    let _ = engine.db.conn().execute("UPDATE sync_group SET relay_full = 1 WHERE id = 1", []);
                }
                _ => {
                    engine.apply(&msg.ops);
                }
            }
            let conn = engine.db.conn();
            upsert_peer(&conn, &item.from, Some(&msg.name).filter(|n| !n.is_empty()).map(|s| s.as_str()), Some(&msg.platform).filter(|n| !n.is_empty()).map(|s| s.as_str()), None, None);
        }
        let _ = engine.db.conn().execute("UPDATE sync_group SET relay_recv = MAX(relay_recv, ?1) WHERE id = 1", [page.next]);
        if !page.more {
            break;
        }
    }
    Ok(())
}

async fn push(engine: &Arc<Engine>, mb: &Mailbox) -> Result<(), String> {
    let me = engine.device_id();
    let (g, ops, upto) = {
        let conn = engine.db.conn();
        let Some(g) = load_group(&conn) else { return Ok(()) };
        let upto = store::head(&conn);
        let ops = if g.relay_full {
            store::all_ops(&conn).map_err(|e| e.to_string())?
        } else {
            let (ops, _, _) = store::ops_since(&conn, g.relay_sent, upto, None, Some(&me), usize::MAX / 2).map_err(|e| e.to_string())?;
            let peers = super::peers(&conn);
            let lan_done = !peers.is_empty() && peers.iter().all(|p| p.sent >= upto);
            if lan_done { vec![] } else { ops }
        };
        (g, ops, upto)
    };
    for chunk in chunks(ops) {
        let msg = Msg { kind: "ops".into(), name: super::device_name(), platform: super::platform().into(), ops: chunk };
        post_msg(mb, &g, &me, &msg).await?;
    }
    let _ = engine.db.conn().execute(
        "UPDATE sync_group SET relay_sent = MAX(relay_sent, ?1), relay_full = 0 WHERE id = 1",
        [upto],
    );
    Ok(())
}
