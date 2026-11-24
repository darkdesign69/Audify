//! Tests for `canonical`. Fixtures (real YouTube Music rows, trimmed) live in
//! `tests/fixtures/canonical/`. The live check:
//!
//! ```sh
//! LIBRARY_PATH=/opt/homebrew/lib cargo test -p limusic-app --lib -- --ignored canonical::tests::live --nocapture
//! ```
//! `CANONICAL_DUMP=<dir>` makes it write the pages it saw as fixtures.

use super::*;

fn live_resolver_parts() -> (innertube::InnerTube, innertube::Clients, Db) {
    let session = innertube::Session {
        locale: Default::default(),
        visitor_data: None,
        data_sync_id: None,
        cookie: None,
    };
    (
        innertube::InnerTube::new(session, None).unwrap(),
        innertube::Clients::bundled(),
        crate::smart::test_db(),
    )
}

/// Blinding Lights (Official Video), The Weeknd.
const OFFICIAL_VIDEO: &str = "4NRXx6U8ABQ";

#[tokio::test]
#[ignore = "hits live YouTube Music"]
async fn live_radio_from_official_video_is_songs() {
    let (it, clients, db) = live_resolver_parts();
    it.set_visitor_data(Some(it.fetch_visitor_data().await.expect("visitor data")));
    let client = clients.get(innertube::METADATA_CLIENT).unwrap();
    let r = Resolver { it: &it, client, db: &db };
    let raw = it
        .next(client, Some(OFFICIAL_VIDEO), Some(&format!("RDAMVM{OFFICIAL_VIDEO}")))
        .await
        .unwrap();
    let seed = raw.items.iter().find(|i| i.video_id == OFFICIAL_VIDEO).cloned().expect("seed row");
    let raw_atv = raw.items.iter().filter(|i| !i.is_video).count();
    eprintln!("raw radio on the video: {} rows, {} audio tracks", raw.items.len(), raw_atv);
    assert!(seed.is_video, "the seed is a music video");
    let atv = r.audio_seed(&seed).await.expect("the video has an audio track");
    eprintln!("seed {OFFICIAL_VIDEO} -> audio track {atv}");
    let page = it.next(client, Some(&atv), Some(&format!("RDAMVM{atv}"))).await.unwrap();
    let before = page.items.iter().filter(|i| !i.is_video).count();
    let seed_row = page.items.iter().find(|i| i.video_id == atv).cloned();
    if let Ok(dir) = std::env::var("CANONICAL_DUMP") {
        let dir = std::path::Path::new(&dir);
        std::fs::write(
            dir.join("radio_on_video.json"),
            serde_json::to_string_pretty(&raw.items).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("radio_on_audio.json"),
            serde_json::to_string_pretty(&page.items).unwrap(),
        )
        .unwrap();
        for v in raw.items.iter().take(12) {
            let q = query_for(v);
            let Ok(found) = it.search_songs(client, &q, false).await else { continue };
            let name = format!("search_{}.json", v.video_id);
            let doc = serde_json::json!({ "video": v, "query": q, "results": found.items });
            std::fs::write(dir.join(name), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        }
    }
    let rows = page.items.len();
    let out = r.canonicalize(page.items, seed_row.as_ref()).await;
    let songs = out.iter().filter(|s| !s.is_video && admits(s, Some(Version::Studio))).count();
    for s in &out {
        eprintln!(
            "  {} {:<50} {}{}",
            s.video_id,
            s.title,
            s.artists,
            if s.is_video { "  [VIDEO]" } else { "" }
        );
    }
    let share = songs as f64 / out.len().max(1) as f64;
    eprintln!(
        "audio-seeded radio: {rows} rows, {before} audio before; after canonical: {} rows, {songs} studio audio tracks ({:.0}%)",
        out.len(),
        share * 100.0
    );
    assert!(out.len() >= 20, "a radio keeps most of its rows");
    assert!(share >= 0.9, "at least 90% audio-track songs, got {:.0}%", share * 100.0);
    // And the same with the raw video radio mapped row by row (the no-reseed fallback).
    let mapped = r.canonicalize(raw.items, Some(&seed)).await;
    let ok = mapped.iter().filter(|s| !s.is_video || s.video_id == OFFICIAL_VIDEO).count();
    eprintln!("video radio mapped row by row: {} rows, {} songs", mapped.len(), ok);
}

#[tokio::test]
#[ignore = "hits live YouTube Music"]
async fn live_discover_weekly_offenders() {
    // The rows seen in a real Weekly Discoveries: a visualizer maps to its song, a live upload goes.
    let (it, clients, db) = live_resolver_parts();
    it.set_visitor_data(Some(it.fetch_visitor_data().await.expect("visitor data")));
    let client = clients.get(innertube::METADATA_CLIENT).unwrap();
    let r = Resolver { it: &it, client, db: &db };
    let mut rows = Vec::new();
    for q in ["Bob Dylan Knockin' On Heaven's Door official", "Eagles Hotel California live 1976"] {
        let mut found = it.search_videos(client, q).await;
        for _ in 0..3 {
            if found.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            found = it.search_videos(client, q).await;
        }
        let found = found.unwrap();
        // The real uploads: Bob Dylan's own "(Official Audio)" video, Eagles' "(Live 1976)".
        let want = if q.contains("Dylan") { "(Official" } else { "(Live 1976)" };
        for v in found.items.iter().take(6) {
            eprintln!("    result: {} {} | {}", v.video_id, v.title, v.artists);
        }
        let v = found
            .items
            .iter()
            .find(|v| {
                v.is_video
                    && v.title.contains(want)
                    && ["Bob Dylan", "Eagles"].contains(&v.artists.as_str())
            })
            .cloned();
        let v = v.or_else(|| found.items.into_iter().find(|v| v.is_video)).expect("a video");
        eprintln!("video: {} {} | {}", v.video_id, v.title, v.artists);
        rows.push(v);
    }
    let out = r.canonicalize(rows.clone(), None).await;
    for s in &out {
        eprintln!(
            "  -> {} {} | {} (video twin {:?})",
            s.video_id, s.title, s.artists, s.counterpart
        );
    }
    assert!(out.iter().all(|s| !s.is_video && version(s) == Version::Studio));
    for v in &rows {
        if version(v) == Version::Studio {
            assert!(
                out.iter().any(|s| s.counterpart.as_ref() == Some(&v.video_id)),
                "{} mapped",
                v.title
            );
        }
    }
}

// --- fixtures --------------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct SearchFx {
    video: SongItem,
    results: Vec<SongItem>,
}

