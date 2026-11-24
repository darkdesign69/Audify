//! Search benchmark, round 2 (stream: search2, docs/bench/search2.md).
//!
//! `tests/fixtures/smart/search2_bench.json`: 87 labelled queries (`search2_queries.json`), with
//! every YouTube Music / iTunes answer the pipeline asked for while answering them live:
//! - **person → podcast** (19): "chris williamson" → Modern Wisdom, "steven bartlett" → The Diary
//!   Of A CEO, …;
//! - artist names (10), partial (6) and typo (6) queries;
//! - **hard** (46): heavy typos, half words, transliterations, and 20 gibberish / keyboard mashes
//!   with no intended answer.
//!
//! The answers are replayed through [`crate::search::resolve`] (the app's own fallback chain) by
//! [`Replay`], so the benchmark scores the shipped code offline.
//!
//! - **Before** = the pipeline as it was: `search_all` + the song search, the podtop show rule
//!   over the merged (unranked) show list, Podcasts shelf last, no fallback.
//! - **After** = `search::resolve` + `podcasts::search::rank_shows` + the new `top_show` +
//!   `search::all_view_order`.
//!
//! Metrics over the All view top to bottom ([`crate::search::all_view_order`]): top-1, MRR and
//! Recall@5 of the labelled entity; zero-result rate.

use std::collections::HashMap;
use std::sync::Mutex;

use innertube::search::{Page, SpellFix};
use innertube::{BrowseItem, SongItem};
use serde::{Deserialize, Serialize};

use crate::identity::search_bench::{same_entity, show_card, ArtistTop, BenchQuery, Captured, Entity};
use crate::podcasts::Show;
use crate::search::{self, AllAnswer, Backend, SearchOverview, ViewRow};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search2_bench.json");
const QUERIES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search2_queries.json");
/// stream: search-pop. 36 partial / prefix queries whose answer is the obvious popular show or
/// artist ("tucker" → The Tucker Carlson Show), recorded the same way, plus Apple's chart.
const POP_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search_pop_bench.json");
const POP_QUERIES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search_pop_queries.json");

/// The set `live_capture` records: `SEARCH2_SET=pop` for the search-pop set.
fn capture_paths() -> (&'static str, &'static str) {
    if std::env::var("SEARCH2_SET").as_deref() == Ok("pop") { (POP_QUERIES, POP_FIXTURE) } else { (QUERIES, FIXTURE) }
}

/// The answers to one query string (the typed one, or one the chain asked for).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Answers {
    #[serde(default)]
    pub all: Option<Captured>,
    #[serde(default)]
    pub all_fix: Option<SpellFix>,
    #[serde(default)]
    pub artist_top: Option<ArtistTop>,
    #[serde(default)]
    pub songs: Option<Vec<SongItem>>,
    #[serde(default)]
    pub songs_fix: Option<SpellFix>,
    #[serde(default)]
    pub suggest: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Q2 {
    pub q: String,
    pub cat: String,
    #[serde(default)]
    pub want: Option<Entity>,
    #[serde(default)]
    pub note: Option<String>,
    /// Keyed by the query string asked.
    #[serde(default)]
    pub answers: HashMap<String, Answers>,
    /// YouTube Music's Podcasts filter and the iTunes directory, raw, for the typed query.
    #[serde(default)]
    pub yt_shows: Vec<Show>,
    #[serde(default)]
    pub dir_shows: Vec<Show>,
    /// `search::search_lyrics` for the typed query, where the gate let it run.
    #[serde(default)]
    pub lyrics: Option<Vec<search::LyricMatch>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Fixture2 {
    pub queries: Vec<Q2>,
    /// "Try these" for a fresh install (no history): YouTube's chart, recorded once.
    #[serde(default)]
    pub trending: Vec<SongItem>,
    /// stream: search-pop. Apple's US Top Shows chart (`podcasts::charts`), recorded once; every
    /// set is replayed with it.
    #[serde(default)]
    pub chart: Vec<crate::podcasts::charts::ChartRow>,
}

fn load() -> Fixture2 {
    serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("search2 fixture")).expect("json")
}

pub fn load_pop() -> Fixture2 {
    serde_json::from_str(&std::fs::read_to_string(POP_FIXTURE).expect("search-pop fixture")).expect("json")
}

