//! The Liked Songs chip benchmark (lane magic-d3). Scores each source and combination against
//! the hand-labelled sample in `docs/bench/tags/labelled.tsv`, using the raw answers
//! `scripts/bench/tags_fetch.py` saved, through the shipped vocabulary and scoring code.
//!
//!     python3 -I scripts/bench/tags_fetch.py docs/bench/tags/labelled.tsv /tmp/raw.json [MSD_DIR]
//!     TAGS_BENCH_RAW=/tmp/raw.json cargo test -p encore-core --lib tags_bench -- --ignored --nocapture
//!
//! Metric: micro precision / recall over (track, genre chip) pairs; a track with no answer counts
//! its labels as misses. Thresholds are picked by F0.5 (a wrong song under a chip is worse than a
//! missing one) with two-fold cross-validation, so the reported numbers are on held-out tracks.

use super::*;
use serde_json::Value;

struct Row {
    genres: Vec<String>,
    moods: Vec<String>,
    mb_search: Option<RawTags>,
    mb_lookup_genres: Option<RawTags>,
    mb_lookup_tags: Option<RawTags>,
    mb_rec: Option<RawTags>,
    deezer: Option<Vec<String>>,
    lastfm: Option<RawTags>,
}

fn tags(v: &Value) -> RawTags {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| Some((t.get(0)?.as_str()?.to_owned(), t.get(1)?.as_f64()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn load(path: &str) -> Vec<Row> {
    let j: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let strs = |v: &Value| -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let mut rows: Vec<(String, Row)> = j
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, d)| {
            let artist = d["artist"].as_str().unwrap_or("");
            let cands: Vec<(String, i64, Vec<String>, RawTags)> = d["mb_artist"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            (
                                c["name"].as_str().unwrap_or("").to_owned(),
                                c["score"].as_i64().unwrap_or(0),
                                vec![],
                                tags(&c["tags"]),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let lk = &d["mb_lookup"];
            let lf = &d["lastfm"];
            (
                k.clone(),
                Row {
                    genres: strs(&d["genres"]),
                    moods: strs(&d["moods"]),
                    mb_search: mb_pick(artist, &cands),
                    mb_lookup_genres: lk.is_object().then(|| tags(&lk["genres"])),
                    mb_lookup_tags: lk.is_object().then(|| tags(&lk["tags"])),
                    mb_rec: d["mb_recording"].is_object().then(|| tags(&d["mb_recording"]["tags"])),
                    deezer: d["deezer"]["hit"].as_bool().unwrap_or(false).then(|| strs(&d["deezer"]["genres"])),
                    lastfm: lf["found"].as_bool().unwrap_or(false).then(|| tags(&lf["tags"])),
                },
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter().map(|(_, r)| r).collect()
}

#[derive(Default, Clone, Copy)]
struct Pr {
    tp: usize,
    fp: usize,
    fn_: usize,
    covered: usize,
    n: usize,
}
impl Pr {
    fn p(&self) -> f64 {
        self.tp as f64 / (self.tp + self.fp).max(1) as f64
    }
    fn r(&self) -> f64 {
        self.tp as f64 / (self.tp + self.fn_).max(1) as f64
    }
    fn f(&self, beta: f64) -> f64 {
        let (p, r) = (self.p(), self.r());
        if p + r == 0.0 {
            return 0.0;
        }
        (1.0 + beta * beta) * p * r / (beta * beta * p + r)
    }
}

type Method = fn(&Row, &Config) -> Vec<&'static str>;

fn genres_only(c: Vec<&'static str>) -> Vec<&'static str> {
    c.into_iter().filter(|c| kind_of(c) == Kind::Genre).collect()
}
fn one(t: &Option<RawTags>, cfg: &Config) -> Vec<&'static str> {
    t.as_ref().map(|t| chips_from(&score(t, cfg), cfg)).unwrap_or_default()
}
fn dz(r: &Row) -> Option<RawTags> {
    // Deezer genres carry no votes; weight them past any `min_votes`.
    r.deezer.as_ref().map(|d| d.iter().map(|g| (g.clone(), 100.0)).collect())
}

const METHODS: &[(&str, Method)] = &[
    ("MusicBrainz artist search, tags", |r, c| one(&r.mb_search, c)),
    ("MusicBrainz artist lookup, genres only", |r, c| one(&r.mb_lookup_genres, c)),
    ("MusicBrainz artist lookup, tags", |r, c| one(&r.mb_lookup_tags, c)),
    ("MusicBrainz recording tags, else artist", |r, c| {
        let rec = one(&r.mb_rec, c);
        if rec.iter().any(|x| kind_of(x) == Kind::Genre) { rec } else { one(&r.mb_search, c) }
    }),
    ("Deezer album genres", |r, c| one(&dz(r), c)),
    ("Last.fm track tags (MSD 2011)", |r, c| one(&r.lastfm, c)),
    ("MB search + Deezer fallback", |r, c| {
        track_chips(&TrackSources { mb: r.mb_search.clone(), lastfm: None, deezer: r.deezer.clone() }, c)
    }),
    ("MB search + Last.fm + Deezer fallback (shipped)", |r, c| {
        track_chips(&TrackSources { mb: r.mb_search.clone(), lastfm: r.lastfm.clone(), deezer: r.deezer.clone() }, c)
    }),
    ("MB search ∪ Deezer (union)", |r, c| {
        let mut a = one(&r.mb_search, c);
        for x in one(&dz(r), c) {
            if !a.contains(&x) {
                a.push(x);
            }
        }
        a
    }),
    ("Last.fm + MB fallback", |r, c| {
        let a = one(&r.lastfm, c);
        if a.iter().any(|x| kind_of(x) == Kind::Genre) { a } else { one(&r.mb_search, c) }
    }),
];

fn eval(rows: &[&Row], m: Method, cfg: &Config, moods: bool) -> Pr {
    let mut pr = Pr::default();
    for r in rows {
        let want: HashSet<&str> =
            if moods { r.moods.iter().map(String::as_str).collect() } else { r.genres.iter().map(String::as_str).collect() };
        if moods && want.is_empty() {
            continue;
        }
        let got: HashSet<&str> = m(r, cfg)
            .into_iter()
            .filter(|c| (kind_of(c) == Kind::Mood) == moods)
            .collect();
        pr.n += 1;
        pr.covered += !got.is_empty() as usize;
        pr.tp += got.intersection(&want).count();
        pr.fp += got.difference(&want).count();
        pr.fn_ += want.difference(&got).count();
    }
    pr
}

fn grid() -> Vec<Config> {
    let mut v = Vec::new();
    for &min_share in &[0.1, 0.15, 0.2, 0.25, 0.35, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0] {
        for &implied in &[0.0, 0.5, 1.0] {
            for &max_genres in &[1, 2, 3, 4] {
                for &min_votes in &[1.0, 2.0] {
                    v.push(Config { min_share, implied, max_genres, min_votes });
                }
            }
        }
    }
    v
}

fn best_on(rows: &[&Row], m: Method) -> Config {
    grid()
        .into_iter()
        .map(|c| (eval(rows, m, &c, false).f(0.5), c))
        .fold(None::<(f64, Config)>, |best, x| match best {
            Some(b) if b.0 >= x.0 => Some(b),
            _ => Some(x),
        })
        .unwrap()
        .1
}

#[test]
#[ignore]
fn tags_bench() {
    let Ok(path) = std::env::var("TAGS_BENCH_RAW") else {
        eprintln!("set TAGS_BENCH_RAW");
        return;
    };
    let rows = load(&path);
    let all: Vec<&Row> = rows.iter().collect();
    let a: Vec<&Row> = rows.iter().step_by(2).collect();
    let b: Vec<&Row> = rows.iter().skip(1).step_by(2).collect();
    let lf_rows: Vec<&Row> = rows.iter().filter(|r| r.lastfm.is_some()).collect();
    println!("{} labelled tracks ({} in the MSD Last.fm set)\n", rows.len(), lf_rows.len());
    println!("Genres, two-fold CV (tuned on one half, scored on the other), F0.5-selected:");
    println!("| Method | Precision | Recall | F1 | F0.5 | Has a genre |");
    println!("|---|---|---|---|---|---|");
    for (name, m) in METHODS {
        let (ca, cb) = (best_on(&a, *m), best_on(&b, *m));
        let (ea, eb) = (eval(&b, *m, &ca, false), eval(&a, *m, &cb, false));
        let pr = Pr {
            tp: ea.tp + eb.tp,
            fp: ea.fp + eb.fp,
            fn_: ea.fn_ + eb.fn_,
            covered: ea.covered + eb.covered,
            n: ea.n + eb.n,
        };
        println!(
            "| {name} | {:.3} | {:.3} | {:.3} | {:.3} | {}/{} |",
            pr.p(),
            pr.r(),
            pr.f(1.0),
            pr.f(0.5),
            pr.covered,
            pr.n
        );
    }
    println!("\nOn the {} tracks Last.fm (MSD) knows, same protocol:", lf_rows.len());
    let la: Vec<&Row> = lf_rows.iter().copied().step_by(2).collect();
    let lb: Vec<&Row> = lf_rows.iter().copied().skip(1).step_by(2).collect();
    for (name, m) in METHODS {
        let (ca, cb) = (best_on(&la, *m), best_on(&lb, *m));
        let (ea, eb) = (eval(&lb, *m, &ca, false), eval(&la, *m, &cb, false));
        let pr = Pr { tp: ea.tp + eb.tp, fp: ea.fp + eb.fp, fn_: ea.fn_ + eb.fn_, covered: ea.covered + eb.covered, n: ea.n + eb.n };
        println!("| {name} | {:.3} | {:.3} | {:.3} | {:.3} | {}/{} |", pr.p(), pr.r(), pr.f(1.0), pr.f(0.5), pr.covered, pr.n);
    }
    for (name, m) in METHODS.iter().filter(|(n, _)| n.contains("shipped") || n.contains("Deezer fallback")) {
        let c = best_on(&all, *m);
        let pr = eval(&all, *m, &c, false);
        println!("\nBest config on all rows for '{name}': {c:?} → P {:.3} R {:.3} F0.5 {:.3}", pr.p(), pr.r(), pr.f(0.5));
        let pr = eval(&all, *m, &Config::BEST, false);
        println!("Config::BEST on all rows: P {:.3} R {:.3} F1 {:.3} F0.5 {:.3}", pr.p(), pr.r(), pr.f(1.0), pr.f(0.5));
    }
    println!("\nMoods (tracks with a mood label), Config::BEST:");
    for (name, m) in METHODS {
        let pr = eval(&all, *m, &Config::BEST, true);
        println!("| {name} | {:.3} | {:.3} | {}/{} |", pr.p(), pr.r(), pr.covered, pr.n);
    }
    // Per-chip precision/recall of the shipped method.
    let ship = METHODS.iter().find(|(n, _)| n.contains("shipped")).unwrap().1;
    let mut per: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for r in &rows {
        let got: HashSet<&str> = genres_only(ship(r, &Config::BEST)).into_iter().collect();
        let want: HashSet<&str> = r.genres.iter().map(String::as_str).collect();
        for c in CHIPS.iter().filter(|c| c.1 == Kind::Genre).map(|c| c.0) {
            let e = per.entry(c).or_default();
            match (got.contains(c), want.contains(c)) {
                (true, true) => e.0 += 1,
                (true, false) => e.1 += 1,
                (false, true) => e.2 += 1,
                _ => {}
            }
        }
    }
    println!("\nPer chip (shipped, Config::BEST): chip tp/fp/fn");
    for (c, (tp, fp, fn_)) in per {
        if tp + fp + fn_ > 0 {
            println!("  {c:12} {tp:3} {fp:3} {fn_:3}");
        }
    }
}
