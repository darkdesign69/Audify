//! YouTube sign-in. The desktop app opens a second webview on Google's login page and reads its
//! cookies. A Tauri Android app has a single webview, so that flow does not exist here: signing
//! in is not available in the Android build and the app runs signed out (search, playback,
//! podcasts, audiobooks, playlists and the library all work without an account).

use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::state::AppState;

/// The UI's "Sign in" button. Answers with the same `login-error` event the desktop flow uses, so
/// the sheet shows a message instead of waiting forever.
pub fn open_login(app: AppHandle, _state: Arc<AppState>, _add_account: bool) {
    let _ = app.emit(
        "login-error",
        "Signing in to YouTube is not available in the Android version yet.".to_string(),
    );
}

pub fn cancel_login(_app: &AppHandle) {}

/// Re-minting a rejected session needs the login webview; with none, there is nothing to heal.
pub async fn refresh_session(_app: AppHandle, _state: Arc<AppState>) {}
