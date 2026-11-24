//! Live reliability soak: ~300 mixed anonymous catalog calls (search, next/radio, browse
//! artist/album/playlist) at the app's real fan-out, counting every failure by endpoint and
//! status. NOT part of the normal run (it hits YouTube Music):
//!
//! ```sh
//! cargo test -p innertube --test soak -- --ignored --nocapture
//! ```
//!
//! Knobs: `SOAK_CONC` (parallel chains, default 16: what Made For You fans out to at startup),
//! `SOAK_QUERIES` (chains, default 48; ≈6 calls each), `SOAK_FRESH=1` (a brand-new install:
//! no visitorData until the transport bootstraps one).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::StreamExt;
use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};

const QUERIES: &[&str] = &[
    "the weeknd blinding lights",
    "taylor swift",
    "daft punk get lucky",
    "radiohead",
    "kendrick lamar",
    "billie eilish",
    "dua lipa levitating",
    "arctic monkeys",
    "beyonce",
    "fleetwood mac dreams",
    "drake",
    "adele hello",
    "coldplay yellow",
    "bad bunny",
    "eminem lose yourself",
    "queen bohemian rhapsody",
    "the beatles",
    "frank ocean",
    "lana del rey",
    "post malone",
    "rosalia",
    "bts dynamite",
    "harry styles",
    "sza kill bill",
    "metallica",
    "nirvana",
    "miles davis",
    "bach cello suite",
    "stromae",
    "tame impala",
    "olivia rodrigo",
    "ed sheeran",
    "rihanna",
    "michael jackson",
    "the strokes",
    "lorde",
    "kanye west",
    "doja cat",
    "pink floyd",
    "bon iver",
    "travis scott",
    "ariana grande",
    "phoebe bridgers",
    "led zeppelin",
    "bruno mars",
    "mac miller",
    "the cure",
    "abba",
    "massive attack",
    "aphex twin",
    "burna boy",
    "karol g",
    "joji",
    "gorillaz",
];

/// Generators run as background work.
async fn with_priority<F: std::future::Future>(f: F) -> F::Output {
    innertube::background(f).await
}
fn report_stats(it: &InnerTube) {
    eprintln!("  transport: {:?}", it.reliability_stats());
}

#[derive(Default)]
struct Tally {
    ok: BTreeMap<&'static str, u32>,
    fail: BTreeMap<(&'static str, String), u32>,
    /// Seconds per call, retries included: what the caller waited.
    lat: Vec<f64>,
}

fn pct(v: &[f64], p: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0)
}

impl Tally {
    fn record<T, E: std::fmt::Display>(&mut self, ep: &'static str, r: &Result<T, E>) {
        match r {
            Ok(_) => *self.ok.entry(ep).or_default() += 1,
            Err(e) => {
                let mut msg = e.to_string();
                // Keep the status, drop the URL (it differs per call).
                if let Some(i) = msg.find(" for url") {
                    msg.truncate(i);
                }
                *self.fail.entry((ep, msg)).or_default() += 1;
            }
        }
    }
}

