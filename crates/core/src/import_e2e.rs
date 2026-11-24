//! Live end-to-end check of the Spotify import (stream import2). Not part of the normal suite:
//!
//! ```sh
//! IMPORT_E2E_OUT=/path/report.tsv cargo test -p limusic-app --lib -- --ignored import::e2e --nocapture
//! ```
//!
//! `IMPORT_E2E_PROFILE` picks the public profile (default: Spotify's own) and `IMPORT_E2E_LINKS`
//! adds playlist links (whitespace-separated) to the default editorial one, so a personal profile
//! or playlist never has to be written into the source.
//!
//! 1. Reads a public profile (its public playlists) and real public playlists off their links,
//!    exactly as the app's `import_read` does.
//! 2. Matches every distinct track with the app's own search + scoring (`search_with`,
//!    `classify`), paced like an import, and writes a TSV for hand-checking.
//! 3. Builds a data-export zip in Spotify's real file formats from those tracks (YourLibrary.json,
//!    Playlist1.json, StreamingHistory_music_0.json) and runs it through `read_file` → the
//!    preview (History list) → matching → the writes the "Create" step makes (local playlist,
//!    likes, listening history) into a fresh database, then checks Recently played, the taste
//!    profile and the personal Home shelves.

use std::collections::HashMap;
use std::io::Write as _;

use super::*;

const DEFAULT_PROFILE: &str = "https://open.spotify.com/user/spotify";
const DEFAULT_LINKS: &[&str] = &[
    // A big editorial playlist.
    "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M", // Today's Top Hits
];

fn profile_link() -> String {
    std::env::var("IMPORT_E2E_PROFILE").unwrap_or_else(|_| DEFAULT_PROFILE.to_string())
}

fn playlist_links() -> Vec<String> {
    let extra = std::env::var("IMPORT_E2E_LINKS").unwrap_or_default();
    extra.split_whitespace().map(str::to_string).chain(DEFAULT_LINKS.iter().map(|l| l.to_string())).collect()
}

fn mmss(ms: Option<u64>) -> String {
    ms.map(|ms| format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)).unwrap_or_default()
}

struct Matcher {
    it: innertube::InnerTube,
    clients: innertube::Clients,
    answers: HashMap<String, Answer>,
    searches: usize,
}

impl Matcher {
    fn new() -> Matcher {
        let session = innertube::Session {
            locale: Default::default(),
            visitor_data: None,
            data_sync_id: None,
            cookie: None,
        };
        Matcher {
            it: innertube::InnerTube::new(session, None).unwrap(),
            clients: innertube::Clients::bundled(),
            answers: HashMap::new(),
            searches: 0,
        }
    }

    async fn answer(&mut self, t: &SourceTrack) -> Answer {
        let k = key(t);
        if let Some(a) = self.answers.get(&k) {
            return a.clone();
        }
        let client = self.clients.get(innertube::METADATA_CLIENT).unwrap();
        let mut n = 0;
        let ranked = search_with(
            &self.it,
            client,
            t,
            || {
                n += 1;
                async { Ok::<(), Halt>(tokio::time::sleep(between(SEARCH_GAP_MS)).await) }
            },
            |e| Halt::Failed(e.to_string()),
        )
        .await;
        self.searches += n;
        let a = match ranked {
            Ok(r) => classify(r),
            Err(h) => {
                eprintln!("search failed for {}: {}", t.title, h.code());
                (Tier::Missing, None, Vec::new())
            }
        };
        self.answers.insert(k, a.clone());
        a
    }
}