fn searches() -> Vec<SearchFx> {
    serde_json::from_str(include_str!("../tests/fixtures/canonical/searches.json")).unwrap()
}
fn radio_on_video() -> Vec<SongItem> {
    serde_json::from_str(include_str!("../tests/fixtures/canonical/radio_on_video.json")).unwrap()
}
fn radio_on_audio() -> Vec<SongItem> {
    serde_json::from_str(include_str!("../tests/fixtures/canonical/radio_on_audio.json")).unwrap()
}

fn song(id: &str, title: &str, artists: &str, video: bool) -> SongItem {
    SongItem {
        video_id: id.into(),
        title: title.into(),
        artists: artists.into(),
        is_video: video,
        duration: Some("3:30".into()),
        ..SongItem::default()
    }
}

/// The search answers as a cache: what [`prepare`] sees once the lookups are done.
fn answers() -> HashMap<String, Option<SongItem>> {
    searches()
        .into_iter()
        .map(|f| (f.video.video_id.clone(), best_audio(&f.video, &f.results)))
        .collect()
}

#[test]
fn every_fixture_video_finds_its_audio_track() {
    // Real Songs searches for real radio rows: the first audio track of the same song, never the
    // karaoke ("Originally Performed By"), the slowed upload or another song by the same artist.
    let want = [
        ("4NRXx6U8ABQ", "J7p4bzqLvCw"), // Blinding Lights (Official Video)
        ("34Na4j8AVgA", "3_g2un5M350"), // Starboy ft. Daft Punk (Official Video)
        ("ApXoWvfEYVU", "r7Rn4ryE_w8"), // Sunflower -> Sunflower (Spider-Man: …)
        ("KEI4qSrkPAs", "ricvj03PHSU"), // Can't Feel My Face (Official Video)
        ("TO-_3tck2tg", "V2SVRJ6dcE4"), // Bones
        ("kTJczUoc26U", "XfEMj-z3TtA"), // STAY (Official Video)
        ("pXRviuL6vMY", "K1FlAphL2p8"), // Stressed Out
    ];
    let got = answers();
    for (video, audio) in want {
        let a =
            got.get(video).cloned().flatten().unwrap_or_else(|| panic!("{video}: no audio track"));
        assert_eq!(a.video_id, audio, "{video}");
        assert!(!a.is_video);
    }
}

