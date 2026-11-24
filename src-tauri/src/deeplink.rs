//! Links opened from outside the app: the `spotless://` scheme and its older alias `encore://`
//! (both registered in Info.plist through `plugins.deep-link` in tauri.conf.json), and https links
//! from YouTube Music, YouTube and Spotify handed to the app (`open -a Spotless <url>`, Finder "Open With", a second launch with
//! the link in argv).
//!
//! Every one ends up where Limusic's `limusic-app <link>` already went (#348): the raw https URL
//! as an `open-link` event for the page, or in `LAUNCH_ARGS` before the page has mounted. The page
//! has the one parser (`ytlink.ts`, `isSpotifyLink`), so this only turns `spotless://` forms into
//! the https link they stand for.
//!
//! `spotless://` forms (`encore://` takes the same ones; it was the app's working name and stays
//! registered so older links keep opening):
//!
//! | link | opens |
//! |---|---|
//! | `encore://track/<videoId>` (also `song`, `watch`) | `https://music.youtube.com/watch?v=<id>` |
//! | `encore://playlist/<id>` | `https://music.youtube.com/playlist?list=<id without VL>` |
//! | `encore://album/<browseId>` | `https://music.youtube.com/browse/<id>` |
//! | `encore://artist/<channelId>` | `https://music.youtube.com/channel/<id>` |
//! | `encore://open?url=<https url>` | that URL |
//! | `spotless://pair?v=1&code=…` | itself: the page opens the device-linking dialog with it (docs/SYNC.md) |
//! | `encore://music.youtube.com/…`, `encore://open.spotify.com/…` | the same path on https |

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter};

/// Set once the page has mounted (`show_main`), after which it listens for `open-link` instead
/// of taking `LAUNCH_ARGS`.
pub(crate) static UI_READY: AtomicBool = AtomicBool::new(false);

/// Hosts whose https links the page knows how to open.
const KNOWN_HOSTS: [&str; 6] = [
    "music.youtube.com",
    "www.youtube.com",
    "youtube.com",
    "m.youtube.com",
    "youtu.be",
    "open.spotify.com",
];

/// The https link an opened URL stands for, or `None` for one the app has no use for.
pub fn to_web_link(raw: &str) -> Option<String> {
    let url = tauri::Url::parse(raw.trim()).ok()?;
    match url.scheme() {
        "http" | "https" => {
            let host = url.host_str()?;
            KNOWN_HOSTS.contains(&host).then(|| url.to_string())
        }
        s if is_app_scheme(s) => app_link(&url),
        _ => None,
    }
}

/// The app's own URL schemes: `spotless`, and `encore` from before the rename.
pub const APP_SCHEMES: [&str; 2] = ["spotless", "encore"];

pub fn is_app_scheme(scheme: &str) -> bool {
    APP_SCHEMES.iter().any(|s| scheme.eq_ignore_ascii_case(s))
}

/// Whether a launch argument is one of the app's own links (`spotless:…` / `encore:…`).
pub fn is_app_link(raw: &str) -> bool {
    raw.split_once(':').is_some_and(|(s, _)| is_app_scheme(s))
}

fn app_link(url: &tauri::Url) -> Option<String> {
    let host = url.host_str().unwrap_or_default();
    let mut segs = url.path_segments().into_iter().flatten().filter(|s| !s.is_empty());
    let first = segs.next();
    let id_ok = |id: &str| {
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
    };
    let id = first.filter(|id| id_ok(id));
    match host {
        "track" | "song" | "watch" => Some(format!("https://music.youtube.com/watch?v={}", id?)),
        "playlist" => {
            let id = id?;
            Some(format!(
                "https://music.youtube.com/playlist?list={}",
                id.strip_prefix("VL").unwrap_or(id)
            ))
        }
        "album" => Some(format!("https://music.youtube.com/browse/{}", id?)),
        // A device-linking QR: the page joins with it as is (`sync_join { qr }`).
        "pair" => Some(format!("spotless://pair?{}", url.query()?)),
        "artist" => Some(format!("https://music.youtube.com/channel/{}", id?)),
        "open" => {
            let target =
                url.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.into_owned())?;
            let target = to_web_link(&target)?;
            target.starts_with("http").then_some(target)
        }
        h if KNOWN_HOSTS.contains(&h) => {
            // `set_scheme` refuses moving a non-special scheme to a special one: rebuild.
            let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
            let web = tauri::Url::parse(&format!("https://{h}{}{query}", url.path())).ok()?;
            Some(web.to_string())
        }
        _ => None,
    }
}

