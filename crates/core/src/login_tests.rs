//! The sign-in capture end to end, with no Google account: a fake cookie jar stands in for the
//! webview and a local server stands in for YouTube. Runs the real core (mpv included), like
//! `instant_tests`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{capture, End};
use crate::host::Host;
use crate::state::AppState;

struct Recorder {
    dir: PathBuf,
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl Host for Recorder {
    fn emit_json(&self, event: &str, json: String) {
        let v = serde_json::from_str(&json).unwrap_or(serde_json::Value::Null);
        self.events.lock().unwrap().push((event.to_owned(), v));
    }
    fn data_dir(&self) -> PathBuf {
        self.dir.clone()
    }
}

impl Recorder {
    fn names(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }
    fn payload(&self, name: &str) -> Option<serde_json::Value> {
        self.events.lock().unwrap().iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }
}

fn fixture(name: &str) -> String {
    let path = format!("{}/../innertube/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path).unwrap()
}

/// A stand-in for YouTube's account endpoints. `signed_in` false answers like a cookie that
/// doesn't authenticate (no account header).
fn fake_youtube(signed_in: bool) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/youtubei/v1/", listener.local_addr().unwrap());
    let (menu, list) = if signed_in {
        (fixture("account_menu_single.json"), fixture("accounts_list_one.json"))
    } else {
        ("{}".to_owned(), "{}".to_owned())
    };
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let mut len = 0;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = h.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let _ = reader.read_exact(&mut vec![0; len]);
            let body = if line.contains("account/accounts_list") { &list } else { &menu };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    base
}

/// A fresh core in its own data dir, talking to [`fake_youtube`]. `None` without libmpv.
fn boot(name: &str, signed_in: bool) -> Option<(Arc<AppState>, Arc<Recorder>)> {
    let dir = std::env::temp_dir().join(format!("spotless-login-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rec = Arc::new(Recorder { dir: dir.clone(), events: Mutex::default() });
    let db = crate::boot::open_db(&dir).unwrap();
    let Ok(booted) = crate::boot::assemble(rec.clone(), db, None, None) else {
        eprintln!("no libmpv here; skipping");
        return None;
    };
    booted.state.it.set_base_url(&fake_youtube(signed_in));
    Some((booted.state.clone(), rec))
}

const MS: Duration = Duration::from_millis(10);

/// A jar that is empty for the first `empty` reads (Google hasn't set the cookies yet), then holds
/// a YouTube session.
fn late_jar(empty: usize) -> impl FnMut() -> std::future::Ready<Option<String>> {
    let mut reads = 0;
    move || {
        reads += 1;
        let jar =
            if reads > empty { "SAPISID=fake-sapisid; SID=fake-sid; PREF=x" } else { "PREF=x" };
        std::future::ready(Some(jar.to_owned()))
    }
}

#[test]
fn cookies_that_arrive_late_still_sign_spotless_in() {
    let Some((st, rec)) = boot("late", true) else { return };
    let (_nudge_tx, mut nudge) = tokio::sync::mpsc::unbounded_channel();
    crate::rt::handle().block_on(async {
        // No page load ever announces them: only the timer finds them.
        let end = capture(&st, late_jar(5), &mut nudge, MS, Duration::from_secs(5)).await;
        assert_eq!(end, End::SignedIn);
    });
    // Spotless itself is signed in, not just the web page.
    assert!(st.it.is_logged_in());
    let accounts = st.db.list_accounts();
    assert_eq!(accounts.len(), 1, "the account row is saved");
    assert!(accounts[0].session_cookie.contains("SAPISID=fake-sapisid"));
    let changed = rec.payload("auth-changed").expect("auth-changed emitted");
    assert_eq!(changed["signedIn"], true);
    assert_eq!(changed["name"], "Personal channel");
    let names = rec.names();
    assert!(names.contains(&"login-done".to_owned()), "{names:?}");
    assert!(!names.contains(&"login-error".to_owned()), "{names:?}");
}

#[test]
fn a_session_youtube_turns_down_is_an_error_not_a_silent_stall() {
    let Some((st, rec)) = boot("refused", false) else { return };
    let (_nudge_tx, mut nudge) = tokio::sync::mpsc::unbounded_channel();
    crate::rt::handle().block_on(async {
        let end = capture(&st, late_jar(0), &mut nudge, MS, Duration::from_secs(5)).await;
        assert_eq!(end, End::Failed);
    });
    assert!(!st.it.is_logged_in(), "rolled back");
    assert!(st.db.list_accounts().is_empty());
    assert!(rec.payload("login-error").is_some(), "{:?}", rec.names());
    assert!(!rec.names().contains(&"login-done".to_owned()));
}

#[test]
fn closing_the_window_cancels_and_a_jar_that_never_fills_times_out() {
    let Some((st, rec)) = boot("cancel", true) else { return };
    let (_nudge_tx, mut nudge) = tokio::sync::mpsc::unbounded_channel();
    crate::rt::handle().block_on(async {
        let mut reads = 0;
        let closed_after_three = || {
            reads += 1;
            std::future::ready((reads <= 3).then(String::new))
        };
        let end = capture(&st, closed_after_three, &mut nudge, MS, Duration::from_secs(5)).await;
        assert_eq!(end, End::Cancelled);
        assert_eq!(rec.names(), ["login-cancelled"]);

        let never = || std::future::ready(Some("PREF=x".to_owned()));
        let end = capture(&st, never, &mut nudge, MS, Duration::from_millis(50)).await;
        assert_eq!(end, End::Failed);
        assert!(rec.payload("login-error").is_some());
    });
    assert!(!st.it.is_logged_in());
    assert!(st.db.list_accounts().is_empty());
}

#[test]
fn a_page_load_checks_at_once_instead_of_waiting_for_the_tick() {
    let Some((st, _rec)) = boot("nudge", true) else { return };
    let (nudge_tx, mut nudge) = tokio::sync::mpsc::unbounded_channel();
    crate::rt::handle().block_on(async {
        // A tick far longer than the test: only the nudge can get the second read in.
        nudge_tx.send(()).unwrap();
        let end = tokio::time::timeout(
            Duration::from_secs(10),
            capture(
                &st,
                late_jar(1),
                &mut nudge,
                Duration::from_secs(600),
                Duration::from_secs(900),
            ),
        )
        .await;
        assert_eq!(end, Ok(End::SignedIn));
    });
}
