//! An artist's discography in Spotify's terms, for both apps (the desktop's artist page and Show
//! all page, the iPhone's Discography block and page). Pure: the apps fetch the release lists
//! (the artist page's `albums` / `singles` shelves, or every release behind their "More" grids)
//! and the top songs, and `build` answers every chip's list.
//!
//! What YouTube gives: the "Albums" and "Singles & EPs" lists, each card subtitled "Album • 2007" /
//! "EP • 1992" / "Single • 2021" (the carousel drops "Album •" and says only "2007"). There is no
//! compilation type, no release date finer than the year, and no popularity per release.
//!
//! Bridges:
//! - Compilations. YouTube's full album list is studio albums (newest first), then live albums,
//!   then remix/other albums, then compilations, each its own newest-first run (checked live on
//!   Queen, Daft Punk and Radiohead, 2026-11). So a list splits into runs where the year goes back
//!   up, and a later run is the compilations when its titles say so. Any album titled like a
//!   compilation counts too.
//! - Popularity. The top songs name each song's album and its play count. A release's score is the
//!   sum of √plays over its songs: depth beats one big hit (OK Computer above Pablo Honey).

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use innertube::SongItem;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseType {
    Album,
    Single,
    Ep,
    Compilation,
}

impl ReleaseType {
    fn label(self) -> &'static str {
        match self {
            Self::Album => "Album",
            Self::Single => "Single",
            Self::Ep => "EP",
            Self::Compilation => "Compilation",
        }
    }
    /// The latest-release pick: an album or EP over a single of the same year.
    fn rank(self) -> u8 {
        match self {
            Self::Album => 0,
            Self::Ep => 1,
            Self::Compilation => 2,
            Self::Single => 3,
        }
    }
}

/// One release card. `item` is the card exactly as the app sent it.
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub item: Value,
    #[serde(rename = "type")]
    pub kind: ReleaseType,
    /// 0 when YouTube gave none.
    pub year: i32,
    /// Position in YouTube's own lists (albums first, then singles): the tiebreak within a year.
    pub order: usize,
    /// The card line: "1997 • Album", "Latest Release • Single", "Compilation".
    pub line: String,
    /// The list view's header: "Album • 1997" (the app adds " • 12 songs" once it has them).
    pub meta: String,
}

impl Release {
    fn id(&self) -> &str {
        self.item.get("id").and_then(Value::as_str).unwrap_or("")
    }
    fn title(&self) -> &str {
        self.item.get("title").and_then(Value::as_str).unwrap_or("")
    }
}

/// Every chip's list. Lists are newest first; `popular` pins a recent release, then ranks by the
/// top songs. An empty list is a chip to leave out (`all` and `popular` are empty only when the
/// artist has no releases at all).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discography {
    pub popular: Vec<Release>,
    /// The release `popular` pins first as the latest one.
    pub latest_id: Option<String>,
    pub all: Vec<Release>,
    pub album: Vec<Release>,
    pub single: Vec<Release>,
    pub compilation: Vec<Release>,
}

static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(19|20)\d{2}\b").unwrap());
/// Titles that only a compilation wears. Deliberately narrow: a false positive moves a real album.
static COMPILATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(greatest hits|best of|the very best|hits collection|the collection|the platinum collection|the essential|essentials|anthology|retrospective|the singles|singles collection|b-sides (and|&) rarities|rarities|deep cuts|the definitive|gold(en)? collection|number ones)\b").unwrap()
});
/// Weaker: only decides whether a whole later run is the compilations.
static SERIES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(vol\.?|volume)\s*\d+\b|\bmusique\b|\bhits\b").unwrap());
static EP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bEP\b").unwrap());
static EP_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bEP$").unwrap());
static SINGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bsingle\b").unwrap());
static COMP_SUB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)compilation").unwrap());
static COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*([\d.,]+)\s*([KMB])?").unwrap());
static EDITION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*[(\[].*?[)\]]\s*").unwrap());
static DASH_TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+-\s+.*$").unwrap());
static REISSUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(19|20)\d{2}\b|deluxe|remaster|edition|anniversary|expanded|reissue|version|mix\b").unwrap()
});

