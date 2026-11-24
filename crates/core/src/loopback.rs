//! The loopback listener the audio and video proxies serve on, made to survive losing its socket.
//!
//! **Why.** iOS reclaims the listening sockets of a suspended app: lock the phone with the music
//! paused for ten minutes and the port is gone when the app comes back. Nothing in-process notices
//! (the accept loop just never wakes again), so every stream URL handed to mpv afterwards points at
//! a port that refuses the connection, each track "fails", and the queue walks itself to the end
//! (187 skips on the owner's phone, docs/streams/proxyfix.md). The desktop can lose one too, to a
//! sandbox or a firewall restart, just far more rarely.
//!
//! So the listener can be checked and rebuilt: [`Loopback::ensure_alive`] connects to its own port
//! and, when that is refused, binds again, on the same port when it is free (every URL already
//! handed out works again) or on a new one (URLs registered from then on carry it). The token is
//! per launch, not per socket, for the same reason. Callers check on app foreground, on audio
//! session activation, before every stream registration, and on a track failure, before deciding
//! whether the track or the proxy was at fault.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Serves one accepted connection (spawns its own task).
pub type Accept = Arc<dyn Fn(tokio::net::TcpStream) + Send + Sync>;

/// What [`Loopback::ensure_alive`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// The listener answered.
    Alive,
    /// It was dead and is listening again. `moved`: on a different port, so URLs handed out
    /// before this point are dead for good and have to be registered again.
    Rebound { port: u16, moved: bool },
    /// Dead, and binding again failed too.
    Down,
}

impl Health {
    /// The proxy was found dead (whether or not it could be brought back).
    pub fn was_dead(self) -> bool {
        !matches!(self, Health::Alive)
    }
}

/// How long the self-connect may take. Loopback answers in microseconds when anything listens; a
/// refusal is immediate. The bound only matters for a wedged network stack.
const PROBE: Duration = Duration::from_millis(250);

pub struct Loopback {
    name: &'static str,
    token: String,
    accept: Accept,
    bound: Mutex<Option<Bound>>,
}

struct Bound {
    port: u16,
    task: tokio::task::AbortHandle,
}

