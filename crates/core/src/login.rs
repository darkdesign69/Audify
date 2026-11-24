//! Turning a Google sign-in webview into a Spotless sign-in (context/15 Path A).
//!
//! The platform opens the webview and knows how to read its cookie jar; this module watches that
//! jar until Google has handed it a YouTube session (a `SAPISID` cookie), then signs Spotless in
//! with it through the same [`AppState::sign_in`] every other path uses.
//!
//! It polls on a timer instead of waiting for "music.youtube.com finished loading": Google can set
//! the cookies a while after that page loads, and YouTube Music navigates client-side, so no
//! further load event comes. Page loads only wake the poll early. A Mac that relied on the load
//! event alone sat on a logged-in YouTube Music page with Spotless still signed out.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::host::HostExt;
use crate::state::{AppState, SignInOutcome};

/// How often the jar is read while the sign-in webview is open.
pub const POLL: Duration = Duration::from_secs(1);
/// How long a sign-in may take before it is given up. Long enough for a password, a 2-step code
/// and a slow phone prompt.
pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How a sign-in attempt ended. Everything except [`End::Cancelled`] means the platform should
/// now close the sign-in webview.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// Signed in; `login-done` was emitted (and `auth-changed`, by `sign_in`).
    SignedIn,
    /// The account has several channels; `account-selection-required` opens the picker.
    PickChannel,
    /// `login-error` was emitted with the reason.
    Failed,
    /// The user closed the sign-in webview; `login-cancelled` was emitted.
    Cancelled,
}

/// Watch the sign-in webview's jar and sign in once it holds a YouTube session.
///
/// `read_jar` returns the webview's youtube.com `Cookie` header (empty while there is none yet), or
/// `None` once the webview is gone. `nudge` is pinged on page loads so a check runs at once rather
/// than at the next tick.
pub async fn capture<F, Fut>(
    state: &AppState,
    mut read_jar: F,
    nudge: &mut UnboundedReceiver<()>,
    poll: Duration,
    timeout: Duration,
) -> End
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let cookie = loop {
        let Some(jar) = read_jar().await else {
            let _ = state.host.emit("login-cancelled", ());
            return End::Cancelled;
        };
        if innertube::cookie_sapisid(&jar).is_some() {
            break jar;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = state.host.emit("login-error", "Sign-in timed out. Try again.");
            return End::Failed;
        }
        tokio::select! {
            _ = tokio::time::sleep(poll) => {}
            Some(()) = nudge.recv() => {}
        }
    };
    tracing::info!("sign-in webview has a YouTube session; signing in");
    match state.sign_in(cookie, None).await {
        Ok(SignInOutcome::Complete) => {
            let _ = state.host.emit("login-done", ());
            End::SignedIn
        }
        // The cookie is saved; the account stays unfinished until the picker in the main window
        // selects a channel.
        Ok(SignInOutcome::SelectionRequired) => End::PickChannel,
        Err(e) => {
            tracing::warn!(error = %e, "sign-in with the webview's session failed");
            let _ = state.host.emit("login-error", e);
            End::Failed
        }
    }
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