#[tokio::test]
#[ignore]
async fn live_import_e2e() {
    let out = std::env::var("IMPORT_E2E_OUT")
        .unwrap_or_else(|_| std::env::temp_dir().join("import_e2e.tsv").display().to_string());

    // 1. The profile path, as `import_read` takes it.
    let user = spotify::parse_profile_link(&profile_link()).unwrap();
    let profile = spotify::read_profile(&user).await.expect("profile");
    println!(
        "profile {} ({}): {} of {} public playlists listed",
        profile.name,
        profile.id,
        profile.playlists.len(),
        profile.total
    );
    assert!(!profile.playlists.is_empty());
    let mut lazy: Vec<SourceList> =
        profile.playlists.iter().take(2).map(|p| spotify::lazy_list(p, &profile.name)).collect();
    let failed = fill_lazy(&mut lazy).await;
    println!("profile playlists read lazily: {} ok, {failed} failed", lazy.len());
    assert!(lazy.iter().all(|l| !l.tracks.is_empty()));

    // 2. Real playlists, matched.
    let mut lists = Vec::new();
    for l in playlist_links() {
        let (kind, id) = spotify::parse_link(&l).unwrap();
        let list = spotify::read_link(kind, &id).await.expect("playlist");
        println!("{}: {} tracks (truncated: {})", list.name, list.tracks.len(), list.truncated);
        lists.push(list);
    }
    let mut m = Matcher::new();
    let started = std::time::Instant::now();
    let mut report = String::from(
        "list\ttier\tspotify_title\tspotify_artists\tspotify_len\tyt_title\tyt_artists\tyt_len\tvideo_id\n",
    );
    let mut tiers: HashMap<&'static str, usize> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for l in &lists {
        for t in &l.tracks {
            if !seen.insert(key(t)) {
                continue;
            }
            let (tier, pick, _) = m.answer(t).await;
            *tiers.entry(tier.as_str()).or_default() += 1;
            let p = pick.as_ref();
            report.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                l.name,
                tier.as_str(),
                t.title,
                t.artists.join(", "),
                mmss(t.duration_ms),
                p.map(|s| s.title.as_str()).unwrap_or(""),
                p.map(|s| s.artists.as_str()).unwrap_or(""),
                p.and_then(|s| s.duration.as_deref()).unwrap_or(""),
                p.map(|s| s.video_id.as_str()).unwrap_or(""),
            ));
        }
    }
    std::fs::write(&out, &report).unwrap();
    println!(
        "matched {} distinct tracks in {:?} with {} searches: {:?}\nreport: {out}",
        seen.len(),
        started.elapsed(),
        m.searches,
        tiers
    );

    // 3. A data-export zip in Spotify's own formats, built from the real tracks above.
    let wedding = &lists[0].tracks;
    let brooklyn = &lists[1].tracks;
    let now = now_secs();
    let fmt_time = |t: i64| {
        chrono::DateTime::from_timestamp(t, 0).unwrap().format("%Y-%m-%d %H:%M").to_string()
    };
    let your_library = serde_json::json!({
        "tracks": wedding.iter().take(10).map(|t| serde_json::json!({
            "artist": t.artists.first(), "album": t.album, "track": t.title,
            "uri": t.id.as_ref().map(|i| format!("spotify:track:{i}")),
        })).collect::<Vec<_>>(),
        "albums": [{"artist": "Daft Punk", "album": "Random Access Memories", "uri": "spotify:album:4m2880jivSbbyEGAKfITCa"}],
        "shows": [], "episodes": [], "bannedTracks": [], "other": [],
        "artists": [{"name": "Earth, Wind & Fire", "uri": "spotify:artist:4QQgXkCYTt3BlENzhyNETg"}],
    });
    let playlist1 = serde_json::json!({ "playlists": [{
        "name": "3 am in Brooklyn (export)", "lastModifiedDate": "2026-10-01",
        "collaborators": [], "description": null, "numberOfFollowers": 0,
        "items": brooklyn.iter().map(|t| serde_json::json!({
            "track": {"trackName": t.title, "artistName": t.artists.first(), "albumName": t.album,
                      "trackUri": t.id.as_ref().map(|i| format!("spotify:track:{i}"))},
            "episode": null, "audiobook": null, "localTrack": null, "addedDate": "2026-09-01"
        })).collect::<Vec<_>>(),
    }]});
    // Account-data history: names only, no ids, times in UTC minutes. Liked songs played a lot,
    // a few Brooklyn songs, one skip, and one podcast-ish row that matches nothing.
    let mut history = Vec::new();
    for (i, t) in wedding.iter().take(6).enumerate() {
        for d in 0..(6 - i) {
            history.push(serde_json::json!({
                "endTime": fmt_time(now - 86_400 * (d as i64 + 1) - 3_600 * i as i64),
                "artistName": t.artists.first(), "trackName": t.title, "msPlayed": 185_000
            }));
        }
    }
    for t in brooklyn.iter().take(4) {
        history.push(serde_json::json!({
            "endTime": fmt_time(now - 86_400 * 3), "artistName": t.artists.first(),
            "trackName": t.title, "msPlayed": 200_000
        }));
    }
    history.push(serde_json::json!({
        "endTime": fmt_time(now - 7_200), "artistName": wedding[7].artists.first(),
        "trackName": wedding[7].title, "msPlayed": 4_000
    }));
    let mut zip_bytes = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut zip_bytes);
        let o = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("Spotify Account Data/YourLibrary.json", your_library.to_string()),
            ("Spotify Account Data/Playlist1.json", playlist1.to_string()),
            ("Spotify Account Data/StreamingHistory_music_0.json", serde_json::to_string(&history).unwrap()),
            ("Spotify Account Data/StreamingHistory_podcast_0.json", "[]".into()),
            ("Spotify Account Data/Userdata.json", r#"{"username":"x","country":"NL"}"#.into()),
            ("Spotify Account Data/ReadMeFirst_AccountData.pdf", "%PDF-1.4".into()),
        ] {
            z.start_file(name, o).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    let zip_path = std::path::Path::new(&out).with_file_name("my_spotify_data.zip");
    std::fs::write(&zip_path, zip_bytes.get_ref()).unwrap();

    // What `import_read_file` does with it.
    let bytes = std::fs::read(&zip_path).unwrap();
    let preview = read_file(&bytes, "my_spotify_data.zip").unwrap();
    let preview_json = serde_json::to_value(&preview).unwrap();
    println!("zip preview: {preview_json}");
    let lib = IMPORT.lock().unwrap().pending.clone().unwrap();
    let kinds: Vec<ListKind> = lib.lists.iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [ListKind::Liked, ListKind::Playlist, ListKind::History]);
    assert_eq!(lib.albums.len(), 1);
    assert_eq!(lib.artists.len(), 1);
    assert!(preview_json["history"]["plays"].as_u64().unwrap() >= 20);

    // Matching (the answers above are the cache; anything new is searched).
    let mut zip_answers: HashMap<String, Answer> = HashMap::new();
    let mut zip_searches_before = m.searches;
    for l in &lib.lists {
        for t in &l.tracks {
            let a = m.answer(t).await;
            zip_answers.insert(key(t), a);
        }
    }
    zip_searches_before = m.searches - zip_searches_before;
    println!("zip matching needed {zip_searches_before} new searches (history adopted list ids)");

    // The "Create" step's writes, into a fresh database.
    let db_path = std::path::Path::new(&out).with_file_name("import_e2e.sqlite");
    let _ = std::fs::remove_file(&db_path);
    let db = crate::db::Db::open(&db_path).unwrap();
    crate::smart::migrate(&db);
    let song = |t: &SourceTrack| {
        zip_answers
            .get(&key(t))
            .filter(|a| a.0 != Tier::Missing)
            .and_then(|a| a.1.clone())
    };
    let liked: Vec<SongItem> = lib.lists[0].tracks.iter().filter_map(song).collect();
    crate::library::import_likes(&db, liked.clone()).unwrap();
    let pl_songs: Vec<SongItem> = lib.lists[1].tracks.iter().filter_map(song).collect();
    let pid = db.create_local_playlist(&lib.lists[1].name, now).unwrap();
    let rows: Vec<(String, String)> = pl_songs
        .iter()
        .map(|s| {
            let s = crate::api::playlist_row(s.clone());
            (s.video_id.clone(), serde_json::to_string(&s).unwrap())
        })
        .collect();
    db.add_local_playlist_tracks(pid, &rows, now).unwrap();
    let written = crate::history::write_plays(&db, &lib.history, song);
    println!(
        "zip import: {} likes, playlist with {} songs, history {written:?}",
        liked.len(),
        rows.len()
    );
    assert!(written.listens > 15, "{written:?}");

    // Home is personal right away.
    let recent = db.with_conn(|c| crate::library::recent_tracks(c, 20));
    println!(
        "Recently played: {:?}",
        recent.iter().take(5).map(|r| (&r.song.title, r.plays)).collect::<Vec<_>>()
    );
    assert!(!recent.is_empty());
    let taste = crate::smart::taste::profile(&db);
    let mut top: Vec<(&String, &f64)> = taste.tracks.iter().collect();
    top.sort_by(|a, b| b.1.total_cmp(a.1));
    println!("taste: {} tracks scored, top {:?}", taste.tracks.len(), &top[..top.len().min(3)]);
    assert!(!taste.tracks.is_empty());
    let shelves = crate::smart::home::local_shelves(
        &db,
        &taste,
        &crate::smart::library::Filter::load(&db),
    );
    let ids: Vec<&str> = shelves.iter().map(|s| s.id.as_str()).collect();
    println!("personal Home shelves: {ids:?}");
    assert!(ids.contains(&"recently_played"));
    // A second import of the same file writes no plays twice.
    let again = crate::history::write_plays(&db, &lib.history, song);
    assert_eq!(again.listens + again.skips, 0, "{again:?}");
}