impl Loopback {
    /// Bind `127.0.0.1:<ephemeral>` and start accepting. Binds synchronously, so [`Self::port`] is
    /// usable the moment this returns.
    pub fn start(name: &'static str, accept: Accept) -> io::Result<Self> {
        let token = format!("{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>());
        let me = Loopback { name, token, accept, bound: Mutex::new(None) };
        let b = me.bind(0)?;
        tracing::info!(port = b.port, "{name} listening on loopback");
        *me.bound.lock().unwrap_or_else(|e| e.into_inner()) = Some(b);
        Ok(me)
    }

    /// This launch's path token. Survives a rebind.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The port being served, as last bound (it may have died since: see [`Self::ensure_alive`]).
    pub fn port(&self) -> Option<u16> {
        self.bound.lock().ok()?.as_ref().map(|b| b.port)
    }

    /// Check the listener with a self-connect and rebuild it if that was refused. Cheap enough to
    /// call before every stream registration: one loopback handshake.
    pub fn ensure_alive(&self) -> Health {
        let mut bound = self.bound.lock().unwrap_or_else(|e| e.into_inner());
        let old = bound.as_ref().map(|b| b.port);
        if let Some(port) = old {
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            if TcpStream::connect_timeout(&addr, PROBE).is_ok() {
                return Health::Alive;
            }
            tracing::warn!(port, "{}: listener is gone, rebinding", self.name);
        }
        if let Some(b) = bound.take() {
            b.task.abort();
        }
        match self.rebind(old) {
            Ok(b) => {
                let port = b.port;
                let moved = old != Some(port);
                tracing::info!(port, moved, "{}: listening again", self.name);
                *bound = Some(b);
                Health::Rebound { port, moved }
            }
            Err(e) => {
                tracing::error!(error = %e, "{}: rebind failed", self.name);
                Health::Down
            }
        }
    }

    /// Debug hook (`spotless://debug?audio=kill-proxy`): close the listener the way iOS does to a
    /// suspended app, keeping the port on record so the next check finds it dead.
    pub fn kill(&self) {
        if let Ok(bound) = self.bound.lock() {
            if let Some(b) = bound.as_ref() {
                tracing::warn!(port = b.port, "{}: killed (debug)", self.name);
                b.task.abort();
            }
        }
    }

    /// The same port when it can be had (the old socket is dropped asynchronously after its task
    /// is aborted, so give it a moment), else any free one.
    fn rebind(&self, prefer: Option<u16>) -> io::Result<Bound> {
        if let Some(port) = prefer {
            for _ in 0..20 {
                match self.bind(port) {
                    Ok(b) => return Ok(b),
                    Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        }
        self.bind(0)
    }

    fn bind(&self, port: u16) -> io::Result<Bound> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (name, accept) = (self.name, self.accept.clone());
        let task = crate::rt::spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return };
            // Out of descriptors or buffers (EMFILE/ENFILE/ENOBUFS) is transient and returns at
            // once, so it waits rather than spins. A socket the system took away (iOS: EBADF and
            // friends) fails the same way forever: after a few seconds of nothing but errors the
            // listener is dropped, and the next `ensure_alive` finds the port refusing and rebinds.
            let mut errors = 0;
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        errors = 0;
                        accept(stream);
                    }
                    Err(e) => {
                        errors += 1;
                        if errors >= MAX_ACCEPT_ERRORS {
                            tracing::warn!(error = %e, "{name}: listener died");
                            return;
                        }
                        tracing::warn!(error = %e, "{name}: accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        })
        .abort_handle();
        Ok(Bound { port, task })
    }
}

/// Check both proxies (and rebuild a dead one). Called by the host on app foreground and on audio
/// session activation. True when one was found dead.
pub fn check_proxies(reason: &str) -> bool {
    let audio = crate::audioproxy::ensure_alive();
    let video = crate::videoproxy::ensure_alive();
    let dead = [audio, video].into_iter().flatten().any(Health::was_dead);
    if dead {
        tracing::warn!(reason, ?audio, ?video, "loopback proxies were dead");
    } else {
        tracing::debug!(reason, "loopback proxies alive");
    }
    dead
}

/// Debug hook: close both listeners the way iOS does to a suspended app.
pub fn kill_proxies() {
    crate::audioproxy::kill();
    crate::videoproxy::kill();
}

/// Back-to-back accept failures (100 ms apart) before the listener is given up on.
const MAX_ACCEPT_ERRORS: u32 = 50;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Answers every connection with a fixed line, so a test can tell it is really being served.
    fn echo_server() -> Loopback {
        Loopback::start(
            "test proxy",
            Arc::new(|mut s: tokio::net::TcpStream| {
                crate::rt::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let _ = s.write_all(b"ok").await;
                });
            }),
        )
        .expect("bind")
    }

    fn served(port: u16) -> bool {
        let Ok(mut s) = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), PROBE)
        else {
            return false;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = [0u8; 2];
        s.read_exact(&mut buf).is_ok() && &buf == b"ok"
    }

    fn wait_refused(port: u16) {
        for _ in 0..100 {
            if TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), PROBE).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("listener on {port} never went away");
    }

    #[test]
    fn a_live_listener_is_left_alone() {
        let lb = echo_server();
        let port = lb.port().unwrap();
        assert_eq!(lb.ensure_alive(), Health::Alive);
        assert_eq!(lb.port(), Some(port));
        assert!(served(port));
    }

    // The bug on the owner's phone: the socket is reclaimed while the app is suspended, every URL
    // then points at a port that refuses. The check has to notice and serve again, on the same
    // port, so the URLs already handed to mpv work again too.
    #[test]
    fn a_killed_listener_is_rebound_on_the_same_port() {
        let lb = echo_server();
        let port = lb.port().unwrap();
        let token = lb.token().to_owned();
        lb.kill();
        wait_refused(port);
        assert_eq!(lb.ensure_alive(), Health::Rebound { port, moved: false });
        assert!(served(port));
        assert_eq!(lb.token(), token, "a rebind keeps the token, so old URLs stay valid");
        assert_eq!(lb.ensure_alive(), Health::Alive);
    }

    // Something else took the port while we were gone: serve on a new one rather than not at all.
    #[test]
    fn a_taken_port_moves_the_listener() {
        let lb = echo_server();
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let taken = held.local_addr().unwrap().port();
        let b = lb.rebind(Some(taken)).expect("falls back to an ephemeral port");
        assert_ne!(b.port, taken);
        b.task.abort();
    }
}