/// The recorded chart (in the search-pop fixture), for replaying any set.
pub fn chart() -> Vec<crate::podcasts::charts::ChartRow> {
    std::fs::read_to_string(POP_FIXTURE)
        .ok()
        .and_then(|s| serde_json::from_str::<Fixture2>(&s).ok())
        .map(|f| f.chart)
        .unwrap_or_default()
}

// ─── replay ──────────────────────────────────────────────────────────────────────────────────────

/// The recorded answers through the same pure re-rankers the app applies (`identity`).
pub struct Replay<'a>(pub &'a Q2);

impl Backend for Replay<'_> {
    async fn all(&self, q: &str) -> Result<AllAnswer, String> {
        let a = self.0.answers.get(q).and_then(|a| a.all.as_ref().map(|c| (a, c)));
        let Some((a, c)) = a else { return Ok(AllAnswer::default()) };
        Ok(AllAnswer {
            results: crate::identity::rerank_search_all(q, c.results()),
            fix: a.all_fix.clone(),
            artist_top: a.artist_top.as_ref().map(|t| (t.name.clone(), t.songs.clone())),
        })
    }
    async fn songs(&self, q: &str) -> Result<Page<SongItem>, String> {
        let Some(a) = self.0.answers.get(q) else { return Ok(Page::default()) };
        let items = crate::identity::rerank_search_songs(q, a.songs.clone().unwrap_or_default());
        Ok(Page { items, continuation: None, spell_fix: a.songs_fix.clone() })
    }
    async fn suggest(&self, q: &str) -> Result<Vec<String>, String> {
        Ok(self.0.answers.get(q).and_then(|a| a.suggest.clone()).unwrap_or_default())
    }
}

// ─── live capture ────────────────────────────────────────────────────────────────────────────────

/// The live requests, as `search::Live` makes them, recording every answer.
struct Recorder<'a> {
    it: &'a innertube::InnerTube,
    c: &'a innertube::YouTubeClient,
    log: Mutex<HashMap<String, Answers>>,
}

impl Backend for Recorder<'_> {
    async fn all(&self, q: &str) -> Result<AllAnswer, String> {
        let (r, fix) = self.it.search_all_fixed(self.c, q, false).await.map_err(|e| e.to_string())?;
        let ranked = crate::identity::rerank_search_all(q, r.clone());
        let mut top = None;
        if let Some(id) = search::artist_card(q, &ranked) {
            if let Ok(mut p) = self.it.artist(self.c, &id).await {
                crate::identity::dedup_songs(&mut p.top_songs);
                top = Some(ArtistTop { id, name: p.name.clone().unwrap_or_default(), songs: p.top_songs });
            }
        }
        {
            let mut log = self.log.lock().unwrap();
            let e = log.entry(q.to_owned()).or_default();
            e.all = Some(Captured::of_results(&r));
            e.all_fix = fix.clone();
            e.artist_top = top.clone();
        }
        Ok(AllAnswer { results: ranked, fix, artist_top: top.map(|t| (t.name, t.songs)) })
    }
    async fn songs(&self, q: &str) -> Result<Page<SongItem>, String> {
        let p = self
            .it
            .search_tracks_page(self.c, q, innertube::SearchFilter::Songs, None)
            .await
            .map_err(|e| e.to_string())?;
        {
            let mut log = self.log.lock().unwrap();
            let e = log.entry(q.to_owned()).or_default();
            e.songs = Some(p.items.clone());
            e.songs_fix = p.spell_fix.clone();
        }
        Ok(Page { items: crate::identity::rerank_search_songs(q, p.items), ..p })
    }
    async fn suggest(&self, q: &str) -> Result<Vec<String>, String> {
        let s = self.it.search_suggestions(self.c, q).await.map_err(|e| e.to_string())?;
        let out: Vec<String> = s.queries.into_iter().filter(|x| !x.history).map(|x| x.text).collect();
        self.log.lock().unwrap().entry(q.to_owned()).or_default().suggest = Some(out.clone());
        Ok(out)
    }
}

