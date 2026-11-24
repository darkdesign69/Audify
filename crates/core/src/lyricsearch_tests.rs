//! Unit tests for `lyricsearch`. Every "lyric" here is an invented sentence: no song's words
//! appear in this file (stream: lyricsearch, copyright rule).

use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use innertube::SongItem;
use serde_json::json;

use super::*;

fn song(id: &str, title: &str, artists: &str, dur: &str) -> SongItem {
    SongItem {
        video_id: id.into(),
        title: title.into(),
        artists: artists.into(),
        duration: Some(dur.into()),
        ..SongItem::default()
    }
}

fn hit(title: &str, artist: &str) -> Hit {
    Hit { title: title.into(), artist: artist.into(), album: None, duration: None }
}

// --- the gate ----------------------------------------------------------------------------------

#[test]
fn invented_sentences_look_like_lyrics() {
    for q in [
        "the quick brown fox walks to the old blue harbour",
        "my neighbour paints his fence green every tuesday",
        "we parked the yellow tractor behind the bakery",
        "she forgot her umbrella at the station",
        "i left my keys on the kitchen table",
        "el gato duerme en la silla de la cocina",
        "je range les tasses dans le placard bleu",
        "ich habe den schlüssel unter der matte vergessen",
        "eu deixei o guarda chuva no carro",
        "il treno parte alle sette di mattina",
        "나는 도서관에 우산을 두고 왔어",
        "我把雨伞忘在图书馆里了",
        "図書館に傘を忘れてしまった",
    ] {
        assert!(looks_like_lyrics(q), "should look like a lyric line: {q}");
    }
}

#[test]
fn titles_artists_and_catalogue_queries_do_not() {
    for q in [
        "",
        "daft punk",
        "the weeknd",
        "blinding lights",
        "bohemian rhapsody queen",
        "despacito remix",
        "bad bunny live",
        "lofi hip hop playlist",
        "the joe rogan experience podcast",
        "taylor swift midnights album",
        "米津玄師",
        "https://open.spotify.com/track/abc",
        "artist name song title acoustic version",
    ] {
        assert!(!looks_like_lyrics(q), "should not look like a lyric line: {q}");
    }
}

/// The search benchmark's labelled queries (titles, artists, albums, podcasts, typos, moods) are
/// what ordinary searches look like; the gate must leave nearly all of them alone.
#[test]
fn gate_false_positives_on_the_search_benchmark() {
    let fx = crate::identity::search_bench::load();
    let n = fx.queries.len();
    let fp = fx.queries.iter().filter(|q| looks_like_lyrics(&q.q)).count();
    assert!(n >= 150);
    // 9/164 measured: four- and five-word titles that read as a sentence. Harmless: the lyric
    // search then finds that very song, and a strong title match keeps the top spot anyway.
    assert!(fp * 100 <= n * 6, "{fp}/{n} ordinary queries taken for lyrics");
}

// --- fusion ------------------------------------------------------------------------------------

#[test]
fn agreement_beats_a_single_first_row() {
    let lists = vec![
        (Source::Itunes, vec![hit("Harbour Song", "The Foxes"), hit("Blue Tuesday", "Fence Painters")]),
        (Source::Deezer, vec![hit("Other Thing", "Somebody"), hit("Blue Tuesday", "Fence Painters")]),
        (Source::Ytm, vec![hit("Blue Tuesday (Remastered 2011)", "Fence Painters")]),
    ];
    let w = fuse(&lists);
    assert_eq!(w[0].hit.title, "Blue Tuesday");
    assert_eq!(w[0].sources, vec![Source::Itunes, Source::Deezer, Source::Ytm]);
    assert!(w[0].score >= STRONG);
    assert_eq!(w[1].hit.title, "Harbour Song");
}

#[test]
fn fusion_keeps_the_known_length() {
    let mut with_len = hit("Blue Tuesday", "Fence Painters");
    with_len.duration = Some(201.0);
    with_len.album = Some("Paint".into());
    let w = fuse(&[
        (Source::Ytm, vec![hit("Blue Tuesday", "Fence Painters")]),
        (Source::Deezer, vec![with_len]),
    ]);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].hit.duration, Some(201.0));
    assert_eq!(w[0].hit.album.as_deref(), Some("Paint"));
}

#[test]
fn only_the_first_rows_vote() {
    let many: Vec<Hit> = (0..DEPTH + 5).map(|i| hit(&format!("Song {i}"), "X")).collect();
    assert_eq!(fuse(&[(Source::Deezer, many)]).len(), DEPTH);
}

// --- parsing -----------------------------------------------------------------------------------