#[test]
fn no_audio_track_for_a_different_song() {
    // Only other songs by the artist in the results: nothing stands in.
    let f = searches().into_iter().find(|f| f.video.video_id == "TO-_3tck2tg").unwrap();
    let others: Vec<SongItem> = f.results.into_iter().filter(|r| r.title != "Bones").collect();
    assert!(best_audio(&f.video, &others).is_none());
    // A karaoke of the song is not its audio track either.
    let video = song("v", "Can't Feel My Face (Official Video)", "The Weeknd", true);
    let karaoke =
        song("k", "Can't Feel My Face (Originally Perfomed By The Weeknd)", "ZZang KARAOKE", false);
    assert!(best_audio(&video, &[karaoke]).is_none());
    // Nor a live cut of a studio video.
    let live = song("l", "Blinding Lights (Live)", "The Weeknd", false);
    assert!(best_audio(
        &song("v", "Blinding Lights (Official Video)", "The Weeknd", true),
        &[live]
    )
    .is_none());
}

#[test]
fn queries_drop_the_video_dressing() {
    assert_eq!(
        query_for(&song("v", "Blinding Lights (Official Video)", "The Weeknd", true)),
        "The Weeknd Blinding Lights"
    );
    assert_eq!(
        query_for(&song("v", "Knockin' On Heaven's Door (Visualizer)", "Bob Dylan", true)),
        "Bob Dylan Knockin' On Heaven's Door"
    );
    assert_eq!(
        query_for(&song(
            "v",
            "Earth, Wind & Fire - September (Official Video)",
            "Earth, Wind & Fire",
            true
        )),
        "Earth, Wind & Fire September"
    );
    // A version keeps its word.
    let q = query_for(&song("v", "Hotel California (Live 1976)", "Eagles", true));
    assert!(q.contains("Live") && q.contains("Hotel California") && q.starts_with("Eagles"), "{q}");
}

#[derive(serde::Deserialize)]
struct VersionFx {
    title: String,
    artists: String,
    is_video: bool,
    studio: bool,
    #[serde(default)]
    admitted: Option<bool>,
}

#[test]
fn version_policy() {
    let fx: Vec<VersionFx> =
        serde_json::from_str(include_str!("../tests/fixtures/canonical/versions.json")).unwrap();
    for f in fx {
        let s = song("x", &f.title, &f.artists, f.is_video);
        let v = version(&s);
        assert_eq!(allowed(v, Some(Version::Studio)), f.studio, "{} -> {v:?}", f.title);
        assert_eq!(admits(&s, None), f.admitted.unwrap_or(f.studio), "{}", f.title);
        // The seed's own version is always welcome.
        assert!(allowed(v, Some(v)), "{}", f.title);
    }
    assert!(admits(
        &song("x", "Hotel California (Live 1976)", "Eagles", false),
        Some(Version::Live)
    ));
}