/// Re-records every query of `search2_queries.json` (SEARCH2_ONLY=<q> for one). Also asks the
/// typeahead for every query, so a chain change can be replayed without a recapture.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live YouTube Music and iTunes (paced)"]
async fn live_capture() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();
    let (queries_path, fixture_path) = capture_paths();
    let queries: Vec<Q2> = serde_json::from_str(&std::fs::read_to_string(queries_path).unwrap()).unwrap();
    let mut fx = std::fs::read_to_string(fixture_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Fixture2>(&s).ok())
        .unwrap_or_default();
    let only = std::env::var("SEARCH2_ONLY").ok();
    let http = reqwest::Client::new();
    for spec in queries {
        if only.as_deref().is_some_and(|o| o != spec.q) {
            continue;
        }
        if only.is_none() && fx.queries.iter().any(|x| x.q == spec.q && !x.answers.is_empty()) {
            continue;
        }
        let rec = Recorder { it: &it, c, log: Mutex::new(HashMap::new()) };
        let r = search::resolve(&rec, &spec.q).await;
        let _ = rec.suggest(&spec.q).await;
        let yt = crate::podcasts::youtube::search_shows(&it, c, &spec.q).await.unwrap_or_default();
        let url = format!(
            "https://itunes.apple.com/search?media=podcast&entity=podcast&limit=25&country=US&term={}",
            urlencoding::encode(&spec.q)
        );
        let dir = match http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => resp
                .text()
                .await
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .map(|v| crate::podcasts::itunes::parse_shows(&v))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        eprintln!(
            "{:>24} tried {:?} songs {} yt_shows {} dir {}",
            spec.q,
            r.as_ref().map(|r| r.tried.clone()).unwrap_or_default(),
            r.as_ref().map(|r| r.songs.len()).unwrap_or(0),
            yt.len(),
            dir.len()
        );
        let q2 = Q2 { answers: rec.log.into_inner().unwrap(), yt_shows: strip(yt), dir_shows: strip(dir), ..spec };
        fx.queries.retain(|x| x.q != q2.q);
        fx.queries.push(q2);
        std::fs::write(fixture_path, serde_json::to_string(&fx).unwrap()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(3200)).await;
    }
}

/// Descriptions and art aren't scored; keep the fixture small.
fn strip(mut v: Vec<Show>) -> Vec<Show> {
    for s in &mut v {
        s.description = None;
        s.thumbnail = None;
    }
    v
}

// ─── scoring ─────────────────────────────────────────────────────────────────────────────────────

fn song_card(s: &SongItem) -> BrowseItem {
    let mut c = crate::identity::search_bench::Card {
        kind: "song".into(),
        id: s.video_id.clone(),
        title: s.title.clone(),
        subtitle: Some(format!("Song • {}", s.artists)),
        ..Default::default()
    }
    .item();
    c.is_video = s.is_video;
    c
}

fn row_card(r: &ViewRow) -> BrowseItem {
    match r {
        ViewRow::Card(c) => (*c).clone(),
        ViewRow::Song(s) => song_card(s),
        ViewRow::Show(s) => show_card(s),
    }
}

fn bq_of(q: &Q2) -> BenchQuery {
    BenchQuery { q: q.q.clone(), cat: q.cat.clone(), note: q.note.clone(), ..Default::default() }
}

/// The page as it was before this stream (see the module docs).
pub fn before(q: &Q2) -> SearchOverview {
    let a = q.answers.get(&q.q).cloned().unwrap_or_default();
    let results = crate::identity::rerank_search_all(&q.q, a.all.clone().unwrap_or_default().results());
    let mut songs = crate::identity::rerank_search_songs(&q.q, a.songs.clone().unwrap_or_default());
    if let Some(t) = &a.artist_top {
        songs = crate::identity::apply_artist_top_songs(&q.q, songs, &t.name, &t.songs);
    }
    let shows = crate::podcasts::search::merge_shows(q.yt_shows.clone(), q.dir_shows.clone());
    let top_show = podtop_top_show(&q.q, results.top.first(), results.artists.first(), &shows);
    SearchOverview { query: q.q.clone(), results, songs, shows, top_show, ..Default::default() }
}

/// The app's pipeline now, replayed. `trending` is "Try these" on a fresh install (the library
/// step finds nothing there).
pub fn after_with(q: &Q2, trending: &[SongItem]) -> SearchOverview {
    let mut ov = after(q);
    if search::all_view_order(&ov).is_empty() {
        ov.try_these = Some(search::TryThese { songs: trending.to_vec(), artists: Vec::new() });
    }
    ov
}

