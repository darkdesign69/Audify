//! `_spotless-sync._tcp` on the LAN (Mac and other desktops; the iPhone uses Bonjour from Swift,
//! because raw multicast on iOS needs a special entitlement).
//!
//! A device advertises while it is paired or showing a code. The TXT record carries its device id,
//! `tag` (8 hex chars derived from the group key, so peers recognise each other without the pair
//! id going on the air) and `pairing=1` while a code is up. Browsing keeps each known peer's address
//! current, so a phone that changed IP is still found.

use std::sync::Arc;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::db::Db;

pub const SERVICE: &str = "_spotless-sync._tcp.local.";

pub struct Mdns {
    daemon: ServiceDaemon,
}

impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

impl Mdns {
    pub fn start(db: Arc<Db>, dev: &str, port: u16, tag: Option<String>, pairing: bool) -> Option<Self> {
        let daemon = ServiceDaemon::new().map_err(|e| tracing::warn!(error = %e, "sync: mDNS unavailable")).ok()?;
        let mut props = vec![("dev", dev.to_string()), ("v", "1".to_string())];
        if let Some(t) = &tag {
            props.push(("tag", t.clone()));
        }
        if pairing {
            props.push(("pairing", "1".to_string()));
        }
        let host = format!("spotless-{}.local.", &dev[..dev.len().min(12)]);
        match ServiceInfo::new(SERVICE, dev, &host, "", port, &props[..]) {
            Ok(info) => {
                if let Err(e) = daemon.register(info.enable_addr_auto()) {
                    tracing::warn!(error = %e, "sync: mDNS register failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "sync: mDNS service info"),
        }
        if let (Some(tag), Ok(rx)) = (tag, daemon.browse(SERVICE)) {
            let me = dev.to_string();
            std::thread::Builder::new()
                .name("sync-mdns".into())
                .spawn(move || {
                    while let Ok(ev) = rx.recv() {
                        let ServiceEvent::ServiceResolved(info) = ev else { continue };
                        if info.get_property_val_str("tag") != Some(tag.as_str()) {
                            continue;
                        }
                        let Some(peer) = info.get_property_val_str("dev").map(str::to_owned) else { continue };
                        if peer == me {
                            continue;
                        }
                        let Some(ip) = info.get_addresses_v4().into_iter().next().map(|a| a.to_string()) else { continue };
                        let conn = db.conn();
                        // Only refresh peers we paired with; a stranger with our tag would need the key.
                        let _ = conn.execute(
                            "UPDATE sync_peers SET addr = ?2, port = ?3 WHERE dev = ?1",
                            rusqlite::params![peer, ip, info.get_port() as i64],
                        );
                    }
                })
                .ok();
        }
        Some(Self { daemon })
    }
}

/// Devices showing a pairing code right now, as `ip:port`.
pub async fn find_pairing_hosts(wait: Duration) -> Vec<String> {
    crate::rt::spawn_blocking(move || {
        let Ok(daemon) = ServiceDaemon::new() else { return vec![] };
        let Ok(rx) = daemon.browse(SERVICE) else { return vec![] };
        let deadline = std::time::Instant::now() + wait;
        let mut out = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(ServiceEvent::ServiceResolved(info)) if info.get_property_val_str("pairing") == Some("1") => {
                    for a in info.get_addresses_v4() {
                        out.push(super::net::join_addr(&a.to_string(), info.get_port()));
                    }
                    if !out.is_empty() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = daemon.shutdown();
        out
    })
    .await
    .unwrap_or_default()
}