/// Hand opened URLs to the page, bringing the window forward.
pub fn route(app: &AppHandle, urls: impl IntoIterator<Item = String>) {
    let links: Vec<String> = urls.into_iter().filter_map(|u| to_web_link(&u)).collect();
    if links.is_empty() {
        return;
    }
    tracing::info!(count = links.len(), "opening link from outside the app");
    if UI_READY.load(Ordering::SeqCst) {
        crate::tray::show_main(app);
        let _ = app.emit_to("main", "open-link", &links);
    } else {
        crate::LAUNCH_ARGS.lock().unwrap().extend(links);
    }
}

#[cfg(test)]
mod tests {
    use super::{is_app_link, to_web_link};

    #[test]
    fn spotless_links_open_the_same_as_the_encore_alias() {
        for tail in [
            "track/dQw4w9WgXcQ",
            "playlist/VLPL123",
            "album/MPREb_x",
            "artist/UCabc",
            "open?url=https%3A%2F%2Fopen.spotify.com%2Fplaylist%2Fabc",
            "music.youtube.com/watch?v=abc",
        ] {
            let new = to_web_link(&format!("spotless://{tail}"));
            assert!(new.is_some(), "spotless://{tail}");
            assert_eq!(new, to_web_link(&format!("encore://{tail}")), "{tail}");
        }
        assert_eq!(
            to_web_link("spotless://track/dQw4w9WgXcQ").as_deref(),
            Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            to_web_link("SPOTLESS://track/abc").as_deref(),
            Some("https://music.youtube.com/watch?v=abc")
        );
        assert_eq!(to_web_link("spotless://nonsense/x"), None);
        assert_eq!(
            to_web_link("encore://pair?v=1&code=123456&addr=10.0.0.2%3A47615").as_deref(),
            Some("spotless://pair?v=1&code=123456&addr=10.0.0.2%3A47615")
        );
        assert!(is_app_link("spotless://track/x") && is_app_link("encore://track/x"));
        assert!(!is_app_link("https://music.youtube.com/watch?v=x") && !is_app_link("--autostart"));
    }

    #[test]
    fn encore_links_become_the_web_link_they_stand_for() {
        let l = |s: &str| to_web_link(s);
        assert_eq!(
            l("encore://track/dQw4w9WgXcQ").as_deref(),
            Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            l("encore://song/dQw4w9WgXcQ/").as_deref(),
            Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            l("encore://playlist/VLPL123").as_deref(),
            Some("https://music.youtube.com/playlist?list=PL123")
        );
        assert_eq!(
            l("encore://album/MPREb_x").as_deref(),
            Some("https://music.youtube.com/browse/MPREb_x")
        );
        assert_eq!(
            l("encore://artist/UCabc").as_deref(),
            Some("https://music.youtube.com/channel/UCabc")
        );
        assert_eq!(
            l("encore://open?url=https%3A%2F%2Fopen.spotify.com%2Fplaylist%2Fabc").as_deref(),
            Some("https://open.spotify.com/playlist/abc")
        );
        assert_eq!(
            l("encore://music.youtube.com/watch?v=abc").as_deref(),
            Some("https://music.youtube.com/watch?v=abc")
        );
        assert_eq!(l("encore://track/a%22b"), None, "ids are [A-Za-z0-9_-] only");
        assert_eq!(l("encore://track"), None);
        assert_eq!(l("encore://open?url=file%3A%2F%2F%2Fetc%2Fpasswd"), None);
        assert_eq!(l("encore://nonsense/x"), None);
    }

    #[test]
    fn only_known_https_hosts_pass_through() {
        assert_eq!(
            to_web_link("https://music.youtube.com/watch?v=abc").as_deref(),
            Some("https://music.youtube.com/watch?v=abc")
        );
        assert!(to_web_link("https://youtu.be/abc").is_some());
        assert!(to_web_link("https://open.spotify.com/track/1").is_some());
        assert_eq!(to_web_link("https://evil.example/watch?v=abc"), None);
        assert_eq!(to_web_link("file:///etc/passwd"), None);
        assert_eq!(to_web_link("--autostart"), None);
    }
}