/// The network steps only (no library, no "Try these").
pub fn after(q: &Q2) -> SearchOverview {
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let r = rt.block_on(search::resolve(&Replay(q), &q.q)).unwrap_or_default();
    let mut merged = crate::podcasts::search::merge_shows(q.yt_shows.clone(), q.dir_shows.clone());
    crate::podcasts::charts::apply(&mut merged, replay_chart());
    let shows = crate::podcasts::search::rank_shows(&q.q, merged);
    let top_show =
        crate::podcasts::search::top_show(&r.query, r.results.top.first(), r.results.artists.first(), &shows);
    let podcasts_first = search::podcasts_first(&r.query, top_show, &shows, &r.results);
    // Lyrics on: the late blend, before the user touches the list; a strong lead becomes the Top
    // result (unless a show already is), as the search page does.
    let mut results = r.results;
    let mut songs = r.songs;
    if let Some(l) = q.lyrics.clone().filter(|l| !l.is_empty()) {
        let card = results.top.first().filter(|t| t.kind != "song").map(|t| t.title.clone());
        let b = search::blend_lyrics(&q.q, songs, l, false, card.as_deref());
        songs = b.songs;
        if b.lead_top && top_show.is_none() {
            results.top.insert(0, song_card(&songs[0]));
        }
    }
    SearchOverview {
        query: r.query,
        correction: r.correction,
        results,
        songs,
        shows,
        top_show,
        podcasts_first,
        ..Default::default()
    }
}

/// The recorded chart, loaded once.
fn replay_chart() -> &'static [crate::podcasts::charts::ChartRow] {
    static C: std::sync::OnceLock<Vec<crate::podcasts::charts::ChartRow>> = std::sync::OnceLock::new();
    // `SEARCH_POP_NO_CHART=1`: the ablation without Apple's chart.
    C.get_or_init(|| if std::env::var("SEARCH_POP_NO_CHART").is_ok() { Vec::new() } else { chart() })
}

/// podtop's rule, verbatim, for the "before" column.
fn podtop_top_show(query: &str, yt_card: Option<&BrowseItem>, artist: Option<&BrowseItem>, shows: &[Show]) -> Option<usize> {
    use crate::podcasts::search::{established, norm, show_match};
    if yt_card.is_some() {
        return None;
    }
    let artist_is_query = artist.is_some_and(|a| {
        crate::identity::norm(&a.title) == crate::identity::norm(query) || norm(&a.title) == norm(query)
    });
    shows
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let (m, est) = (show_match(query, s), established(s));
            let ok = m > 0 && (if artist_is_query { est } else { m == 2 || est });
            ok.then_some((i, m, est))
        })
        .max_by_key(|&(i, m, est)| (m, est, std::cmp::Reverse(i)))
        .map(|(i, _, _)| i)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Score {
    pub n: usize,
    pub top1: f64,
    pub rr: f64,
    pub r5: f64,
    /// Queries (labelled or not) whose page had nothing at all.
    pub zero: usize,
    pub all: usize,
}

impl Score {
    fn line(&self, name: &str) -> String {
        let d = self.n.max(1) as f64;
        format!(
            "| {name} | {} | {:.3} | {:.3} | {:.3} | {}/{} ({:.1}%) |",
            self.n,
            self.top1 / d,
            self.rr / d,
            self.r5 / d,
            self.zero,
            self.all,
            100.0 * self.zero as f64 / self.all.max(1) as f64
        )
    }
    pub fn mrr(&self) -> f64 {
        self.rr / self.n.max(1) as f64
    }
    pub fn top1(&self) -> f64 {
        self.top1 / self.n.max(1) as f64
    }
    pub fn recall5(&self) -> f64 {
        self.r5 / self.n.max(1) as f64
    }
}

/// The rank (1-based) of the labelled entity on the page, top to bottom.
pub fn rank(q: &Q2, ov: &SearchOverview) -> Option<usize> {
    let want = q.want.as_ref()?;
    let bq = bq_of(q);
    let strict = q.cat.starts_with("pop_");
    search::all_view_order(ov)
        .iter()
        .map(row_card)
        .position(|c| if strict { matches_strict(&bq, want, &c) } else { matches_want(&bq, want, &c) })
        .map(|i| i + 1)
}

