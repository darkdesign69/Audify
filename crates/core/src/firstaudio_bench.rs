//! stream: firstaudio. Live latency bench for the two waits a user feels most: a tap's stream
//! resolve and a search. `#[ignore]`: it talks to YouTube.
//!
//! ```sh
//! export LIBRARY_PATH=/opt/homebrew/lib
//! # the iPhone's chain (no BotGuard, VISIONOS first):
//! cargo test -p encore-core --no-default-features --lib firstaudio_bench -- --ignored --nocapture
//! ```
//!
//! Per track it times the pieces of the resolve one by one (WEB_REMIX `/player`, VISIONOS
//! `/player`, the stream probe), then the real [`Orchestrator::resolve`] end to end, so a change
//! to the resolve shows as the gap between the sum of the parts and the whole. `FA_BENCH_IDS`
//! (comma-separated videoIds) and `FA_BENCH_QUERIES` override the defaults. Numbers:
//! docs/bench/firstaudio.md.

use std::sync::Arc;
use std::time::{Duration, Instant};

use innertube::{AudioQuality, Clients, InnerTube, Session, MAIN_CLIENT, METADATA_CLIENT};

use crate::cipher::PlayerConfigStore;
use crate::cipher::CipherDeobfuscator;
use crate::orchestrator::Orchestrator;
use crate::potoken::PoTokenGenerator;

