//! Fixture-driven tests for `identity` (research 22 §4). Fixtures live in
//! `tests/fixtures/smart/`; add every real-world miss to them.

use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct TitleCase {
    title: String,
    artist: String,
    base: String,
    version: String,
    #[serde(default)]
    feat: Vec<String>,
    #[serde(default)]
    remaster: bool,
    #[serde(default)]
    edit: bool,
    #[serde(default)]
    video_marker: bool,
}

#[derive(Deserialize)]
struct SrcFx {
    title: String,
    artists: Vec<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize)]
struct CandFx {
    id: String,
    title: String,
    artists: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    video: bool,
}

#[derive(Deserialize)]
struct MatchCase {
    name: String,
    src: SrcFx,
    cands: Vec<CandFx>,
    pick: String,
    tier: String,
    #[serde(default)]
    gated: HashMap<String, Vec<String>>,
}

fn version_name(v: Version) -> String {
    serde_json::to_value(v).unwrap().as_str().unwrap().to_owned()
}

fn gate_name(g: Gate) -> String {
    serde_json::to_value(g).unwrap().as_str().unwrap().to_owned()
}

pub fn song(
    id: &str,
    title: &str,
    artists: &str,
    album: Option<&str>,
    dur: &str,
) -> SongItem {
    serde_json::from_value(serde_json::json!({
        "video_id": id, "title": title, "artists": artists, "album": album, "duration": dur,
    }))
    .unwrap()
}

#[test]
fn titles_fixture() {
    let cases: Vec<TitleCase> =
        serde_json::from_str(include_str!("../tests/fixtures/smart/titles.json")).unwrap();
    let mut fails = Vec::new();
    for c in &cases {
        let p = parse(&c.title, &c.artist, None);
        let got = (
            p.base.clone(),
            version_name(p.version),
            p.feat.clone(),
            p.remaster,
            p.edit,
            p.video_marker,
        );
        let want =
            (c.base.clone(), c.version.clone(), c.feat.clone(), c.remaster, c.edit, c.video_marker);
        if got != want {
            fails.push(format!("{:?}: got {:?}, want {:?}", c.title, got, want));
        }
    }
    assert!(fails.is_empty(), "{} title cases failed:\n{}", fails.len(), fails.join("\n"));
}

#[test]
fn matches_fixture() {
    let cases: Vec<MatchCase> =
        serde_json::from_str(include_str!("../tests/fixtures/smart/matches.json")).unwrap();
    let mut fails = Vec::new();
    for c in &cases {
        let src = Track {
            title: c.src.title.clone(),
            artist_line: c.src.artists.join(", "),
            artists: c.src.artists.clone(),
            album: c.src.album.clone(),
            duration: c.src.duration.as_deref().and_then(secs),
            ..Default::default()
        };
        let mut ranked: Vec<(Score, String)> = c
            .cands
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let mut s = song(
                    &f.id,
                    &f.title,
                    &f.artists,
                    f.album.as_deref(),
                    f.duration.as_deref().unwrap_or(""),
                );
                s.is_video = f.video;
                (score(&src, &Track::from_song(&s), i), f.id.clone())
            })
            .collect();
        for (id, want) in &c.gated {
            let (sc, _) = ranked.iter().find(|(_, i)| i == id).unwrap();
            let mut got: Vec<String> = sc.gates.iter().copied().map(gate_name).collect();
            got.sort();
            let mut want = want.clone();
            want.sort();
            if got != want {
                fails.push(format!("{}: {id} gates {got:?}, want {want:?}", c.name));
            }
        }
        sort_ranked(&mut ranked);
        let t = tier(&ranked.iter().map(|(s, _)| s).collect::<Vec<_>>());
        let t = serde_json::to_value(t).unwrap().as_str().unwrap().to_owned();
        let pick = ranked[0].1.clone();
        if pick != c.pick || t != c.tier {
            let dump: Vec<String> = ranked
                .iter()
                .map(|(s, id)| format!("{id}={:.3}/{:.3}{:?}", s.value, s.rank, s.gates))
                .collect();
            fails.push(format!(
                "{}: got {pick}/{t}, want {}/{} [{}]",
                c.name,
                c.pick,
                c.tier,
                dump.join(" ")
            ));
        }
    }
    assert!(fails.is_empty(), "{} match cases failed:\n{}", fails.len(), fails.join("\n"));
}

