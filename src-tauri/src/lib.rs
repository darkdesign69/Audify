//! Spotless for Android: a thin Tauri host over `encore_core` (crates/core). The core owns the
//! transport, player (libmpv), database, queue and every feature; this crate adds the one
//! window, the command boundary the UI calls, and implements the core's `Host` (host.rs).
//!
//! The desktop-only parts of the original app (tray, mini player, global hotkeys, menu bar, the
//! hidden cipher webview, the Google sign-in webview, the self-updater) are gone or stubbed.

// The core's modules, re-exported under their old paths so `crate::state::AppState` and friends
// keep resolving in the command files.
#[allow(unused_imports)]
pub(crate) use encore_core::{
    api, audioproxy, blocked, canonical, cipher, db, firstaudio, history, identity, import, local,
    localuser, lyrics, orchestrator, potoken, reliability_boot, romanize, secrets, spotify, state,
    stats, videoproxy,
};

// Core modules that also carry this crate's Tauri commands (`<module>/commands.rs`).
pub(crate) mod downloads {
    pub use encore_core::downloads::*;
    pub mod commands;
}
pub(crate) mod podcasts {
    pub use encore_core::podcasts::*;
    pub mod commands;
}
pub(crate) mod polish {
    pub use encore_core::polish::*;
    pub mod commands;
}
pub(crate) mod smart {
    pub use encore_core::smart::*;
    pub mod commands;
}
pub(crate) mod http {
    pub use crate::webproxy::*;
    #[allow(unused_imports)]
    pub use encore_core::http::*;
}
pub(crate) mod audio {
    pub use crate::audio_cmds::*;
}

mod audio_cmds;
mod audiobooks;
mod commands;
mod deeplink;
mod diagnostics;
mod host;
mod hotkeys;
mod library;
mod mini;
mod session;
mod sync;
mod tray;
mod webproxy;

use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use state::AppState;

/// Android's logcat as a `tracing` writer, so `adb logcat -s Spotless` shows the app's log.
#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::{c_char, c_int, CString};
    use std::io::{self, Write};

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    pub struct Writer;

    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let text = String::from_utf8_lossy(buf).replace('\0', "");
            let text = text.trim_end();
            if !text.is_empty() {
                if let (Ok(tag), Ok(msg)) = (CString::new("Spotless"), CString::new(text)) {
                    // 4 = ANDROID_LOG_INFO
                    unsafe { __android_log_write(4, tag.as_ptr(), msg.as_ptr()) };
                }
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub struct Make;

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Make {
        type Writer = Writer;
        fn make_writer(&'a self) -> Writer {
            Writer
        }
    }
}

fn init_logging(dir: &std::path::Path) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,app_lib=debug,encore_core=debug".into())
    };
    let path = dir.join("encore.log");
    let _ = std::fs::rename(&path, dir.join("encore.log.1"));
    let file = std::fs::File::create(&path).ok().map(std::sync::Mutex::new);
    let file_layer = file.map(|f| {
        tracing_subscriber::fmt::layer().with_ansi(false).with_writer(f).with_filter(filter())
    });
    #[cfg(target_os = "android")]
    let console = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(logcat::Make)
        .with_filter(filter());
    #[cfg(not(target_os = "android"))]
    let console = tracing_subscriber::fmt::layer().with_filter(filter());
    let _ = tracing_subscriber::registry().with(console).with(file_layer).try_init();
}

fn fatal(what: &str, detail: &str) -> ! {
    tracing::error!("fatal at startup: {what}: {detail}");
    std::process::exit(1)
}