#[test]
fn radio_on_a_video_maps_to_songs() {
    // The raw radio YouTube builds on "Blinding Lights (Official Video)": 50 music videos.
    let raw = radio_on_video();
    assert_eq!(raw.iter().filter(|s| s.is_video).count(), raw.len());
    let seed = raw[0].clone();
    let known = answers();
    let mut items = raw.clone();
    let pending = prepare(&mut items, Some(&seed), |id| known.get(id).cloned());
    // The seed stays as the user's video; every answered video became its audio track, with the
    // video kept as its Song/Video twin.
    assert_eq!(items[0].video_id, seed.video_id);
    let starboy = items.iter().find(|s| s.video_id == "3_g2un5M350").expect("Starboy mapped");
    assert_eq!(starboy.counterpart.as_deref(), Some("34Na4j8AVgA"));
    assert!(!starboy.is_video);
    // The seed's own audio track never follows it (same song).
    assert!(!items.iter().any(|s| s.video_id == "J7p4bzqLvCw"));
    // What's left unanswered is for the lookup pass, and only videos.
    assert!(pending.iter().all(|s| s.is_video));
    assert_eq!(pending.len(), items.iter().skip(1).filter(|s| s.is_video).count());
    // "Dancin - Krono Remix (Official Video)" may map (a released remix); a live upload may not.
    assert!(!items.iter().any(|s| version(s) == Version::Live));
}

#[test]
fn radio_on_the_audio_track_is_clean() {
    // The radio on the seed's audio track (what a radio on the video now runs on).
    let mut items = radio_on_audio();
    let seed = items[0].clone();
    let pending = prepare(&mut items, Some(&seed), |_| None);
    assert!(pending.is_empty());
    let titles: Vec<&str> = items.iter().map(|s| s.title.as_str()).collect();
    // The cover goes; released remixes stay; nothing is a video.
    assert!(!titles.iter().any(|t| t.contains("Remake Cover")), "{titles:?}");
    assert!(titles.contains(&"Cold Heart (PNAU Remix)"));
    assert!(items.iter().all(|s| !s.is_video));
    assert_eq!(items.len(), 49);
}

#[test]
fn prepare_swaps_paired_videos_and_drops_known_dead_ends() {
    let mut paired = song("mv", "Song A (Official Video)", "Artist", true);
    paired.counterpart = Some("atv".into());
    let dead = song("ugc", "Song B", "Someone", true);
    let mut items = vec![paired, dead, song("c", "Song C", "Artist", false)];
    let lookup = |id: &str| (id == "ugc").then_some(None);
    let pending = prepare(&mut items, None, lookup);
    assert!(pending.is_empty());
    let ids: Vec<&str> = items.iter().map(|s| s.video_id.as_str()).collect();
    assert_eq!(ids, ["atv", "c"]);
    assert_eq!(items[0].counterpart.as_deref(), Some("mv"));
    assert!(!items[0].is_video);
}

#[test]
fn dedup_keeps_one_row_per_song_audio_first() {
    let items = vec![
        song("v1", "Hotel California (Official Video)", "Eagles", true),
        song("a1", "Hotel California (2013 Remaster)", "Eagles", false),
        song("a2", "Hotel California", "Eagles", false),
        song("a1", "Hotel California (2013 Remaster)", "Eagles", false),
        song("b", "Take It Easy", "Eagles", false),
    ];
    let out = dedup(items, None);
    let ids: Vec<&str> = out.iter().map(|s| s.video_id.as_str()).collect();
    assert_eq!(ids, ["a1", "b"], "the audio track takes the video's slot");
}

#[test]
fn apply_replaces_ahead_of_the_play_position_only() {
    let mut playing = song("p", "Now (Official Video)", "X", true);
    playing.counterpart = None;
    let mut queued = song("q", "Mine (Official Video)", "Y", true);
    queued.queued = true;
    let mut items = vec![
        song("h", "Past (Official Video)", "Z", true), // history: untouched
        playing,
        song("v1", "One (Official Video)", "A", true),
        queued,
        song("v2", "Two (Visualizer)", "B", true),
        song("a3", "Three", "C", false),
        song("v3", "Three (Official Video)", "C", true),
    ];
    let mut found = HashMap::new();
    found.insert("h".to_owned(), Some(song("ha", "Past", "Z", false)));
    found.insert("v1".to_owned(), Some(song("a1", "One", "A", false)));
    found.insert("q".to_owned(), Some(song("qa", "Mine", "Y", false)));
    found.insert("v2".to_owned(), None);
    found.insert("v3".to_owned(), Some(song("a3", "Three", "C", false)));
    let first = apply(&mut items, 2, Some("p"), &found);
    assert_eq!(first, Some(2));
    let ids: Vec<&str> = items.iter().map(|s| s.video_id.as_str()).collect();
    // v1 -> a1; the hand-queued video stays; v2 has no song and goes; v3's song is already there.
    assert_eq!(ids, ["h", "p", "a1", "q", "a3"]);
    assert_eq!(items[2].counterpart.as_deref(), Some("v1"));
}