/// stream: search-pop. For the search-pop set, a show matches its label only when its title *is*
/// the label, or the label followed by a host or a tagline ("The Diary Of A CEO with Steven
/// Bartlett", "Armchair Expert with Dax Shepard | Podcast"); "Joe Rogan Experience Review" is not
/// The Joe Rogan Experience. Other kinds as `same_entity`.
fn matches_strict(bq: &BenchQuery, want: &Entity, c: &BrowseItem) -> bool {
    if want.kind != "podcast" {
        return same_entity(bq, want, c);
    }
    let podcast = c.id.starts_with("MPSP") || c.subtitle.as_deref().is_some_and(|s| s.starts_with("Podcast"));
    let low = |x: &str| {
        let l = x.trim().to_lowercase();
        l.strip_prefix("the ").map(str::to_owned).unwrap_or(l)
    };
    let (t, w) = (low(&c.title), low(&want.title));
    podcast
        && (t == w
            || t.strip_prefix(&w).is_some_and(|rest| {
                [" with ", " w/", " |", ":", " -", " (", " podcast"].iter().any(|p| rest.starts_with(p))
            }))
}

/// `same_entity`, plus: a show whose title holds the labelled title as whole words, or the other
/// way round ("The Diary Of A CEO with Steven Bartlett" is the show labelled "The Diary Of A CEO").
fn matches_want(bq: &BenchQuery, want: &Entity, c: &BrowseItem) -> bool {
    if same_entity(bq, want, c) {
        return true;
    }
    let pad = |x: &str| format!(" {} ", crate::podcasts::search::norm(x));
    want.kind == "podcast"
        && c.id.starts_with("MPSP") | c.subtitle.as_deref().is_some_and(|s| s.starts_with("Podcast"))
        && (pad(&c.title).contains(&pad(&want.title)) || pad(&want.title).contains(&pad(&c.title)))
}

pub fn score<'a>(qs: impl Iterator<Item = &'a Q2>, f: impl Fn(&Q2) -> SearchOverview) -> Score {
    let mut s = Score::default();
    for q in qs {
        let ov = f(q);
        s.all += 1;
        if search::all_view_order(&ov).is_empty() {
            s.zero += 1;
        }
        if q.want.is_none() {
            continue;
        }
        s.n += 1;
        if let Some(r) = rank(q, &ov) {
            s.top1 += (r == 1) as u8 as f64;
            s.rr += 1.0 / r as f64;
            s.r5 += (r <= 5) as u8 as f64;
        }
    }
    s
}

#[test]
fn fixture_bench2() {
    let fx = load();
    assert!(fx.queries.len() >= 80, "fixture has {} queries", fx.queries.len());
    let groups: [(&str, &[&str]); 6] = [
        ("person → podcast", &["person_podcast"]),
        ("artist", &["artist"]),
        ("partial + typo", &["partial", "typo"]),
        ("hard: typos, half words, transliterations", &["hard_typo", "hard_half", "hard_translit"]),
        ("hard: gibberish", &["hard_gibberish"]),
        ("all 87", &["person_podcast", "artist", "partial", "typo", "hard_typo", "hard_half", "hard_translit", "hard_gibberish"]),
    ];
    println!("| set | pipeline | labelled | top-1 | MRR | Recall@5 | zero-result |");
    let mut res = HashMap::new();
    for (name, cats) in groups {
        let full = |q: &Q2| after_with(q, &fx.trending);
        let pipelines: [(&str, &dyn Fn(&Q2) -> SearchOverview); 3] =
            [("before", &before), ("after, network steps", &after), ("after (app)", &full)];
        for (pname, f) in pipelines {
            let sc = score(fx.queries.iter().filter(|q| cats.contains(&q.cat.as_str())), f);
            println!("{}", sc.line(&format!("{name} | {pname}")));
            res.insert((name, pname), sc);
        }
    }
    if std::env::var("SEARCH_BENCH_VERBOSE").is_ok() {
        for q in &fx.queries {
            let (b, a) = (before(q), after(q));
            let first = |ov: &SearchOverview| {
                search::all_view_order(ov).first().map(|r| {
                    let c = row_card(r);
                    format!("{} {}", c.kind, c.title)
                })
            };
            println!(
                "[{}] {} | want {:?} | before #{:?} {:?} | after #{:?} {:?} via {:?}",
                q.cat,
                q.q,
                q.want.as_ref().map(|w| &w.title),
                rank(q, &b),
                first(&b),
                rank(q, &a),
                first(&a),
                a.correction.as_ref().map(|c| (c.kind, c.query.clone()))
            );
        }
    }
    let pp = res[&("person → podcast", "after (app)")];
    assert!(pp.top1() >= 0.9, "person → podcast top-1 {:.3}", pp.top1());
    assert!(!fx.trending.is_empty(), "record the chart: search2_bench::capture_trending");
    let hard_g = res[&("hard: gibberish", "after (app)")];
    let hard_t = res[&("hard: typos, half words, transliterations", "after (app)")];
    assert_eq!(hard_g.zero + hard_t.zero, 0, "a hard query had no results");
    let all_a = res[&("all 87", "after (app)")];
    let all_b = res[&("all 87", "before")];
    assert!(all_a.mrr() >= all_b.mrr() && all_a.top1() >= all_b.top1() && all_a.recall5() >= all_b.recall5());
    assert_eq!(all_a.zero, 0);
    // Floors from the run in docs/bench/search2.md.
    assert!(all_a.mrr() >= 0.92 && all_a.recall5() >= 0.95, "all: MRR {:.3} R@5 {:.3}", all_a.mrr(), all_a.recall5());
}