const IDS: &[&str] = &[
    "fJ9rUzIMcZQ", // Queen, Bohemian Rhapsody (video)
    "4NRXx6U8ABQ", // The Weeknd, Blinding Lights
    "kJQP7kiw5Fk", // Despacito
    "JGwWNGJdvx8", // Shape of You
    "lp-EO5I60KA", // Thinking Out Loud
    "60ItHLz5WEA", // Faded
    "hT_nvWreIhg", // Counting Stars
    "YQHsXMglC9A", // Hello
    "RgKAFK5djSk", // See You Again
    "OPf0YbXqDm0", // Uptown Funk
    "CevxZvSJLk8", // Roar
    "09R8_2nJtjg", // Sugar
];
const QUERIES: &[&str] =
    &["daft punk", "taylor swift anti hero", "lofi beats", "bohemian rhapsody", "billie eilish"];

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn summary(name: &str, xs: &mut [f64]) {
    if xs.is_empty() {
        println!("{name:<28} n=0");
        return;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = xs[xs.len() / 2];
    println!(
        "{name:<28} median {med:>6.0} ms  min {:>6.0}  max {:>6.0}  (n={})",
        xs[0],
        xs[xs.len() - 1],
        xs.len()
    );
}

fn list(var: &str, default: &[&str]) -> Vec<String> {
    match std::env::var(var) {
        Ok(v) => v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        Err(_) => default.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_tap_to_audio_and_search() {
    let dir = std::env::temp_dir().join(format!("fa-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = Instant::now();
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    println!("visitorData bootstrap            {:>6.0} ms", ms(t));
    let it = InnerTube::new(Session { visitor_data: vd.clone(), ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    #[cfg(target_vendor = "apple")]
    let js: Option<Arc<dyn crate::js::JsEngine>> = Some(Arc::new(crate::js::InProcEngine::jsc()));
    #[cfg(not(target_vendor = "apple"))]
    let js: Option<Arc<dyn crate::js::JsEngine>> = None;
    let cipher = Arc::new(CipherDeobfuscator::new(js, &dir, Arc::new(PlayerConfigStore::new(&dir))));
    let db = Arc::new(crate::db::Db::open(&dir.join("bench.sqlite")).unwrap());
    let potoken = Arc::new(PoTokenGenerator::new(db));
    let orch = Orchestrator::new(it.clone(), clients.clone(), cipher.clone(), potoken.clone());

    let t = Instant::now();
    cipher.prewarm().await;
    println!("cipher prewarm (player.js)       {:>6.0} ms", ms(t));
    if let Some(vd) = &vd {
        let t = Instant::now();
        let pot = potoken.get_session_po_token(vd).await;
        println!("session PoToken                  {:>6.0} ms  (token: {})", ms(t), pot.is_some());
    }
    let sts = cipher.signature_timestamp().await;
    let main = clients.get(MAIN_CLIENT).unwrap();
    let vis = clients.get("VISIONOS").unwrap();

    let (mut wr, mut vi, mut probe, mut full) = (vec![], vec![], vec![], vec![]);
    let mut by_client = std::collections::BTreeMap::<String, usize>::new();
    for id in list("FA_BENCH_IDS", IDS) {
        // The real resolve first, while nothing about this track is warm.
        let t = Instant::now();
        match orch.resolve(&id, false, AudioQuality::High, &Default::default()).await {
            Ok(d) => {
                full.push(ms(t));
                *by_client.entry(d.stream_client).or_default() += 1;
            }
            Err(e) => println!("{id}: resolve failed: {e}"),
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        let t = Instant::now();
        let _ = it.player(main, &id, None, sts, None).await;
        wr.push(ms(t));
        let t = Instant::now();
        let r = it.player(vis, &id, None, None, None).await;
        vi.push(ms(t));
        let url = r.ok().and_then(|r| {
            let f = innertube::find_format(r.streaming_data.as_ref()?, AudioQuality::High)?;
            Some((f.url.clone()?, f.content_length.clone()))
        });
        if let Some((url, len)) = url {
            let len: u64 = len.and_then(|l| l.parse().ok()).unwrap_or(1024);
            let t = Instant::now();
            let _ = crate::http::client()
                .get(&url)
                .header("Range", format!("bytes={}-{}", len.saturating_sub(256), len - 1))
                .header("User-Agent", vis.user_agent.clone())
                .timeout(Duration::from_secs(10))
                .send()
                .await;
            probe.push(ms(t));
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    println!("-- stream resolve, per track --");
    summary("WEB_REMIX /player", &mut wr);
    summary("VISIONOS /player", &mut vi);
    summary("probe (conn. reused)", &mut probe);
    summary("Orchestrator::resolve", &mut full);
    println!("stream clients: {by_client:?}");

    let meta = clients.get(METADATA_CLIENT).unwrap();
    let mut search = vec![];
    for q in list("FA_BENCH_QUERIES", QUERIES) {
        let t = Instant::now();
        if it.search_all(meta, &q, false).await.is_ok() {
            search.push(ms(t));
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    println!("-- search --");
    summary("search_all", &mut search);

    // Screen loads: the YouTube half of Home, an artist, an album, a playlist (the anonymous
    // response cache is cleared by using a fresh client per round, so every load is a real one).
    let (mut home, mut artist, mut album, mut playlist) = (vec![], vec![], vec![], vec![]);
    for _ in 0..3 {
        let it = InnerTube::new(Session { visitor_data: vd.clone(), ..Session::default() }, None).unwrap();
        let t = Instant::now();
        let _ = tokio::join!(it.home(meta, None), it.moods(meta));
        home.push(ms(t));
        let t = Instant::now();
        if it.artist(meta, "UCiMhD4jzUqG-IgPzUmmytRQ").await.is_ok() {
            artist.push(ms(t)); // Queen
        }
        let t = Instant::now();
        if it.album(meta, "MPREb_9nqEki4ZDpp").await.is_ok() {
            album.push(ms(t));
        }
        let t = Instant::now();
        if it.playlist(meta, "VLRDCLAK5uy_kmPRjHDECIcuVwnKsx2Ng7fyNgFKWNJFs", None).await.is_ok() {
            playlist.push(ms(t));
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
    println!("-- screens (network part) --");
    summary("home + moods", &mut home);
    summary("artist", &mut artist);
    summary("album", &mut album);
    summary("playlist", &mut playlist);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The iPhone's refusal (TestFlight 1.0.1 build 13), reproduced: a session whose visitorData
/// YouTube does not accept gets "Sign in to confirm you're not a bot" from VISIONOS for every
/// "- Topic" track. The resolve must fetch the anonymous clients a fresh visitorData and play.
///
/// ```sh
/// ENCORE_IOS_CHAIN=1 cargo test -p encore-core --no-default-features --lib \
///   live_bad_visitor_data_heals -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_bad_visitor_data_heals() {
    let dir = std::env::temp_dir().join(format!("fa-vd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = Some("CgtXXXXXXXXXXXXXXXX%3D%3D".to_owned());
    let it = InnerTube::new(Session { visitor_data: bad, ..Session::default() }, None).unwrap();
    let cipher = Arc::new(CipherDeobfuscator::new(None, &dir, Arc::new(PlayerConfigStore::new(&dir))));
    let db = Arc::new(crate::db::Db::open(&dir.join("vd.sqlite")).unwrap());
    let orch = Orchestrator::new(it, Clients::bundled(), cipher, Arc::new(PoTokenGenerator::new(db)));
    // Starboy, Ride: generated audio tracks, the kind VISIONOS bot-checks.
    for id in ["3_g2un5M350", "w1Smzzw_w7Q"] {
        let d = orch.resolve(id, false, AudioQuality::High, &Default::default()).await;
        println!("{id}: {:?}", d.as_ref().map(|d| &d.stream_client));
        assert_eq!(d.unwrap().stream_client, "VISIONOS");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Which picture a podcast episode resolves to (the Mac `<video>` path), and whether a byte range
/// deep in it is served at full speed in bounded chunks.
///
/// cargo test -p encore-core --lib firstaudio_bench::live_podcast_video -- --ignored --nocapture
/// `FA_VIDEO_IDS=a,b` overrides the episodes.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_podcast_video() {
    let dir = std::env::temp_dir().join(format!("fa-video-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd.clone(), ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    #[cfg(target_vendor = "apple")]
    let js: Option<Arc<dyn crate::js::JsEngine>> = Some(Arc::new(crate::js::InProcEngine::jsc()));
    #[cfg(not(target_vendor = "apple"))]
    let js: Option<Arc<dyn crate::js::JsEngine>> = None;
    let cipher = Arc::new(CipherDeobfuscator::new(js, &dir, Arc::new(PlayerConfigStore::new(&dir))));
    let db = Arc::new(crate::db::Db::open(&dir.join("bench.sqlite")).unwrap());
    let potoken = Arc::new(PoTokenGenerator::new(db));
    let orch = Orchestrator::new(it.clone(), clients.clone(), cipher.clone(), potoken.clone());
    cipher.prewarm().await;
    let sts = cipher.signature_timestamp().await;
    let main = clients.get(MAIN_CLIENT).unwrap();
    for id in list("FA_VIDEO_IDS", &["S7CrlFLAmEA", "Uv2w4VTU-iQ"]) {
        if let Some(sd) = it.player(main, &id, None, sts, None).await.ok().and_then(|r| r.streaming_data) {
            let mut v: Vec<String> = sd
                .adaptive_formats
                .iter()
                .filter(|f| !f.is_audio())
                .map(|f| format!("{}:{}p{}", f.itag, f.height.unwrap_or(0), f.mime_type.split(';').next().unwrap_or("")))
                .collect();
            v.sort();
            println!("{id}: WEB_REMIX video formats {v:?}");
            for h in [720, 1080] {
                println!("{id}: pick at {h}: {:?}", innertube::find_video_format(&sd, h).map(|f| f.itag));
            }
        }
        for h in [720, 1080] {
            let Some(url) = orch.resolve_video(&id, h, &Default::default()).await else {
                println!("{id}: resolve_video({h}) -> None");
                continue;
            };
            let itag = url.split("itag=").nth(1).and_then(|s| s.split('&').next()).unwrap_or("?");
            // A bounded 4 MiB range 20 MiB in, the shape the proxy now sends upstream.
            let t = Instant::now();
            let r = crate::http::client()
                .get(&url)
                .header("Range", "bytes=20971520-25165823")
                .send()
                .await;
            let (status, bytes) = match r {
                Ok(r) => (r.status().as_u16(), r.bytes().await.map(|b| b.len()).unwrap_or(0)),
                Err(_) => (0, 0),
            };
            let secs = t.elapsed().as_secs_f64();
            println!(
                "{id}: resolve_video({h}) itag={itag}; 4 MiB range: {status} {bytes} B in {secs:.2}s ({:.1} MB/s)",
                bytes as f64 / secs / 1e6
            );
        }
    }
}