#[test]
fn parses_deezer_and_itunes() {
    let d = json!({"data": [{"title": "Blue Tuesday", "duration": 201,
        "artist": {"name": "Fence Painters"}, "album": {"title": "Paint"}}]});
    let h = &parse_deezer(&d)[0];
    assert_eq!((h.title.as_str(), h.artist.as_str(), h.duration), ("Blue Tuesday", "Fence Painters", Some(201.0)));
    assert_eq!(h.album.as_deref(), Some("Paint"));
    let i = json!({"results": [{"trackName": "Blue Tuesday", "artistName": "Fence Painters",
        "collectionName": "Paint", "trackTimeMillis": 201500}]});
    let h = &parse_itunes(&i)[0];
    assert_eq!(h.duration, Some(201.5));
    assert!(parse_deezer(&json!({"error": {}})).is_empty());
}

// --- resolution --------------------------------------------------------------------------------

#[test]
fn best_row_skips_karaoke_and_other_artists() {
    let mut h = hit("Blue Tuesday", "Fence Painters");
    h.duration = Some(201.0);
    let rows = vec![
        song("k", "Blue Tuesday (Karaoke Version)", "Sing Along Stars", "3:21"),
        song("o", "Blue Tuesday", "Someone Else", "2:40"),
        song("r", "Blue Tuesday", "Fence Painters", "3:22"),
    ];
    assert_eq!(best_row(&h, &rows).unwrap().video_id, "r");
    assert!(best_row(&h, &rows[..2]).is_none());
}

struct FakeWeb {
    lists: Vec<(Source, Vec<Hit>, u64)>,
}

impl LyricSources for FakeWeb {
    fn sources<'a>(&'a self, _q: &'a str) -> Vec<BoxFuture<'a, (Source, Vec<Hit>)>> {
        self.lists
            .iter()
            .map(|(s, h, ms)| {
                let (s, h, ms) = (*s, h.clone(), *ms);
                async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    (s, h)
                }
                .boxed()
            })
            .collect()
    }
}

struct FakeYtm {
    /// (query contains, rows, delay ms)
    answers: Vec<(&'static str, Vec<SongItem>, u64)>,
}

impl SongSearch for FakeYtm {
    fn songs<'a>(&'a self, query: &'a str) -> BoxFuture<'a, Vec<SongItem>> {
        let a = self.answers.iter().find(|(k, _, _)| query.contains(k)).cloned();
        async move {
            match a {
                Some((_, rows, ms)) => {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    rows
                }
                None => Vec::new(),
            }
        }
        .boxed()
    }
}

const Q: &str = "we parked the yellow tractor behind the bakery";

#[tokio::test(start_paused = true)]
async fn finds_and_resolves_the_agreed_song() {
    let web = FakeWeb {
        lists: vec![
            (Source::Itunes, vec![hit("Blue Tuesday", "Fence Painters")], 300),
            (Source::Deezer, vec![hit("Blue Tuesday", "Fence Painters")], 200),
        ],
    };
    let ytm = FakeYtm {
        answers: vec![
            (Q, vec![song("x", "Unrelated", "Nobody", "3:00")], 400),
            ("Blue Tuesday Fence Painters", vec![song("r", "Blue Tuesday", "Fence Painters", "3:22")], 300),
        ],
    };
    let t = tokio::time::Instant::now();
    let out = search_with(Q, &web, &ytm, DEADLINE).await;
    assert_eq!(out[0].song.video_id, "r");
    assert_eq!(out[0].sources, vec![Source::Deezer, Source::Itunes]);
    assert!(t.elapsed() <= DEADLINE);
}

#[tokio::test(start_paused = true)]
async fn ytm_rows_resolve_without_a_second_search() {
    let web = FakeWeb { lists: vec![(Source::Itunes, vec![hit("Blue Tuesday", "Fence Painters")], 100)] };
    // Only the lyric query answers; a title search would find nothing.
    let ytm = FakeYtm { answers: vec![(Q, vec![song("r", "Blue Tuesday", "Fence Painters", "3:22")], 200)] };
    let out = search_with(Q, &web, &ytm, DEADLINE).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].song.video_id, "r");
    assert!(out[0].sources.contains(&Source::Ytm));
}

#[tokio::test(start_paused = true)]
async fn hard_deadline_holds_when_everything_is_slow() {
    let web = FakeWeb {
        lists: vec![
            (Source::Itunes, vec![hit("Blue Tuesday", "Fence Painters")], 5_000),
            (Source::Deezer, vec![hit("Blue Tuesday", "Fence Painters")], 800),
        ],
    };
    // The resolution search hangs.
    let ytm = FakeYtm {
        answers: vec![("Blue Tuesday", vec![song("r", "Blue Tuesday", "Fence Painters", "3:22")], 10_000)],
    };
    let t = tokio::time::Instant::now();
    let out = search_with(Q, &web, &ytm, DEADLINE).await;
    assert!(t.elapsed() <= DEADLINE + Duration::from_millis(1), "took {:?}", t.elapsed());
    assert!(out.is_empty());
}