#[test]
fn episodes_and_local_files_pass_untouched() {
    let mut ep = song("e", "Episode 12 (Live)", "Show", true);
    ep.is_episode = true;
    let mut items = vec![ep];
    let pending = prepare(&mut items, None, |_| Some(None));
    assert!(pending.is_empty());
    assert_eq!(items.len(), 1);
}

#[test]
fn autoplay_never_brings_back_a_song_the_queue_has() {
    let mut playing = song("mv", "Blinding Lights (Official Video)", "The Weeknd", true);
    playing.counterpart = Some("atv".into());
    let have = vec![playing, song("s", "Starboy (feat. Daft Punk)", "The Weeknd", false)];
    let mut fresh = vec![
        song("atv", "Blinding Lights", "The Weeknd", false),
        song("s2", "Starboy", "The Weeknd", false),
        song("n", "Save Your Tears", "The Weeknd", false),
    ];
    drop_known(&mut fresh, &have);
    let ids: Vec<&str> = fresh.iter().map(|s| s.video_id.as_str()).collect();
    assert_eq!(ids, ["n"]);
}

#[test]
fn own_plays_keep_their_versions_but_not_their_videos() {
    // Throwback Mix / a Your Mix's favourites: the live cut the user loves stays; the music
    // video they played becomes its song.
    let mut items = vec![
        song("live", "Hotel California (Live 1976)", "Eagles", false),
        song("mv", "Knockin' On Heaven's Door (Visualizer)", "Bob Dylan", true),
    ];
    let atv = song("atv", "Knockin' On Heaven's Door", "Bob Dylan", false);
    let pending = prepare_songs(&mut items, |id| (id == "mv").then(|| Some(atv.clone())));
    assert!(pending.is_empty());
    let ids: Vec<&str> = items.iter().map(|s| s.video_id.as_str()).collect();
    assert_eq!(ids, ["live", "atv"]);
    // The generated-list pass drops the live cut.
    let mut items = vec![song("live", "Hotel California (Live 1976)", "Eagles", false)];
    prepare(&mut items, None, |_| None);
    assert!(items.is_empty());
}

// ── stream: reliability ──
#[test]
fn post_play_lookups_coalesce_and_one_worker_drains_them() {
    let row = |id: &str| SongItem { video_id: id.into(), ..Default::default() };
    let mut d = Deferred::default();
    assert!(d.push(vec![row("a"), row("b")]), "first push starts the worker");
    assert!(!d.push(vec![row("b"), row("c")]), "a worker is already draining");
    let batch: Vec<String> = d.take().into_iter().map(|s| s.video_id).collect();
    assert_eq!(batch, ["a", "b", "c"], "one batch, deduped, in order");
    assert!(d.take().is_empty(), "nothing left: the worker stops");
    assert!(d.push(vec![row("d")]), "and the next push starts a new one");
}

// ── stream: autoplay ── a live cut seeds autoplay on its studio recording.
#[test]
fn a_live_seed_looks_for_its_studio_song() {
    let live = song("lv", "Bohemian Rhapsody (Live Aid 1985)", "Queen", false);
    let probe = studio_probe(&live);
    assert_eq!(probe.title, "Bohemian Rhapsody");
    assert_eq!(version(&probe), Version::Studio);
    assert_eq!(query_for(&probe), "Queen Bohemian Rhapsody");
    let cands = vec![
        song("lv2", "Bohemian Rhapsody (Live at Wembley '86)", "Queen", false),
        song("pan", "Bohemian Rhapsody", "Panic! At The Disco", false),
        song("st", "Bohemian Rhapsody", "Queen", false),
    ];
    let found = best_audio(&probe, &cands).expect("the studio song");
    assert_eq!(found.video_id, "st", "not another live cut, not someone else's song");
}