// Replay must see exactly the code's chain: Recorder and Replay agree on the queries asked.
#[test]
fn replay_answers_every_query_the_chain_asks() {
    let fx = load();
    for q in &fx.queries {
        let r = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(search::resolve(&Replay(q), &q.q))
            .unwrap();
        for t in &r.tried {
            assert!(q.answers.contains_key(t), "{}: the chain asked {t:?}, not recorded", q.q);
        }
    }
}

#[test]
fn debug_one() {
    let Ok(q) = std::env::var("S2_DEBUG") else { return };
    let fx = load();
    let x = fx.queries.iter().find(|y| y.q == q).unwrap();
    let ov = after(x);
    println!("top card {:?} artist {:?} top_show {:?}", ov.results.top.first().map(|c| &c.title), ov.results.artists.first().map(|c| &c.title), ov.top_show);
    for (i, s) in ov.shows.iter().enumerate().take(8) {
        println!("{i} {} | {} | eps {:?} yt {} feed {} | rel {} host {} est {}", s.title, s.author, s.episode_count, s.yt_browse_id.is_some(), s.feed_url.is_some(),
            crate::podcasts::search::show_relevance(&q, s, true), crate::podcasts::search::host_match(&q, s), crate::podcasts::search::established(s));
    }
}

/// Records YouTube's chart once, for "Try these" on a fresh install.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live YouTube Music"]
async fn capture_trending() {
    use innertube::{Clients, InnerTube, Session, METADATA_CLIENT};
    let vd = InnerTube::new(Session::default(), None).unwrap().fetch_visitor_data().await.ok();
    let it = InnerTube::new(Session { visitor_data: vd, ..Session::default() }, None).unwrap();
    let clients = Clients::bundled();
    let c = clients.get(METADATA_CLIENT).unwrap();
    let page = it.charts(c, Some("US")).await.unwrap();
    let pl = page.highlights.top_songs.as_ref().or(page.highlights.trending.as_ref()).expect("a songs chart");
    let mut fx = load();
    fx.trending = it.playlist(c, &pl.id, None).await.unwrap().items.into_iter().take(10).collect();
    assert!(!fx.trending.is_empty(), "no songs on the chart");
    std::fs::write(FIXTURE, serde_json::to_string(&fx).unwrap()).unwrap();
}

#[test]
fn lyric_gate_counts() {
    if std::env::var("S2_GATE").is_err() {
        return;
    }
    let a: Vec<String> = load().queries.into_iter().map(|q| q.q).collect();
    let b: Vec<String> = crate::identity::search_bench::load().queries.into_iter().map(|q| q.q).collect();
    for (name, qs) in [("search2", a), ("search", b)] {
        let hit: Vec<&String> = qs.iter().filter(|q| crate::lyricsearch::looks_like_lyrics(q)).collect();
        println!("{name}: {}/{} gated: {hit:?}", hit.len(), qs.len());
    }
}