#[tokio::test(start_paused = true)]
async fn nothing_found_is_empty() {
    let web = FakeWeb { lists: vec![(Source::Deezer, vec![], 100)] };
    let ytm = FakeYtm { answers: vec![] };
    assert!(search_with(Q, &web, &ytm, DEADLINE).await.is_empty());
    assert!(search_with("   ", &web, &ytm, DEADLINE).await.is_empty());
}

/// The gate on the benchmark's generated lyric queries (positives) and on title / artist strings
/// (negatives), both read from the gitignored scratch directory the benchmark scripts fill
/// (LYRICSEARCH_DATA). Prints rates only, never a query.
#[test]
#[ignore = "needs the benchmark's generated data (docs/bench/lyricsearch.md)"]
fn gate_validation() {
    let dir = std::env::var("LYRICSEARCH_DATA").expect("LYRICSEARCH_DATA");
    let read = |f: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{f}")).unwrap()).unwrap()
    };
    let qs = read("queries.json");
    let qs = qs.as_array().unwrap();
    let mut by: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for q in qs {
        let text = q["q"].as_str().unwrap();
        let yes = looks_like_lyrics(text) as usize;
        let n = text.split_whitespace().count();
        let wc = if n <= 1 { "cjk".to_string() } else { format!("{n:02} words") };
        for k in ["all".to_string(), format!("kind {}", q["kind"].as_str().unwrap()), wc, format!("lang {}", q["lang"].as_str().unwrap())] {
            let e = by.entry(k).or_default();
            e.0 += yes;
            e.1 += 1;
        }
    }
    for (k, (y, n)) in &by {
        eprintln!("recall {k:12} {y:3}/{n:3} = {:.3}", *y as f64 / *n as f64);
    }
    let neg = read("negatives.json");
    let neg: Vec<&str> = neg.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    let fp = neg.iter().filter(|q| looks_like_lyrics(q)).count();
    eprintln!("false positives on title/artist strings: {fp}/{} = {:.3}", neg.len(), fp as f64 / neg.len() as f64);
    let fx = crate::identity::search_bench::load();
    let fp = fx.queries.iter().filter(|q| looks_like_lyrics(&q.q)).count();
    eprintln!("false positives on search_bench queries: {fp}/{}", fx.queries.len());
}

/// End to end against the live services: every generated query (LYRICSEARCH_DATA/queries.json)
/// through [`search_with`], one pass per deadline. Writes the candidates' titles, artists and ids per
/// query index to `live_<deadline>.json` next to the input, for the benchmark script to score;
/// prints nothing but progress counts. LYRICSEARCH_LIMIT / LYRICSEARCH_PAUSE_MS pace it (iTunes
/// allows about 20 searches a minute).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits Deezer, iTunes and YouTube Music live (paced)"]
async fn live_bench() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let dir = std::env::var("LYRICSEARCH_DATA").expect("LYRICSEARCH_DATA");
    let qs: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/queries.json")).unwrap())
            .unwrap();
    let qs = qs.as_array().unwrap();
    let limit: usize = std::env::var("LYRICSEARCH_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(qs.len());
    let pause: u64 = std::env::var("LYRICSEARCH_PAUSE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3500);
    let deadlines: Vec<u64> = std::env::var("LYRICSEARCH_DEADLINES")
        .unwrap_or_else(|_| "1200,2500".into())
        .split(',')
        .map(|d| d.parse().unwrap())
        .collect();
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let ytm = Ytm { it: &it, client: clients.get(METADATA_CLIENT).unwrap() };
    // A stride through the list so a limit still covers every song and kind.
    let step = (qs.len() as f64 / limit as f64).max(1.0);
    let picks: Vec<usize> = (0..limit).map(|k| (k as f64 * step) as usize).filter(|i| *i < qs.len()).collect();
    let mut out: std::collections::HashMap<u64, Vec<serde_json::Value>> = Default::default();
    for &d in &deadlines {
        for (n, &i) in picks.iter().enumerate() {
            let t = std::time::Instant::now();
            let c = search_with(qs[i]["q"].as_str().unwrap(), &Web, &ytm, Duration::from_millis(d)).await;
            let ms = t.elapsed().as_millis() as u64;
            let cands: Vec<serde_json::Value> = c
                .iter()
                .map(|c| json!([c.song.title, c.song.artists, c.song.video_id, c.score, c.sources]))
                .collect();
            out.entry(d).or_default().push(json!({"i": i, "ms": ms, "cands": cands}));
            tokio::time::sleep(Duration::from_millis(pause)).await;
            if n % 20 == 0 {
                eprintln!("deadline {d}: {n}/{}", picks.len());
                std::fs::write(format!("{dir}/live_{d}.json"), serde_json::to_string(&out[&d]).unwrap()).unwrap();
            }
        }
    }
    for (d, rows) in &out {
        std::fs::write(format!("{dir}/live_{d}.json"), serde_json::to_string(rows).unwrap()).unwrap();
    }
}