#[test]
fn artist_is_never_matched_in_the_title() {
    // The root karaoke bug: the source artist named in the candidate *title* earned full marks.
    let src = Track {
        title: "Fantasy".into(),
        artists: vec!["Earth, Wind & Fire".into()],
        artist_line: "Earth, Wind & Fire".into(),
        ..Default::default()
    };
    let cand = Track::from_song(&song(
        "k",
        "Fantasy (As Made Famous By Earth, Wind & Fire)",
        "The Karaoke Crew",
        None,
        "3:47",
    ));
    assert!(artist_score(&src, &cand, &cand.parsed()) < 0.7);
}

fn ids(v: &[SongItem]) -> Vec<&str> {
    v.iter().map(|s| s.video_id.as_str()).collect()
}

#[test]
fn search_creep_karaoke_puts_karaoke_first() {
    // Case 9: the query asks for karaoke.
    let rows = vec![
        song("orig", "Creep", "Radiohead", Some("Pablo Honey"), "3:59"),
        song("kar", "Creep (Karaoke Version)", "Sing King", None, "3:58"),
    ];
    let out = rerank("creep karaoke", rows, Track::from_song);
    assert_eq!(ids(&out)[0], "kar");
}

#[test]
fn search_shake_it_off_puts_the_studio_track_first() {
    // Case 10: YouTube put a cover and a video on top.
    let mut omv = song("omv", "Shake It Off (Official Video)", "Taylor Swift", None, "4:02");
    omv.is_video = true;
    let rows = vec![
        song("cover", "Shake It Off (Cover)", "Arrocha Funk", None, "3:30"),
        omv,
        song("other", "Blank Space", "Taylor Swift", None, "3:51"),
        song("atv", "Shake It Off", "Taylor Swift", Some("1989"), "3:39"),
        song("live", "Shake It Off (Live)", "Taylor Swift", None, "3:50"),
    ];
    let out = rerank("shake it off", rows, Track::from_song);
    let order = ids(&out);
    assert_eq!(order[0], "atv");
    // The video is the same work: it goes behind the other distinct versions.
    assert_eq!(*order.last().unwrap(), "omv");
    // The cover YouTube put first is no longer on top.
    assert_ne!(order[0], "cover");
    assert_ne!(order[1], "cover");
}

#[test]
fn search_live_query_keeps_live_on_top() {
    let rows = vec![
        song("live", "Creep (Live)", "Radiohead", None, "4:20"),
        song("studio", "Creep", "Radiohead", None, "3:59"),
    ];
    assert_eq!(ids(&rerank("creep live", rows, Track::from_song))[0], "live");
}

#[test]
fn search_keeps_youtube_order_between_equals() {
    let rows = vec![
        song("a", "Yellow", "Coldplay", None, "4:29"),
        song("b", "Fix You", "Coldplay", None, "4:55"),
        song("c", "Viva La Vida", "Coldplay", None, "4:02"),
    ];
    assert_eq!(ids(&rerank("coldplay", rows, Track::from_song)), ["a", "b", "c"]);
}

#[test]
fn artist_page_dedup_keeps_one_me_and_mrs_jones() {
    // Case 8: two single versions + the plain title collapse into the plain one.
    let mut a = song("sv1", "Me and Mrs. Jones (Single Version)", "Billy Paul", None, "3:40");
    a.play_count = Some("20M".into());
    let mut b = song("sv2", "Me and Mrs. Jones (Single Version)", "Billy Paul", None, "3:40");
    b.play_count = Some("5M".into());
    let mut c = song("plain", "Me and Mrs. Jones", "Billy Paul", None, "4:44");
    c.play_count = Some("1M".into());
    let live = song("live", "Me and Mrs. Jones (Live)", "Billy Paul", None, "5:10");
    let other = song("other", "Thanks for Saving My Life", "Billy Paul", None, "3:20");
    let mut v = vec![a, other, b, c, live];
    dedup_songs(&mut v);
    assert_eq!(ids(&v), ["plain", "other", "live"]);
}