async fn chain(it: InnerTube, clients: Arc<Clients>, q: &'static str, tally: Arc<Mutex<Tally>>) {
    let c = clients.get(METADATA_CLIENT).unwrap();
    macro_rules! call {
        ($ep:expr, $f:expr) => {{
            let t0 = Instant::now();
            let r = $f.await;
            let mut t = tally.lock().unwrap();
            t.lat.push(t0.elapsed().as_secs_f64());
            t.record($ep, &r);
            drop(t);
            r.ok()
        }};
    }
    let songs = call!("search_songs", it.search_songs(c, q, false));
    let all = call!("search_all", it.search_all(c, q, false));
    let video = songs.as_ref().and_then(|s| s.items.first()).map(|s| s.video_id.clone());
    if let Some(v) = &video {
        let radio = call!("next", it.next(c, Some(v), Some(&format!("RDAMVM{v}"))));
        // The app's second radio call (canonical re-seed on another row).
        if let Some(v2) = radio.as_ref().and_then(|r| r.items.get(3)).map(|s| s.video_id.clone()) {
            call!("next", it.next(c, Some(&v2), Some(&format!("RDAMVM{v2}"))));
        }
    }
    let Some(all) = all else { return };
    let artist = all.artists.first().or(all.top.iter().find(|t| t.kind == "artist"));
    if let Some(a) = artist.map(|a| a.id.clone()) {
        let page = call!("artist", it.artist(c, &a));
        if let Some(pl) = page.and_then(|p| p.top_songs_id) {
            call!("playlist", it.playlist(c, &pl, None));
        }
    }
    if let Some(al) = all.albums.first().map(|a| a.id.clone()) {
        call!("album", it.album(c, &al));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "hits live YouTube Music"]
async fn live_soak() {
    let conc: usize = std::env::var("SOAK_CONC").ok().and_then(|v| v.parse().ok()).unwrap_or(16);
    let n: usize = std::env::var("SOAK_QUERIES").ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    let fresh = std::env::var("SOAK_FRESH").is_ok_and(|v| v == "1");
    let it = InnerTube::new(Session::default(), None).unwrap();
    if !fresh {
        // The app's start state on any launch after the first: a stored visitorData.
        let vd = it.fetch_visitor_data().await.ok();
        it.set_visitor_data(vd);
    }
    let clients = Arc::new(Clients::bundled());
    let tally = Arc::new(Mutex::new(Tally::default()));
    let start = Instant::now();
    // Like the app: generators (Made For You, home) run in the background while the user clicks
    // around. Every fourth chain is a user action; the rest are background work.
    let fg_tally = Arc::new(Mutex::new(Tally::default()));
    futures_util::stream::iter(QUERIES.iter().cycle().take(n).enumerate())
        .for_each_concurrent(conc, |(i, q)| {
            let (it, clients, tally, fg_tally) =
                (it.clone(), clients.clone(), tally.clone(), fg_tally.clone());
            async move {
                if i % 4 == 0 {
                    let mine = Arc::new(Mutex::new(Tally::default()));
                    chain(it, clients, q, mine.clone()).await;
                    let mine = std::mem::take(&mut *mine.lock().unwrap());
                    let (mut t, mut f) = (tally.lock().unwrap(), fg_tally.lock().unwrap());
                    for (k, v) in mine.ok {
                        *t.ok.entry(k).or_default() += v;
                        *f.ok.entry(k).or_default() += v;
                    }
                    for (k, v) in mine.fail {
                        *t.fail.entry(k.clone()).or_default() += v;
                        *f.fail.entry(k).or_default() += v;
                    }
                    t.lat.extend(&mine.lat);
                    f.lat.extend(&mine.lat);
                } else {
                    with_priority(chain(it, clients, q, tally)).await;
                }
            }
        })
        .await;
    let fg = fg_tally.lock().unwrap();
    let fg_fail: u32 = fg.fail.values().sum();
    let fg_total = fg_fail + fg.ok.values().sum::<u32>();
    let t = tally.lock().unwrap();
    let ok: u32 = t.ok.values().sum();
    let fail: u32 = t.fail.values().sum();
    let total = ok + fail;
    eprintln!("\n== soak: {total} calls, conc {conc}, {:.1}s ==", start.elapsed().as_secs_f64());
    for (ep, n) in &t.ok {
        let f: u32 = t.fail.iter().filter(|((e, _), _)| e == ep).map(|(_, n)| n).sum();
        eprintln!("  {ep:<13} ok {n:>4}  fail {f:>3}");
    }
    for ((ep, msg), n) in &t.fail {
        eprintln!("  FAIL {ep}: {msg} ×{n}");
    }
    let rate = ok as f64 / total.max(1) as f64 * 100.0;
    eprintln!("  success {rate:.2}%  (user actions: {fg_fail} failed of {fg_total})");
    eprintln!(
        "  latency all p50 {:.2}s p95 {:.2}s max {:.2}s | user actions p50 {:.2}s p95 {:.2}s max {:.2}s",
        pct(&t.lat, 0.5), pct(&t.lat, 0.95), pct(&t.lat, 1.0),
        pct(&fg.lat, 0.5), pct(&fg.lat, 0.95), pct(&fg.lat, 1.0)
    );
    report_stats(&it);
    eprintln!(
        "SOAK_RESULT total={total} ok={ok} fail={fail} rate={rate:.2} fg_fail={fg_fail}/{fg_total}"
    );
    let min: f64 = std::env::var("SOAK_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    assert!(rate >= min, "success {rate:.2}% below SOAK_MIN {min}");
}