/// Records `search::search_lyrics` (the shipped call, on a throwaway core) for every query of both
/// benchmarks that passes `looks_like_lyrics`; the rest stay `None` (the app runs nothing for them).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live iTunes, Deezer and YouTube Music (paced)"]
async fn capture_lyrics() {
    use std::sync::Arc;
    let dir = std::env::temp_dir().join(format!("spotless-s2lyr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let host: Arc<dyn crate::host::Host> = Arc::new(crate::host::NullHost(dir.clone()));
    let st = crate::boot::start_core(host).map_err(|e| e.to_string()).unwrap().state.clone();
    let mut fx = load();
    for q in fx.queries.iter_mut().filter(|q| crate::lyricsearch::looks_like_lyrics(&q.q)) {
        q.lyrics = Some(search::search_lyrics(&st, q.q.clone()).await);
        eprintln!("search2 {}: {:?}", q.q, q.lyrics.as_ref().map(|l| l.iter().map(|m| (m.song.title.clone(), m.score)).collect::<Vec<_>>()));
        tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    }
    std::fs::write(FIXTURE, serde_json::to_string(&fx).unwrap()).unwrap();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/smart/search_bench.json");
    let mut old = crate::identity::search_bench::load();
    for q in old.queries.iter_mut().filter(|q| crate::lyricsearch::looks_like_lyrics(&q.q)) {
        q.lyrics = Some(search::search_lyrics(&st, q.q.clone()).await);
        eprintln!("search {}: {:?}", q.q, q.lyrics.as_ref().map(|l| l.iter().map(|m| (m.song.title.clone(), m.score)).collect::<Vec<_>>()));
        tokio::time::sleep(std::time::Duration::from_millis(3500)).await;
    }
    std::fs::write(path, serde_json::to_string(&old).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── stream: search-pop ──────────────────────────────────────────────────────────────────────────

/// Records Apple's US Top Shows chart (200) into the search-pop fixture.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits live iTunes"]
async fn capture_chart() {
    let url = "https://itunes.apple.com/us/rss/toppodcasts/limit=200/json";
    let v: serde_json::Value = reqwest::get(url).await.unwrap().json().await.unwrap();
    let mut fx = std::fs::read_to_string(POP_FIXTURE)
        .ok()
        .and_then(|s| serde_json::from_str::<Fixture2>(&s).ok())
        .unwrap_or_default();
    fx.chart = crate::podcasts::charts::parse(&v);
    assert!(fx.chart.len() >= 100, "chart has {} rows", fx.chart.len());
    std::fs::write(POP_FIXTURE, serde_json::to_string(&fx).unwrap()).unwrap();
}

/// The Songs list's first row is by the labelled artist (an artist query), or the rank of the
/// labelled song in it (a song query): whether the list leads with what the query means.
fn songs_rank(q: &Q2, ov: &SearchOverview) -> Option<usize> {
    let want = q.want.as_ref()?;
    let bq = bq_of(q);
    ov.songs.iter().take(20).position(|s| match want.kind.as_str() {
        "artist" => crate::identity::search_bench::same_artist(&want.title, &s.artists, &[]),
        "song" => same_entity(&bq, want, &song_card(s)),
        _ => false,
    }).map(|i| i + 1)
}

/// The search-pop set: partial / prefix queries for popular shows and artists. Also prints every
/// set's scores so one run gives the whole table (docs/bench/search-pop.md).
#[test]
fn fixture_bench_pop() {
    let fx = load_pop();
    assert!(fx.queries.len() >= 30, "search-pop fixture has {} queries", fx.queries.len());
    assert!(fx.chart.len() >= 100, "record the chart: search2_bench::capture_chart");
    let full = |q: &Q2| after_with(q, &fx.trending);
    println!("| set | labelled | top-1 | MRR | Recall@5 | zero-result |");
    let mut res = HashMap::new();
    for (name, cats) in [("pop: podcasts", &["pop_podcast"][..]), ("pop: music", &["pop_music"][..]), ("pop: all", &["pop_podcast", "pop_music"][..])] {
        let sc = score(fx.queries.iter().filter(|q| cats.contains(&q.cat.as_str())), full);
        println!("{}", sc.line(name));
        res.insert(name, sc);
    }
    // The Songs list on the music queries: MRR of the labelled song / first row by the artist.
    let (mut n, mut rr, mut top1) = (0usize, 0.0, 0.0);
    for q in fx.queries.iter().filter(|q| q.cat == "pop_music") {
        n += 1;
        if let Some(r) = songs_rank(q, &full(q)) {
            rr += 1.0 / r as f64;
            top1 += f64::from(u8::from(r == 1));
        }
    }
    println!("| pop: music, Songs list (labelled artist / song) | {n} | {:.3} | {:.3} | | |", top1 / n as f64, rr / n as f64);
    if std::env::var("SEARCH_BENCH_VERBOSE").is_ok() {
        for q in &fx.queries {
            let ov = full(q);
            let first = search::all_view_order(&ov).first().map(|r| {
                let c = row_card(r);
                format!("{} {}", c.kind, c.title)
            });
            println!("[{}] {} | want {:?} | #{:?} first {:?} | songs #{:?} first song {:?} | shows {:?}", q.cat, q.q,
                q.want.as_ref().map(|w| &w.title), rank(q, &ov), first, songs_rank(q, &ov),
                ov.songs.first().map(|s| format!("{} - {}", s.title, s.artists)),
                ov.shows.iter().take(3).map(|s| format!("{} ({:?}, {:?})", s.title, s.chart_rank, s.episode_count)).collect::<Vec<_>>());
        }
    }
    // Latency: the ranking work this stream added runs on data already fetched (merge, chart
    // marks, scores, Top show, Songs list). Per query, over the whole set, release or debug as built.
    let rounds = 20;
    let time = |f: &dyn Fn(&Q2)| {
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            fx.queries.iter().for_each(f);
        }
        t.elapsed().as_micros() as f64 / (rounds * fx.queries.len()) as f64
    };
    let shows_new = time(&|q| {
        let mut merged = crate::podcasts::search::merge_shows(q.yt_shows.clone(), q.dir_shows.clone());
        crate::podcasts::charts::apply(&mut merged, &fx.chart);
        let shows = crate::podcasts::search::rank_shows(&q.q, merged);
        let a = q.answers.get(&q.q).and_then(|a| a.all.clone()).unwrap_or_default().results();
        let top = crate::podcasts::search::top_show(&q.q, a.top.first(), a.artists.first(), &shows);
        std::hint::black_box(search::podcasts_first(&q.q, top, &shows, &a));
    });
    let songs_old = time(&|q| {
        let a = q.answers.get(&q.q).cloned().unwrap_or_default();
        let songs = crate::identity::rerank_search_songs(&q.q, a.songs.clone().unwrap_or_default());
        std::hint::black_box(match a.artist_top.as_ref() { Some(t) => crate::identity::apply_artist_top_songs(&q.q, songs, &t.name, &t.songs), None => songs });
    });
    let results: HashMap<String, innertube::SearchResults> =
        fx.queries.iter().map(|q| (q.q.clone(), q.answers.get(&q.q).and_then(|a| a.all.clone()).unwrap_or_default().results())).collect();
    let songs_new = time(&|q| {
        let a = q.answers.get(&q.q).cloned().unwrap_or_default();
        let r = &results[&q.q];
        let songs = crate::identity::rerank_search_songs(&q.q, a.songs.clone().unwrap_or_default());
        std::hint::black_box(search::songs_for(&q.q, songs, r, a.artist_top.as_ref().map(|t| (t.name.clone(), t.songs.clone())).as_ref()));
    });
    println!("| CPU per query: shows (merge, chart, scores, Top show, shelf) | {shows_new:.0} µs | songs list before / after | {songs_old:.0} µs | {songs_new:.0} µs | |");
    let all = res["pop: all"];
    assert_eq!(all.zero, 0);
    // Floors from the run in docs/bench/search-pop.md.
    assert!(all.top1() >= POP_FLOOR_TOP1 && all.mrr() >= POP_FLOOR_MRR && all.recall5() >= POP_FLOOR_R5,
        "pop: top-1 {:.3} MRR {:.3} R@5 {:.3}", all.top1(), all.mrr(), all.recall5());
}

const POP_FLOOR_TOP1: f64 = 0.85;
const POP_FLOOR_MRR: f64 = 0.92;
const POP_FLOOR_R5: f64 = 0.97;