#[test]
fn dedup_prefers_audio_over_video_then_plays() {
    let mut vid = song("vid", "Hello", "Adele", None, "6:07");
    vid.is_video = true;
    vid.play_count = Some("3B".into());
    let mut atv = song("atv", "Hello", "Adele", None, "4:55");
    atv.play_count = Some("900M".into());
    let mut v = vec![vid, atv];
    dedup_songs(&mut v);
    assert_eq!(ids(&v), ["atv"]);
}

#[test]
fn play_counts_parse() {
    assert_eq!(play_count("53M"), 53e6);
    assert_eq!(play_count("1.2B plays"), 1.2e9);
    assert_eq!(play_count("820K"), 820e3);
    assert_eq!(play_count("1,234"), 1234.0);
    assert_eq!(play_count(""), 0.0);
}

/// The live "daft punk" song search in YouTube's two observed orders (it reshuffles between
/// requests). Plays decide for an artist query, so both orders give the same, most-played Songs.
#[test]
fn search_artist_query_is_stable_and_most_played_first() {
    let row = |id: &str, title: &str, artists: &str, plays: &str| {
        let mut s = song(id, title, artists, None, "3:30");
        s.play_count = Some(plays.into());
        s
    };
    let atw = row("atw", "Around the World", "Daft Punk", "303M");
    let vq = row("vq", "Veridis Quo (Edit)", "Daft Punk", "113M");
    let omt = row("omt", "One More Time (Radio Edit)", "Daft Punk", "859M");
    let dz = row("dz", "Derezzed (From \"TRON: Legacy\"/Score)", "Daft Punk", "44M");
    let gl = row("gl", "Give Life Back to Music (Drumless Edition)", "Daft Punk", "55M");
    let ly = row("ly", "Lose Yourself to Dance (feat. Pharrell Williams)", "Daft Punk & Pharrell Williams", "390M");
    let hb = row("hb", "Harder, Better, Faster, Stronger (Alive Radio Edit 2007)", "Daft Punk", "47M");
    let ic = row("ic", "Instant Crush (feat. Julian Casablancas)", "Daft Punk & Julian Casablancas", "1.2B");
    let a = vec![atw.clone(), vq.clone(), omt.clone(), dz.clone(), gl.clone(), ly.clone(), hb.clone(), ic.clone()];
    let b = vec![atw, dz, omt, ly, ic, gl, vq, hb];
    let top = |v: Vec<SongItem>| {
        let mut t: Vec<String> =
            rerank_search_songs("daft punk", v).iter().take(4).map(|s| s.video_id.clone()).collect();
        t.sort();
        t
    };
    assert_eq!(top(a.clone()), ["atw", "ic", "ly", "omt"]);
    assert_eq!(top(a), top(b));
}

#[test]
fn search_title_query_still_puts_the_title_match_first() {
    let row = |id: &str, title: &str, plays: &str| {
        let mut s = song(id, title, "Taylor Swift", None, "3:30");
        s.play_count = Some(plays.into());
        s
    };
    let rows = vec![row("sio", "Shake It Off", "900M"), row("bs", "Blank Space", "3B")];
    assert_eq!(ids(&rerank_search_songs("shake it off", rows))[0], "sio");
}

// ── stream: bench2 ── search-ranking benchmark cases (docs/bench/search.md).
fn played(id: &str, title: &str, artists: &str, plays: &str) -> SongItem {
    let mut s = song(id, title, artists, None, "3:30");
    s.play_count = Some(plays.into());
    s
}

/// A band whose name holds the query ("Uptown Funk Empire", "Hello Adele Tribute") is not the
/// artist the query names: the title match keeps the top.
#[test]
fn search_namesake_act_is_not_an_artist_query() {
    let rows = vec![
        played("boogie", "Boogie", "Uptown Funk Empire", "557K"),
        played("uf", "Uptown Funk (feat. Bruno Mars)", "Mark Ronson", "6.8B"),
    ];
    assert_eq!(ids(&rerank_search_songs("uptown funk", rows))[0], "uf");
    let rows = vec![
        played("trib", "Someone Like You", "Hello Adele Tribute", "7.2M"),
        played("hello", "Hello", "Adele", "3.5B"),
    ];
    assert_eq!(ids(&rerank_search_songs("hello adele", rows))[0], "hello");
}