/// Links the app was opened with, handed to the page once (`take_launch_args`). Always empty on
/// Android for now; kept because the command and `deeplink` share it.
pub(crate) static LAUNCH_ARGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // One tokio runtime for the core and Tauri: the core's background loops and Tauri's commands
    // share a pool.
    tauri::async_runtime::set(encore_core::rt::handle());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            let handle = app.handle().clone();

            // App data dir for the SQLite file and mpv's on-disk audio cache (`Host::cache_dir`).
            let data_dir = app.path().app_data_dir().unwrap_or_else(|_| std::env::temp_dir());
            std::fs::create_dir_all(&data_dir).ok();
            init_logging(&data_dir);
            tracing::info!(dir = %data_dir.display(), "Spotless for Android starting");

            let db = match encore_core::boot::open_db(&data_dir) {
                Ok(db) => db,
                Err(e) => fatal(e.title, &e.detail),
            };
            // `LIMUSIC_PROXY` or the stored setting, applied to the shared HTTP client.
            let proxy = encore_core::boot::resolve_proxy(&db);

            // No OS media widget from Rust on Android (the notification comes from the Kotlin
            // side); the core treats `None` as "no media controls".
            let host: Arc<dyn encore_core::Host> =
                Arc::new(host::TauriHost::new(handle.clone(), data_dir.clone()));
            let mut booted = match encore_core::boot::assemble(host, db.clone(), proxy.as_deref(), None) {
                Ok(b) => b,
                Err(e) => fatal(e.title, &e.detail),
            };
            let volume = booted.volume;
            // The level unmute returns to: the current one, or after a muted quit the level saved
            // when it was muted.
            let unmute = if volume > 0 {
                Some(volume)
            } else {
                db.get_setting("volume_before_mute").and_then(|s| s.parse::<i64>().ok())
            };
            if let Some(v) = unmute.filter(|v| (1..=100).contains(v)) {
                hotkeys::LAST_NONZERO_VOLUME.store(v, std::sync::atomic::Ordering::Relaxed);
            }
            let app_state = booted.state.clone();
            app.manage(app_state.clone());
            // Smart, podcasts, polish, downloads, audio loops, the loopback proxies.
            encore_core::boot::start_services(&booted);

            // The settings page still reads the (inert) hotkeys config.
            let hotkeys_cfg = hotkeys::load_config(&app_state.db);
            app.manage(Arc::new(hotkeys::HotkeysManager::new(hotkeys_cfg)));

            // Keep the login alive by itself: persist a rotated cookie, keep the session warm.
            // (Sign-in itself is not available on Android, so these idle for a signed-out user.)
            {
                let st = app_state.clone();
                let app_handle = handle.clone();
                let rejected = st.it.session_rejected();
                let rotated = st.it.cookie_changed();
                tauri::async_runtime::spawn(async move {
                    if st.it.is_logged_in() {
                        if let Some(client) = st.clients.get(innertube::METADATA_CLIENT) {
                            let _ = innertube::without_healing(st.it.account_menu(client)).await;
                        }
                    }
                    let mut keepalive = tokio::time::interval(Duration::from_secs(30 * 60));
                    keepalive.tick().await;
                    loop {
                        tokio::select! {
                            _ = rejected.notified() => {
                                let _heal = st.it.begin_heal();
                                innertube::without_healing(
                                    session::refresh_session(app_handle.clone(), st.clone()),
                                )
                                .await;
                            }
                            _ = rotated.notified() => st.persist_rotated_cookie(),
                            _ = keepalive.tick() => st.keep_session_alive().await,
                        }
                    }
                });
            }

            // Queue restore, visitorData bootstrap, the player event pump, cipher prewarm and
            // idle teardown.
            encore_core::boot::start_playback(&mut booted);
            Ok(())
        })
        .invoke_handler(only_ui(tauri::generate_handler![
            // ── stream: smart ──
            smart::commands::smart_settings,
            smart::commands::smart_set_setting,
            smart::commands::smart_home,
            smart::commands::smart_home_local, // stream: magic-d4
            smart::commands::podcast_home,     // stream: home-tiles-pods
            smart::commands::podcast_category, // stream: home-tiles-pods
            smart::commands::smart_mix,
            smart::commands::playlist_recommendations,
            smart::commands::explicit_flags,
            smart::commands::playable_flags,
            smart::commands::taste_profile,
            // ── end stream: smart ──
            // ── stream: smart2 ──
            smart::commands::made_for_you,
            smart::commands::whats_new,
            smart::commands::whats_new_count,
            smart::commands::whats_new_mark_seen,
            smart::commands::smart_shuffle_state,
            smart::commands::smart_shuffle_set,
            smart::commands::smart_shuffle_remove,
            smart::commands::smart_shuffle_keep,
            // ── end stream: smart2 ──
            // ── lane magic-d3 ──
            commands::smart_tags_liked,
            commands::enhance_state,
            commands::enhance_set,
            commands::enhance_remove,
            commands::enhance_keep,
            // lane magic-d1
            commands::radio_tracks,
            commands::play_single_at,
            commands::song_credits,
            commands::discography, // stream: onecore
            commands::taste_set_excluded,
            commands::taste_excluded,
            // ── stream: audiobooks ──
            audiobooks::audiobook_search,
            audiobooks::audiobook_home,
            audiobooks::audiobook_genre,
            audiobooks::audiobook_page,
            audiobooks::audiobook_for_chapter,
            audiobooks::audiobook_save,
            audiobooks::audiobook_saved,
            // ── stream: podcasts ──
            podcasts::commands::podcast_search,
            podcasts::commands::podcast_top_show, // ── stream: podtop ──
            commands::search_blend_artist_top, // ── stream: podtop ──
            podcasts::commands::podcast_show,
            podcasts::commands::podcast_show_more,
            podcasts::commands::podcast_episode,
            podcasts::commands::podcast_chapters, // ── stream: chapters ──
            podcasts::commands::podcast_episode_video, // ── stream: video-pods ──
            podcasts::commands::podcast_follow,
            podcasts::commands::podcast_unfollow,
            podcasts::commands::podcast_follows,
            podcasts::commands::podcast_add_feed,
            podcasts::commands::podcast_new_episodes,
            podcasts::commands::podcast_continue_listening,
            podcasts::commands::podcast_saved_episodes,
            podcasts::commands::podcast_save_episode,
            podcasts::commands::podcast_mark_played,
            podcasts::commands::podcast_mark_show_played,
            podcasts::commands::podcast_progress,
            podcasts::commands::podcast_refresh,
            podcasts::commands::podcast_opml_export,
            podcasts::commands::podcast_opml_import,
            podcasts::commands::podcast_settings,
            podcasts::commands::podcast_set_settings,
            // ── end stream: podcasts ──
            commands::search,
            commands::search_all,
            commands::search_suggestions,
            commands::search_cards,
            commands::search_overview, // stream: search2
            commands::search_page, // stream: search2
            commands::search_lyrics, // stream: search2
            commands::search_blend_lyrics, // stream: search2
            commands::get_live_now,
            commands::search_videos,
            commands::song,
            commands::play,
            commands::hint_play, // stream: firstaudio
            commands::play_index,
            commands::switch_version,
            commands::remove_from_queue,
            commands::clear_queued,
            commands::add_to_queue_at, // lane magic-d2
            commands::restore_queued, // lane magic-d2
            commands::add_to_queue,
            commands::move_in_queue,
            commands::play_next,
            commands::next_track,
            commands::prev_track,
            commands::back_to_previous,
            commands::toggle_shuffle,
            commands::set_repeat,
            commands::toggle_pause,
            commands::seek,
            commands::playback_clock,
            commands::set_volume,
            commands::set_playback_params,
            // ── stream: audio ──
            audio::audio_settings,
            audio::audio_eq_info,
            audio::set_eq,
            audio::set_eq_preset,
            audio::set_normalize_volume,
            audio::set_volume_level,
            audio::set_crossfade_secs,
            audio::audio_devices,
            audio::set_audio_device,
            audio::set_sleep_timer,
            audio::cancel_sleep_timer,
            audio::sleep_timer,
            // ── end stream: audio ──
            commands::get_queue,
            commands::get_playback,
            commands::video_stream,
            commands::forget_video_stream,
            commands::get_settings,
            commands::set_setting,
            commands::get_global_hotkeys,
            commands::global_hotkeys_on_wayland,
            commands::set_global_hotkeys,
            commands::reset_global_hotkeys,
            commands::get_stream_clients,
            commands::clear_caches,
            commands::cache_size, // stream: p-settings
            commands::reveal_in_finder, // stream: p-settings
            commands::save_text_file, // stream: p-settings
            commands::local_user_name, // stream: parity3
            commands::get_account,
            commands::get_account_identities,
            commands::switch_account,
            commands::sign_out,
            commands::login_webview,
            commands::cancel_login,
            commands::get_google_accounts,
            commands::switch_google_account,
            commands::remove_google_account,
            commands::open_mini,
            commands::close_mini,
            commands::set_mini_compact,
            commands::show_main,
            commands::take_launch_args,
            commands::get_home,
            commands::network_reachable,
            commands::get_home_more,
            commands::get_library,
            commands::get_library_albums,
            commands::get_library_artists,
            commands::get_library_subscriptions,
            commands::get_history,
            commands::get_playlist,
            commands::get_playlist_more,
            commands::get_playlist_suggestions,
            commands::playlist_index,
            commands::sync_playlist_index,
            commands::play_counts,
            commands::listening_stats,
            commands::top_artists,
            commands::clear_listening_history,
            commands::get_album,
            commands::get_blocked_artists,
            commands::block_artist,
            commands::unblock_artist,
            commands::get_local_library,
            commands::add_local_folder,
            commands::remove_local_folder,
            commands::get_artist,
            commands::get_artist_appears_on,
            commands::get_browse_grid,
            commands::get_moods,
            commands::get_mood_art,
            commands::play_playlist,
            commands::start_radio,
            commands::rate,
            commands::set_song_saved,
            commands::set_album_saved,
            commands::add_to_playlist,
            commands::readonly_playlists,
            commands::add_to_local_playlist,
            commands::local_playlists,
            commands::remove_from_playlist,
            commands::remove_many_from_playlist,
            commands::move_in_playlist,
            commands::create_playlist,
            commands::edit_playlist_details,
            commands::set_playlist_cover,
            commands::set_playlist_sort,
            commands::delete_playlist,
            commands::subscribe,
            commands::import_read,
            commands::import_read_file,
            commands::import_start,
            commands::import_status,
            commands::import_offer,
            commands::import_offer_done,
            commands::import_rows,
            commands::import_pick,
            commands::import_create,
            commands::import_cancel,
            commands::import_source,
            commands::import_update,
            commands::import_resolve,
            commands::get_lyrics,
            commands::choose_lyrics_source,
            commands::set_lyrics_offset,
            commands::lyrics_providers,
            commands::lyrics_parse,
            commands::theater_fullscreen,
            commands::title_bar_double_click,
            commands::can_self_update,
            commands::open_external,
            commands::quit_app,
            commands::diagnostics,
            commands::diagnostics_summary,
            commands::save_diagnostics,
            commands::log_ui,
            // ── device sync (docs/SYNC.md) ──
            sync::sync_status,
            sync::sync_start_pairing,
            sync::sync_cancel_pairing,
            sync::sync_join,
            sync::sync_unpair,
            sync::sync_now,
            sync::sync_set_active,
            // ── stream: library ──
            library::lib_snapshot,
            library::lib_liked_ids,
            library::lib_liked_songs,
            library::lib_like,
            library::lib_unlike,
            library::lib_restore_likes,
            library::lib_saved,
            library::lib_save,
            library::lib_unsave,
            library::lib_restore_saved,
            library::lib_mark_synced,
            library::lib_playlists,
            library::lib_playlist_create,
            library::lib_playlist_update,
            library::lib_playlist_delete,
            library::lib_playlist_restore,
            library::lib_deleted_playlists, // stream: magic-d4
            library::lib_playlist_entries,
            library::lib_playlist_duplicates,
            library::lib_playlist_add,
            library::lib_playlist_remove,
            library::lib_playlist_insert_entries,
            library::lib_playlist_move,
            library::lib_playlist_set_order,
            library::lib_folders,
            library::lib_folder_create,
            library::lib_folder_rename,
            library::lib_folder_move,
            library::lib_folder_delete,
            library::lib_folder_restore,
            library::lib_folder_set_item,
            library::lib_pins,
            library::lib_pin,
            library::lib_unpin,
            library::lib_pins_reorder,
            library::lib_note_played,
            library::lib_recent_contexts,
            library::lib_recent_tracks,
            library::lib_last_played,
            library::lib_hidden_songs,
            library::lib_hide_song,
            library::lib_unhide_song,
            library::lib_private_session,
            library::lib_set_private_session,
            library::lib_migrate_legacy,
            library::lib_export,
            // ── end stream: library ──
            // ── stream: polish ──
            polish::commands::get_charts,
            polish::commands::get_new_releases,
            polish::commands::get_browse_sections,
            polish::commands::playback_skip_reasons,
            polish::commands::playback_health,
            polish::commands::recent_play_contexts,
            polish::commands::podcast_youtube_twin,
            polish::commands::podcast_youtube_match,
            polish::commands::podcast_set_youtube_match,
            // ── end stream: polish ──
            // ── stream: downloads ──
            downloads::commands::downloads_snapshot,
            downloads::commands::downloads_add,
            downloads::commands::downloads_remove,
            downloads::commands::downloads_remove_all,
            downloads::commands::downloads_retry,
            downloads::commands::downloads_sync,
            downloads::commands::downloads_set_settings,
            downloads::commands::downloads_set_dir,
            downloads::commands::downloads_songs,
            // ── end stream: downloads ──
        ]))
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|handle, event| {
            // Saves the play position on the way out.
            if let tauri::RunEvent::Exit = &event {
                if let Some(state) = handle.try_state::<Arc<AppState>>() {
                    state.flush_position();
                }
            }
        });
}

/// Let app commands answer only the UI's own window. Tauri checks capabilities for plugin
/// commands, but without an app ACL manifest an app command is open to any webview on a local
/// origin.
fn only_ui<R: tauri::Runtime>(
    handler: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if matches!(invoke.message.webview_ref().label(), "main" | mini::LABEL) {
            return handler(invoke);
        }
        invoke.resolver.reject("not allowed from this webview");
        true
    }
}
