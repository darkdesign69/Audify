//! Encore's shared core: everything the desktop (Tauri) app and the iOS app have in common.
//!
//! Tauri-free. A host (the Tauri adapter in `src-tauri`, or the UniFFI layer in `crates/ffi`)
//! implements [`host::Host`] for events, paths, the cipher's JavaScript engine, the secrets store
//! and the network monitor, builds an [`state::AppState`] through [`boot`], and forwards its UI
//! calls to it. See `docs/streams/ios-core.md`.

pub mod host;
pub mod js;
pub mod rt;

pub mod api;
pub mod audio;
pub mod audioproxy;
pub mod blocked;
pub mod boot;
pub mod autoplay;
pub mod canonical;
pub mod cipher;
pub mod db;
pub mod downloads;
pub mod firstaudio;
#[cfg(test)]
mod firstaudio_bench; // stream: firstaudio (live, #[ignore])
pub mod history;
pub mod http;
pub mod identity;
pub mod import;
pub mod library;
pub mod local;
pub mod loopback;
pub mod localuser;
pub mod login;
pub mod lyrics;
pub mod lyricsearch;
pub mod orchestrator;
pub mod podcasts;
pub mod audiobooks; // stream: audiobooks
pub mod polish;
pub mod radio_page; // lane magic-d1
pub mod credits; // lane magic-d1
pub mod discography; // stream: onecore
pub mod potoken;
pub mod reliability_boot;
pub mod romanize;
pub mod search; // stream: search2
#[cfg(test)]
mod search2_bench; // stream: search2
pub mod secrets;
pub mod smart;
pub mod spotify;
pub mod speeds;
pub mod state;
#[cfg(test)]
mod instant_tests; // stream: instant
pub mod stats;
pub mod sync; // device sync (docs/SYNC.md)
pub mod videoproxy;

#[cfg(test)]
mod browse_tiles;
#[cfg(test)]
mod devmock_capture;

pub use host::{Host, HostExt, MediaControls};
pub use js::{JsContext, JsEngine};