/// "halo" names an act (the game soundtrack's "Halo") and a hit song: the more played wins.
#[test]
fn search_artist_intent_yields_to_a_bigger_title_match() {
    let rows = vec![
        played("game", "Blow Me Away", "Breaking Benjamin & Halo", "37M"),
        played("halo", "Halo", "Beyoncé", "1.2B"),
    ];
    assert_eq!(ids(&rerank_search_songs("halo", rows))[0], "halo");
}

/// A misspelt or half-typed artist query still lists the artist's most played songs first.
#[test]
fn search_typo_and_prefix_artist_queries() {
    let rows = || {
        vec![
            played("new", "hate that i made you love me", "Ariana Grande", "297M"),
            played("tun", "thank u, next", "Ariana Grande", "2.1B"),
        ]
    };
    assert_eq!(ids(&rerank_search_songs("arianna grande", rows()))[0], "tun");
    assert_eq!(ids(&rerank_search_songs("ariana gra", rows()))[0], "tun");
}

/// An album on top gives way to its hit title track; an album query, or a title track that's no
/// hit, keeps the album.
#[test]
fn search_top_album_yields_to_a_hit_title_track() {
    let card = |kind: &'static str, id: &str, title: &str, sub: &str, plays: Option<&str>| {
        innertube::BrowseItem {
            kind,
            id: id.into(),
            title: title.into(),
            subtitle: Some(sub.into()),
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: Vec::new(),
            play_count: plays.map(Into::into),
            is_video: false,
            is_upload: false,
            explicit: false,
        }
    };
    let results = |plays: &str| innertube::SearchResults {
        top: vec![card("album", "MPRE1", "Starboy", "Album • The Weeknd • 2016", None)],
        songs: vec![card("song", "sb", "Starboy (feat. Daft Punk)", "The Weeknd", Some(plays))],
        ..Default::default()
    };
    assert_eq!(rerank_search_all("starboy", results("4.1B")).top[0].id, "sb");
    assert_eq!(rerank_search_all("starboy album", results("4.1B")).top[0].id, "MPRE1");
    assert_eq!(rerank_search_all("starboy", results("147M")).top[0].id, "MPRE1");
}

// ── stream: podtop ── an artist query blends in the artist page's "Top songs" order.
#[test]
fn search_artist_query_blends_the_artist_page_top_songs() {
    let rows = vec![
        played("tun", "thank u, next", "Ariana Grande", "2.1B"),
        played("7r", "7 rings", "Ariana Grande", "1.9B"),
        played("pos", "positions", "Ariana Grande", "800M"),
        played("hate", "hate that i made you love me", "Ariana Grande", "297M"),
    ];
    let ranked = rerank_search_songs("ariana grande", rows.clone());
    assert_eq!(ids(&ranked), ["tun", "7r", "pos", "hate"]);
    // The page's shelf: the new single first (another upload of it), then 7 rings.
    let top = vec![
        played("hate-page", "hate that i made you love me", "Ariana Grande", "297M"),
        played("7r", "7 rings", "Ariana Grande", "1.9B"),
    ];
    // Borda: tun 0+4, 7r 1+1, pos 2+4, hate 3+0.
    let out = apply_artist_top_songs("ariana grande", ranked.clone(), "Ariana Grande", &top);
    assert_eq!(ids(&out), ["7r", "hate", "tun", "pos"]);
    // Not an artist query, or another artist's shelf: unchanged.
    assert_eq!(ids(&apply_artist_top_songs("7 rings", ranked.clone(), "Ariana Grande", &top)), ids(&ranked));
    assert_eq!(ids(&apply_artist_top_songs("ariana grande", ranked.clone(), "Ariana", &top)), ids(&ranked));
}