fn text<'a>(i: &'a Value, key: &str) -> &'a str {
    i.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn year_of(i: &Value) -> i32 {
    YEAR.find(text(i, "subtitle")).and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

/// Splits a list into YouTube's newest-first runs: a new run starts wherever the year goes up.
fn runs(items: &[Value]) -> Vec<Vec<&Value>> {
    let mut out: Vec<Vec<&Value>> = Vec::new();
    let mut prev = i32::MAX;
    for i in items {
        let y = year_of(i);
        if out.is_empty() || (y != 0 && prev != 0 && y > prev) {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(i);
        if y != 0 {
            prev = y;
        }
    }
    out
}

/// Albums and singles in Spotify's types, deduped, in YouTube's order.
pub fn classify(albums: &[Value], singles: &[Value]) -> Vec<Release> {
    let mut seen = HashSet::new();
    let mut out: Vec<Release> = Vec::new();
    let mut push = |item: &Value, kind: ReleaseType| {
        if !seen.insert(text(item, "id").to_owned()) {
            return;
        }
        let year = year_of(item);
        let order = out.len();
        out.push(Release { item: item.clone(), kind, year, order, line: line(kind, year, false), meta: meta(kind, year) });
    };
    for (n, run) in runs(albums).iter().enumerate() {
        // A later run (never the first, the studio albums) is the compilations when one of its
        // titles reads like one.
        let comp_run =
            n > 0 && run.iter().any(|i| COMPILATION.is_match(text(i, "title")) || SERIES.is_match(text(i, "title")));
        for i in run {
            let sub = text(i, "subtitle");
            let kind = if COMP_SUB.is_match(sub) || comp_run || COMPILATION.is_match(text(i, "title")) {
                ReleaseType::Compilation
            } else if EP.is_match(sub) {
                ReleaseType::Ep
            } else if SINGLE.is_match(sub) {
                ReleaseType::Single
            } else {
                ReleaseType::Album
            };
            push(i, kind);
        }
    }
    for i in singles {
        let sub = text(i, "subtitle");
        let kind = if EP.is_match(sub) || EP_END.is_match(text(i, "title")) {
            ReleaseType::Ep
        } else if COMP_SUB.is_match(sub) {
            ReleaseType::Compilation
        } else {
            ReleaseType::Single
        };
        push(i, kind);
    }
    out
}

fn line(kind: ReleaseType, year: i32, latest: bool) -> String {
    if latest {
        format!("Latest Release • {}", kind.label())
    } else if year != 0 {
        format!("{year} • {}", kind.label())
    } else {
        kind.label().to_owned()
    }
}

fn meta(kind: ReleaseType, year: i32) -> String {
    if year != 0 { format!("{} • {year}", kind.label()) } else { kind.label().to_owned() }
}

/// Newest first; same year keeps YouTube's order (which is newest first inside a list).
pub fn by_newest(rels: &[Release]) -> Vec<Release> {
    let mut v = rels.to_vec();
    v.sort_by(|a, b| b.year.cmp(&a.year).then(a.order.cmp(&b.order)));
    v
}

/// "2.3B" → 2_300_000_000, "638M", "86K", "1,234" → numbers; `None` when unreadable.
pub fn parse_count(s: Option<&str>) -> Option<f64> {
    let c = COUNT.captures(s?)?;
    let mult = match c.get(2).map(|m| m.as_str().to_ascii_uppercase()) {
        Some(m) if m == "K" => 1e3,
        Some(m) if m == "M" => 1e6,
        Some(m) if m == "B" => 1e9,
        _ => 1.0,
    };
    let digits = &c[1];
    let n: f64 = if mult == 1.0 { digits.replace(['.', ','], "") } else { digits.replacen(',', ".", 1) }
        .parse()
        .ok()?;
    n.is_finite().then_some(n * mult)
}

/// Lowercased, accent- and punctuation-free: "Hail To the Thief" ≡ "Hail to the Thief".
fn norm(s: &str) -> String {
    let folded: String = s.to_lowercase().nfkd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect();
    let folded = folded.replace('&', " and ");
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Without the edition suffix: "Jazz (Deluxe Edition)" → "jazz".
fn base(s: &str) -> String {
    norm(&DASH_TAIL.replace(&EDITION.replace_all(s, " "), ""))
}

/// Which release a top song belongs to: by album id, then by title (exact, base, reissue prefix).
pub fn release_for<'a>(song: &SongItem, rels: &'a [Release]) -> Option<&'a Release> {
    if let Some(id) = song.album_id.as_deref() {
        if let Some(hit) = rels.iter().find(|r| r.id() == id) {
            return Some(hit);
        }
    }
    let title = song.album.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
    let n = norm(title);
    if let Some(exact) = rels.iter().find(|r| norm(r.title()) == n) {
        return Some(exact);
    }
    let b = base(title);
    if b.is_empty() {
        return None;
    }
    // Same record in another edition: prefer the plain original ("Jazz" over "Jazz (2026 Mix)").
    let same: Vec<&Release> = rels.iter().filter(|r| base(r.title()) == b).collect();
    if !same.is_empty() {
        return Some(same.iter().find(|r| norm(r.title()) == b).copied().unwrap_or(same[0]));
    }
    // "OK Computer OKNOTOK 1997 2017" → "OK Computer": a reissue named after the original.
    let mut best: Option<(&Release, usize)> = None;
    for r in rels {
        let rb = base(r.title());
        if rb.is_empty() || !n.starts_with(&format!("{rb} ")) || !REISSUE.is_match(&n[rb.len()..]) {
            continue;
        }
        if best.is_none_or(|(_, len)| rb.len() > len) {
            best = Some((r, rb.len()));
        }
    }
    best.map(|(r, _)| r)
}

/// Is `year` recent enough to pin as the latest release? Spotify pins one from about the last six
/// months; YouTube says only the year, so: this year, or last year while this one is under half
/// gone. `month` is 0-based.
pub fn is_recent(year: i32, now_year: i32, month: u32) -> bool {
    year == now_year || (year == now_year - 1 && month < 6)
}

/// Spotify's "Popular releases": a recent release pinned first, then releases by popularity (from
/// the top songs, most popular first), then everything else newest first. A song without a play
/// count is weighted as half the smallest one seen.
pub fn popular_releases(
    rels: &[Release],
    songs: &[SongItem],
    now_year: i32,
    month: u32,
    limit: usize,
) -> (Vec<Release>, Option<String>) {
    let counts: Vec<Option<f64>> = songs.iter().map(|s| parse_count(s.play_count.as_deref())).collect();
    let known: Vec<f64> = counts.iter().flatten().copied().collect();
    let floor = if known.is_empty() { 1.0 } else { known.iter().copied().fold(f64::INFINITY, f64::min) };
    let mut score: HashMap<String, f64> = HashMap::new();
    let mut seen_song = HashSet::new();
    for (i, s) in songs.iter().enumerate() {
        if !seen_song.insert(s.video_id.as_str()) {
            continue;
        }
        let Some(r) = release_for(s, rels) else { continue };
        // No counts at all: rank decides (1, 1/2, 1/3, …).
        let c = if known.is_empty() { floor / (i + 1) as f64 } else { counts[i].unwrap_or(floor * 0.5) };
        *score.entry(r.id().to_owned()).or_default() += c.sqrt();
    }

    let latest = rels
        .iter()
        .min_by(|a, b| b.year.cmp(&a.year).then(a.kind.rank().cmp(&b.kind.rank())).then(a.order.cmp(&b.order)))
        .filter(|r| r.year != 0 && is_recent(r.year, now_year, month));
    let latest_id = latest.map(|r| r.id().to_owned());

    let rest: Vec<&Release> = rels.iter().filter(|r| Some(r.id()) != latest_id.as_deref()).collect();
    let mut scored: Vec<&Release> = rest.iter().copied().filter(|r| score.contains_key(r.id())).collect();
    scored.sort_by(|a, b| score[b.id()].total_cmp(&score[a.id()]).then(a.order.cmp(&b.order)));
    let unscored: Vec<Release> =
        by_newest(&rest.iter().filter(|r| !score.contains_key(r.id())).map(|r| (*r).clone()).collect::<Vec<_>>());

    let mut out: Vec<Release> = Vec::new();
    if let Some(l) = latest {
        let mut l = l.clone();
        l.line = line(l.kind, l.year, true);
        out.push(l);
    }
    out.extend(scored.into_iter().cloned());
    out.extend(unscored);
    out.truncate(limit);
    (out, latest_id)
}

/// Every chip's list for these releases and top songs, as of `now_year` / `month` (0-based).
pub fn build(albums: &[Value], singles: &[Value], top_songs: &[SongItem], now_year: i32, month: u32) -> Discography {
    let rels = classify(albums, singles);
    let newest = by_newest(&rels);
    let (popular, latest_id) = popular_releases(&rels, top_songs, now_year, month, 20);
    let of = |f: &dyn Fn(ReleaseType) -> bool| newest.iter().filter(|r| f(r.kind)).cloned().collect::<Vec<_>>();
    Discography {
        album: of(&|k| k == ReleaseType::Album),
        single: of(&|k| matches!(k, ReleaseType::Single | ReleaseType::Ep)),
        compilation: of(&|k| k == ReleaseType::Compilation),
        all: newest.clone(),
        popular,
        latest_id,
    }
}

/// The same, now (local calendar).
pub fn build_now(albums: &[Value], singles: &[Value], top_songs: &[SongItem]) -> Discography {
    use chrono::Datelike;
    let now = chrono::Local::now();
    build(albums, singles, top_songs, now.year(), now.month0())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    fn rel(title: &str, sub: &str) -> Value {
        let n = N.fetch_add(1, Ordering::SeqCst);
        json!({ "kind": "album", "id": format!("MPRE{n}_{title}"), "title": title, "subtitle": sub })
    }
    fn song(title: &str, album: &str, album_id: Option<&str>, plays: Option<&str>) -> SongItem {
        let n = N.fetch_add(1, Ordering::SeqCst);
        serde_json::from_value(json!({
            "video_id": format!("v{n}"), "title": title, "artists": "x", "album": album,
            "album_id": album_id, "play_count": plays
        }))
        .unwrap()
    }
    fn titles(rs: &[Release]) -> Vec<&str> {
        rs.iter().map(Release::title).collect()
    }
    fn types(rs: &[Release]) -> Vec<ReleaseType> {
        rs.iter().map(|r| r.kind).collect()
    }

    fn queen() -> Vec<Release> {
        let albums = [
            rel("Queen Budapest", "Album • 2026"),
            rel("The Game", "Album • 1980"),
            rel("Queen", "Album • 1973"),
            rel("Live Around The World", "Album • 2020"),
            rel("Live at Wembley Stadium", "Album • 1992"),
            rel("In Concert (Glam Rock Mix)", "Album • 2023"),
            rel("Greatest Hits In Japan", "Album • 2020"),
            rel("The Platinum Collection", "Album • 2011"),
            rel("Greatest Hits", "Album • 1981"),
        ];
        let singles =
            [rel("Seven Seas of Rhye (2026 Mix)", "Single • 2026"), rel("Face It Alone", "Single • 2022"), rel("Some EP", "EP • 2019")];
        classify(&albums, &singles)
    }

    fn radiohead() -> (Vec<Value>, Vec<Value>) {
        (
            vec![
                rel("KID A MNESIA", "Album • 2021"),
                rel("In Rainbows", "Album • 2007"),
                rel("OK Computer", "Album • 1997"),
                rel("The Bends", "Album • 1995"),
                rel("Pablo Honey", "Album • 1993"),
                rel("Hail to the Thief (Live Recordings 2003-2009)", "Album • 2025"),
                rel("I Might Be Wrong", "Album • 2001"),
                rel("TKOL RMX 1234567", "Album • 2011"),
                rel("Com Lag: 2+2=5", "Album • 2003"),
            ],
            vec![rel("Creep", "Single • 1992"), rel("Drill EP", "EP • 1992")],
        )
    }

    #[test]
    fn parse_count_reads_youtube_numbers() {
        assert_eq!(parse_count(Some("2.3B")), Some(2.3e9));
        assert_eq!(parse_count(Some("638M")), Some(638e6));
        assert_eq!(parse_count(Some("86K")), Some(86e3));
        assert_eq!(parse_count(Some("1,234")), Some(1234.0));
        assert_eq!(parse_count(None), None);
        assert_eq!(parse_count(Some("n/a")), None);
    }

    #[test]
    fn compilations_from_youtube_run_order() {
        let q = queen();
        let comps: Vec<&str> = q.iter().filter(|r| r.kind == ReleaseType::Compilation).map(Release::title).collect();
        assert_eq!(comps, ["In Concert (Glam Rock Mix)", "Greatest Hits In Japan", "The Platinum Collection", "Greatest Hits"]);
        assert_eq!(q.iter().filter(|r| r.kind == ReleaseType::Album).count(), 5, "studio + live");
        let d = build(&[], &[], &[], 2026, 10);
        assert!(d.all.is_empty() && d.popular.is_empty());

        let daft = classify(
            &[
                rel("Random Access Memories", "Album • 2013"),
                rel("Discovery", "Album • 2001"),
                rel("Alive 2007", "Album • 2007"),
                rel("TRON: Legacy Reconfigured", "Album • 2011"),
                rel("Daft Club", "Album • 2003"),
                rel("Musique, Vol. 1", "Album • 2006"),
            ],
            &[],
        );
        let comps: Vec<&str> = daft.iter().filter(|r| r.kind == ReleaseType::Compilation).map(Release::title).collect();
        assert_eq!(comps, ["Musique, Vol. 1"]);

        let (a, s) = radiohead();
        let rh = build(&a, &s, &[], 2026, 10);
        assert!(rh.compilation.is_empty(), "Radiohead has no compilations");
        assert_eq!(types(&rh.single), [ReleaseType::Single, ReleaseType::Ep]);
        // The carousel's bare "2021" is an album; repeated cards appear once.
        assert_eq!(types(&classify(&[rel("Kid A", "2000")], &[])), [ReleaseType::Album]);
        let dup = rel("Dup", "Single • 2005");
        assert_eq!(classify(&[], &[dup.clone(), dup]).len(), 1);
    }

    #[test]
    fn chips_and_newest_first() {
        let (a, s) = radiohead();
        let d = build(&a, &s, &[], 2026, 10);
        assert_eq!(
            &titles(&d.all)[..4],
            ["Hail to the Thief (Live Recordings 2003-2009)", "KID A MNESIA", "TKOL RMX 1234567", "In Rainbows"]
        );
        let q = queen();
        let qd = build(
            &q.iter().filter(|r| r.kind != ReleaseType::Single && r.kind != ReleaseType::Ep).map(|r| r.item.clone()).collect::<Vec<_>>(),
            &[],
            &[],
            2026,
            10,
        );
        assert_eq!(qd.album.len(), 5);
        assert_eq!(qd.compilation.len(), 4);
    }

    #[test]
    fn songs_find_their_release() {
        let (a, s) = radiohead();
        let rh = classify(&a, &s);
        let okc = rh.iter().find(|r| r.title() == "OK Computer").unwrap();
        let t = |s: SongItem, rels: &[Release]| release_for(&s, rels).map(|r| r.title().to_owned());
        assert_eq!(t(song("No Surprises", "OK Computer OKNOTOK 1997 2017", Some("MPRE_other"), None), &rh).as_deref(), Some("OK Computer"));
        assert_eq!(t(song("Airbag", "OK Computer", Some(okc.id()), None), &rh).as_deref(), Some("OK Computer"));
        assert_eq!(t(song("Nude", "In Rainbows (Deluxe Edition)", Some("MPRE_x"), None), &rh).as_deref(), Some("In Rainbows"));
        assert_eq!(t(song("Creep", "Creep EP", Some("MPRE_y"), None), &rh), None);
        assert_eq!(t(song("x", "Queen Rock Montreal", Some("MPRE_z"), None), &queen()), None, "a bare prefix is not a reissue");
        let jazz = classify(&[rel("Jazz (2026 Mix)", "Album • 2026"), rel("Jazz", "Album • 1978")], &[]);
        assert_eq!(t(song("x", "Jazz (2026 Mix)", Some("MPRE_j"), None), &jazz).as_deref(), Some("Jazz (2026 Mix)"));
        assert_eq!(t(song("x", "Jazz (Deluxe Edition)", Some("MPRE_j"), None), &jazz).as_deref(), Some("Jazz"));
    }

    #[test]
    fn popular_releases_rank_by_depth_and_pin_the_latest() {
        let (a, s) = radiohead();
        let p = |t: &str, al: &str, id: Option<&str>, c: &str| song(t, al, id, Some(c));
        let songs = vec![
            p("Creep", "Pablo Honey", None, "2.3B"),
            p("No Surprises", "OK Computer OKNOTOK 1997 2017", Some("MPRE_ok"), "638M"),
            p("Let Down", "OK Computer OKNOTOK 1997 2017", Some("MPRE_ok"), "227M"),
            p("Fake Plastic Trees", "The Bends", None, "161M"),
            p("Karma Police", "OK Computer OKNOTOK 1997 2017", Some("MPRE_ok"), "262M"),
            p("High and Dry", "The Bends", None, "191M"),
            p("Exit Music", "OK Computer OKNOTOK 1997 2017", Some("MPRE_ok"), "117M"),
            p("Jigsaw Falling Into Place", "In Rainbows", None, "111M"),
            p("All I Need", "In Rainbows", None, "57M"),
            p("Weird Fishes", "In Rainbows", None, "68M"),
            p("Paranoid Android", "OK Computer OKNOTOK 1997 2017", Some("MPRE_ok"), "100M"),
        ];
        let d = build(&a, &s, &songs, 2026, 10);
        assert_eq!(d.latest_id, None, "a 2025 release is not pinned in Nov 2026");
        assert_eq!(&titles(&d.popular)[..2], ["OK Computer", "Pablo Honey"], "depth beats one hit");
        assert_eq!(titles(&d.popular)[4], "Hail to the Thief (Live Recordings 2003-2009)", "unscored follow, newest first");

        let q = queen();
        let items = |k: &[ReleaseType]| q.iter().filter(|r| k.contains(&r.kind)).map(|r| r.item.clone()).collect::<Vec<_>>();
        let qd = build(
            &items(&[ReleaseType::Album, ReleaseType::Compilation]),
            &items(&[ReleaseType::Single, ReleaseType::Ep]),
            &[song("Bohemian Rhapsody", "Greatest Hits", None, Some("2.7B"))],
            2026,
            10,
        );
        assert_eq!(&titles(&qd.popular)[..2], ["Queen Budapest", "Greatest Hits"], "latest pinned, then popular");
        assert_eq!(qd.popular[0].line, "Latest Release • Album");
        assert_eq!(qd.popular[1].line, "1981 • Compilation");

        let rank = build(&a, &s, &[song("a", "The Bends", None, None), song("b", "Pablo Honey", None, None)], 2026, 10);
        assert_eq!(&titles(&rank.popular)[..2], ["The Bends", "Pablo Honey"], "rank fallback");
    }

    #[test]
    fn recency_and_lines() {
        assert!(is_recent(2026, 2026, 10));
        assert!(!is_recent(2025, 2026, 10));
        assert!(is_recent(2025, 2026, 1));
        let (a, s) = radiohead();
        let rh = classify(&a, &s);
        let okc = rh.iter().find(|r| r.title() == "OK Computer").unwrap();
        assert_eq!(okc.meta, "Album • 1997");
        assert_eq!(okc.line, "1997 • Album");
        assert_eq!(rh.iter().find(|r| r.kind == ReleaseType::Ep).unwrap().line, "1992 • EP");
        assert_eq!(classify(&[rel("C", "Compilation")], &[])[0].line, "Compilation");
    }
}
