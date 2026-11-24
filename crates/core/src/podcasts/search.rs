//! Merging YouTube Music and directory (iTunes / Podcast Index) results into one list.
//!
//! YouTube-backed shows rank first (ad-free stream, video). A directory show that is the same
//! show — same normalised title, and the same author or a title no other result shares — is
//! folded into the YouTube one (it gains `feed_url` / `itunes_id`) instead of being listed twice.
//! The author check can't be strict: YouTube names the host ("Andrew Huberman") where Apple names
//! the publisher ("Scicomm Media").

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use innertube::BrowseItem;

use super::{Episode, Show};

const STOP: [&str; 6] = ["the", "a", "an", "podcast", "show", "with"];

/// Lowercase, accents stripped, punctuation dropped, filler words ("the", "podcast") removed.
pub fn norm(s: &str) -> String {
    let folded: String = s
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .to_lowercase();
    let words: Vec<&str> = folded.split_whitespace().filter(|w| !STOP.contains(w)).collect();
    if words.is_empty() {
        folded.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        words.join(" ")
    }
}

/// The dedupe key: normalised title + author.
pub fn key(title: &str, author: &str) -> String {
    format!("{}|{}", norm(title), norm(author))
}

pub fn authors_match(a: &str, b: &str) -> bool {
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() || a == b || a.contains(&b) || b.contains(&a) {
        return true;
    }
    let wa: HashSet<&str> = a.split(' ').collect();
    b.split(' ').any(|w| w.len() > 2 && wa.contains(w))
}

/// YouTube shows first, then directory shows not already among them.
pub fn merge_shows(yt: Vec<Show>, dir: Vec<Show>) -> Vec<Show> {
    let count = |list: &[Show]| {
        let mut m: HashMap<String, usize> = HashMap::new();
        for s in list {
            *m.entry(norm(&s.title)).or_default() += 1;
        }
        m
    };
    let (yt_titles, dir_titles) = (count(&yt), count(&dir));
    let mut out = yt;
    let mut seen_keys: HashSet<String> = out.iter().map(|s| key(&s.title, &s.author)).collect();
    let mut seen_feeds: HashSet<String> = HashSet::new();
    for d in dir {
        let t = norm(&d.title);
        if let Some(f) = &d.feed_url {
            if !seen_feeds.insert(f.clone()) {
                continue;
            }
        }
        let unique = yt_titles.get(&t) == Some(&1) && dir_titles.get(&t) == Some(&1);
        if let Some(y) = fold_target(&out, &d, &t, unique).map(|i| &mut out[i]) {
            if norm(&d.author) != norm(&y.author) && !d.author.trim().is_empty() {
                y.dir_author = Some(d.author.clone());
            }
            y.feed_url = d.feed_url.clone();
            y.itunes_id = y.itunes_id.or(d.itunes_id);
            y.episode_count = y.episode_count.or(d.episode_count);
            if y.description.is_none() {
                y.description = d.description.clone();
            }
            if y.thumbnail.is_none() {
                y.thumbnail = d.thumbnail.clone();
            }
            continue;
        }
        if seen_keys.insert(key(&d.title, &d.author)) {
            out.push(d);
        }
    }
    out
}

/// The YouTube row a directory show folds into: one with the same normalised title and no feed
/// yet, whose author matches the directory's; else the only such row when the title is unique on
/// both sides; else the one row titled exactly like the directory's (its author named, before
/// one without). `None` leaves the directory show listed on its own.
///
/// stream: search-pop. An empty author used to match anything, so Apple's feed (and its episode
/// count and chart place) went to the first namesake YouTube row without a channel name: "rogan"
/// folded The Joe Rogan Experience into "Joe Rogan Podcast Experience" ("podcast" and "the" are
/// filler words), "smartless" into "smartless podcast".
fn fold_target(out: &[Show], d: &Show, t: &str, unique: bool) -> Option<usize> {
    let cands: Vec<usize> =
        (0..out.len()).filter(|&i| out[i].yt_browse_id.is_some() && out[i].feed_url.is_none() && norm(&out[i].title) == t).collect();
    let named = |a: &str| !a.trim().is_empty();
    if let Some(&i) = cands.iter().find(|&&i| named(&out[i].author) && named(&d.author) && authors_match(&out[i].author, &d.author)) {
        return Some(i);
    }
    if unique && cands.len() == 1 {
        return cands.first().copied();
    }
    let raw = |x: &str| x.trim().to_lowercase();
    let exact: Vec<usize> = cands.iter().copied().filter(|&i| raw(&out[i].title) == raw(&d.title)).collect();
    let exact_named: Vec<usize> = exact.iter().copied().filter(|&i| named(&out[i].author)).collect();
    match (exact_named.as_slice(), exact.as_slice()) {
        ([i], _) => Some(*i),
        ([], [i]) => Some(*i),
        _ => None,
    }
}

/// YouTube episodes first, then directory episodes with a title + show not already listed.
pub fn merge_episodes(yt: Vec<Episode>, dir: Vec<Episode>) -> Vec<Episode> {
    let mut seen: HashSet<String> = yt.iter().map(|e| key(&e.title, &e.show_title)).collect();
    let mut out = yt;
    for e in dir {
        if seen.insert(key(&e.title, &e.show_title)) {
            out.push(e);
        }
    }
    out
}

// ── stream: podtop ── the search page's Top result for a show's name (docs/streams/podtop.md).

/// How well a show answers the query: 2 = its title is the query ("the daily", "serial podcast"),
/// or its host is the query and the title names them ("joe rogan" → The Joe Rogan Experience);
/// 1 = the query is a run of whole words of the title ("huberman" → Huberman Lab); 0 = neither.
pub fn show_match(query: &str, s: &Show) -> u8 {
    let q = norm(query);
    let t = norm(&s.title);
    if q.is_empty() || t.is_empty() {
        return 0;
    }
    let padded = |x: &str| format!(" {x} ");
    let in_title = padded(&t).contains(&padded(&q));
    if t == q || (in_title && norm(&s.author) == q) {
        2
    } else if in_title && q.chars().count() >= 5 {
        1
    } else {
        0
    }
}

/// A show with a real audience: listed by both YouTube Music and the directory (folded by
/// [`merge_shows`]), or with a long back catalogue. Fan, recap and one-off shows that echo a
/// name rarely are either.
pub fn established(s: &Show) -> bool {
    (s.yt_browse_id.is_some() && s.feed_url.is_some())
        || s.episode_count.unwrap_or(0) >= ESTABLISHED_EPISODES
        || s.chart_rank.is_some() // stream: search-pop — on Apple's chart
}

/// Episodes from which a directory-only show counts as [`established`].
pub const ESTABLISHED_EPISODES: u32 = 100;

// ── stream: search2 ── a person's name finds their show ("chris williamson" → Modern Wisdom).

/// The query names the show's host: the show's author *is* the query, or one of its co-hosts is
/// ("Chris Williamson" in "Chris Williamson & Guests"). Never a run inside a longer name ("smells
/// like" is not the host of "Smells Like Teen Angst"). Two words at least, so a one-word name
/// ("drake", "adele") never pulls a namesake's show up.
pub fn host_match(query: &str, s: &Show) -> bool {
    let q = norm(query);
    if q.split(' ').count() < 2 {
        return false;
    }
    let named = |author: &str| {
        let lower = author.to_lowercase();
        let parts: Vec<String> = lower
            .split([',', '&', '|', '/', '+', ';', '(', ')'])
            .flat_map(|p| p.split(" and ").flat_map(|x| x.split(" with ")).map(str::to_owned).collect::<Vec<_>>())
            .map(|p| norm(&p))
            .collect();
        norm(author) == q || parts.iter().any(|p| *p == q)
    };
    named(&s.author) || s.dir_author.as_deref().is_some_and(named)
}

/// The query is the show's host, or the title names them as its host: "… with <name>", "… con
/// <name>", "w/ <name>", or the title ends with the name ("The Diary Of A CEO with Steven
/// Bartlett", "On Purpose with Jay Shetty"). A show that lists them among several names ("Broken
/// Record with Rick Rubin, Malcolm Gladwell, …") is not theirs.
pub fn hostlike(query: &str, s: &Show) -> bool {
    if host_match(query, s) {
        return true;
    }
    let words = |x: &str| -> String {
        x.nfkd()
            .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
            .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (q, t) = (words(query), words(&s.title));
    if q.split(' ').count() < 2 {
        return false;
    }
    let t = format!(" {t} ");
    [" with ", " con ", " w ", " featuring ", " hosted by "].iter().any(|p| t.contains(&format!("{p}{q} ")))
        || t.ends_with(&format!(" {q} podcast ")) && !t.contains(',')
}

/// The query is a person's name: some show's host is the query ([`host_match`]). Their big show
/// may still be listed under its publisher ("The Diary Of A CEO with Steven Bartlett" by DOAC,
/// "On Purpose with Jay Shetty" by Perfect Strangers), so for a person a title naming them counts
/// as well.
pub fn person_query(query: &str, shows: &[Show]) -> bool {
    shows.iter().any(|s| host_match(query, s))
}

/// How well a show answers the query, for ordering the Podcasts shelf and chip:
/// 3 = an [`established`] show whose title is the query, or whose host is the query, or (for a
/// person) whose title names them; 2 = the title is the query, or the query is its host, or a run
/// of its title's words; 0 = neither (left in source order).
pub fn show_relevance(query: &str, s: &Show, person: bool) -> u8 {
    let (m, host, est) = (show_match(query, s), host_match(query, s), established(s));
    if est && (m == 2 || host || (person && m == 1)) {
        3
    } else if m == 2 || host || m == 1 {
        2
    } else {
        0
    }
}

/// The merged show list in Spotify's order for a query, by [`show_score`]: how well the title
/// (or host) answers what was typed, times how popular the show is. Ties keep [`merge_shows`]'s
/// order (YouTube first). Stable.
///
/// stream: search-pop. Before, only the text tier ([`show_relevance`]) and the episode count among
/// a person's shows ordered the list, so "tucker" listed The Tucker Carlson Show with a dozen
/// namesakes in source order and "lex" put two-episode shows above Lex Fridman Podcast.
pub fn rank_shows(query: &str, mut shows: Vec<Show>) -> Vec<Show> {
    let person = person_query(query, &shows);
    let mut keyed: Vec<(f64, u8, Show)> = shows
        .drain(..)
        .map(|s| {
            let tier = u8::from(person && show_relevance(query, &s, true) == 3 && hostlike(query, &s));
            (show_score(query, &s), tier, s)
        })
        .collect();
    // A person's own established show stays first (search2); the score orders everything else.
    keyed.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.total_cmp(&a.0)));
    keyed.into_iter().map(|x| x.2).collect()
}

// ── stream: search-pop ── popularity-aware ranking (docs/streams/search-pop.md).

/// [`text_match`] grades: the title (or the host) is the query.
pub const MATCH_EXACT: f64 = 1.0;
/// The title names the query as its host ("On Purpose with Jay Shetty").
pub const MATCH_HOST: f64 = 0.9;
/// The query is a run of whole words of the title ("tucker" in The Tucker Carlson Show).
pub const MATCH_WORDS: f64 = 0.6;
/// The same, the last word still being typed ("smartle" → SmartLess, "diary of a c").
pub const MATCH_PREFIX: f64 = 0.5;
/// The query (or its start) is in the author's name rather than the title.
pub const MATCH_AUTHOR: f64 = 0.4;
/// Every query word starts some word of the title or author, in any order.
pub const MATCH_LOOSE: f64 = 0.3;
/// No word of the query is in the title or author (the source matched a description or an
/// episode): such shows keep their source order after every show that matches, however popular
/// ("diary of a c" must not list The Joe Rogan Experience above The Diary Of A CEO).
pub const MATCH_NONE: f64 = 0.0;

/// The query words appear as a run of `t`'s words; with `prefix`, the last one only has to start
/// its word (and must not be all of it, which the whole-word run already covers).
fn word_run(q: &[&str], t: &[&str], prefix: bool) -> bool {
    if q.is_empty() || q.len() > t.len() {
        return false;
    }
    let last = q.len() - 1;
    (0..=t.len() - q.len()).any(|i| {
        q.iter().enumerate().all(|(j, w)| {
            let x = t[i + j];
            if prefix && j == last { x.len() > w.len() && x.starts_with(w) } else { x == *w }
        })
    })
}

/// How well a show's text answers the query, 0–1 ([`MATCH_EXACT`] … [`MATCH_NONE`]): the
/// match-strength half of [`show_score`]. Typed queries are often unfinished, so a word being
/// typed counts as a prefix of the title's word.
pub fn text_match(query: &str, s: &Show) -> f64 {
    let q = norm(query);
    let t = norm(&s.title);
    if t == q && !q.is_empty() {
        return MATCH_EXACT;
    }
    if q.is_empty() || t.is_empty() {
        return MATCH_NONE;
    }
    if t == q || host_match(query, s) {
        return MATCH_EXACT;
    }
    if hostlike(query, s) {
        return MATCH_HOST;
    }
    let qw: Vec<&str> = q.split(' ').collect();
    let tw: Vec<&str> = t.split(' ').collect();
    if word_run(&qw, &tw, false) {
        return MATCH_WORDS;
    }
    if word_run(&qw, &tw, true) {
        return MATCH_PREFIX;
    }
    let authors: Vec<String> = std::iter::once(&s.author).chain(s.dir_author.as_ref()).map(|a| norm(a)).collect();
    if authors.iter().any(|a| {
        let aw: Vec<&str> = a.split(' ').collect();
        word_run(&qw, &aw, false) || word_run(&qw, &aw, true)
    }) {
        return MATCH_AUTHOR;
    }
    let all: Vec<&str> = tw.iter().copied().chain(authors.iter().flat_map(|a| a.split(' '))).collect();
    if qw.iter().all(|w| all.iter().any(|x| x.starts_with(w))) {
        return MATCH_LOOSE;
    }
    MATCH_NONE
}

/// Apple's chart is this long (`podcasts::charts`).
pub const CHART_SIZE: f64 = 200.0;
/// From this many episodes the back catalogue counts in full.
pub const EPISODES_FULL: f64 = 1000.0;
/// Weights of the chart place and the back catalogue in [`popularity`]. Being among the country's
/// 200 most popular shows says far more than a long back catalogue (a daily fan or news show
/// piles up episodes nobody plays): "shapiro" is The Ben Shapiro Show (#53), not a 976-episode
/// namesake.
pub const W_CHART: f64 = 1.0;
pub const W_EPISODES: f64 = 0.4;
/// What a show with no popularity signal at all still scores, so the text match decides there.
pub const POP_FLOOR: f64 = 0.05;

/// How popular a show is, 0–1.4, log-scaled: its place on Apple's Top Shows chart (#1 → 1, #200 →
/// 0.5, off the chart → 0) and its back catalogue (`ln(1 + episodes)`, full at
/// [`EPISODES_FULL`]). Apple's directory sends the episode count; a YouTube-only row has neither
/// unless it charts (matched by title, `podcasts::charts::apply`).
pub fn popularity(s: &Show) -> f64 {
    let chart = s.chart_rank.map_or(0.0, |r| 1.0 - 0.5 * f64::from(r.max(1)).ln() / CHART_SIZE.ln());
    let eps = s.episode_count.map_or(0.0, |n| ((1.0 + f64::from(n)).ln() / (1.0 + EPISODES_FULL).ln()).min(1.0));
    W_CHART * chart.clamp(0.0, 1.0) + W_EPISODES * eps
}

/// The show's search score: match strength times log-scaled popularity, the learning-to-rank
/// form `text × (floor + popularity)`. A popular show that only half matches ("tucker" → The
/// Tucker Carlson Show, #52 on the chart) beats an exact title nobody listens to ("Lex & Podcast",
/// four episodes, for "lex"), and an exact title with an audience still beats a popular show that
/// merely contains the words ("serial podcast" → Serial).
pub fn show_score(query: &str, s: &Show) -> f64 {
    text_match(query, s) * (POP_FLOOR + popularity(s))
}

/// The show a YouTube episode card belongs to ("Episode • Aug 10 • The Tucker Carlson Show" →
/// "The Tucker Carlson Show").
pub fn episode_card_show(card: &BrowseItem) -> Option<String> {
    let sub = card.subtitle.as_deref()?;
    if !sub.starts_with("Episode") {
        return None;
    }
    sub.split(" • ").last().filter(|s| !s.trim().is_empty() && !s.starts_with("Episode")).map(str::to_owned)
}

/// The show the query means when YouTube's Top card is one of its *episodes* and the query names
/// the show rather than the episode ("tucker" → The Hunter Biden Interview, an episode of The
/// Tucker Carlson Show): YouTube's own popularity-aware pick, lifted to the show, as Spotify
/// shows the show. The best-scored matching show; `None` when the query is in the episode's own
/// title or no listed show is that one.
pub fn show_of_episode_card(query: &str, card: &BrowseItem, shows: &[Show]) -> Option<usize> {
    let name = episode_card_show(card)?;
    let pad = |x: &str| format!(" {} ", norm(x));
    let (q, ep) = (pad(query), pad(&card.title));
    if q.trim().is_empty() || ep.contains(&q) {
        return None;
    }
    let n = pad(&name);
    shows
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let t = pad(&s.title);
            (t.contains(&n) || n.contains(&t)) && text_match(query, s) >= MATCH_LOOSE
        })
        .max_by(|a, b| show_score(query, a.1).total_cmp(&show_score(query, b.1)).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

/// How well a YouTube card's title answers the query, on [`text_match`]'s scale (title only).
pub fn card_text_match(query: &str, card: &BrowseItem) -> f64 {
    let (q, t) = (norm(query), norm(&card.title));
    if q.is_empty() || t.is_empty() {
        return MATCH_NONE;
    }
    if t == q {
        return MATCH_EXACT;
    }
    let qw: Vec<&str> = q.split(' ').collect();
    let tw: Vec<&str> = t.split(' ').collect();
    if word_run(&qw, &tw, false) {
        MATCH_WORDS
    } else if word_run(&qw, &tw, true) {
        MATCH_PREFIX
    } else {
        MATCH_NONE
    }
}

/// A charting show that answers the typed text better than YouTube's Top card does: "crime jun"
/// → Crime Junkie (#4) over the song "Crime", "hidden br" → Hidden Brain over "Doom (BR
/// session)", "smartle" → SmartLess over LE SSERAFIM. The query must be the show's title, its
/// words, or their start ([`MATCH_PREFIX`] and up), and the card's title must match less. The
/// best-scored such show.
pub fn charting_show_over_card(query: &str, card: Option<&BrowseItem>, shows: &[Show]) -> Option<usize> {
    let card_m = card.map_or(0.0, |c| card_text_match(query, c));
    shows
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let m = text_match(query, s);
            s.chart_rank.is_some() && m >= MATCH_PREFIX && m > card_m
        })
        .max_by(|a, b| show_score(query, a.1).total_cmp(&show_score(query, b.1)).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

/// The first show of the ranked list is the query's own popular show, so the Podcasts shelf goes
/// right under the Top result: its title or host is the query; or, when YouTube shows no strong
/// music card (`strong_music`), it charts and the query is its title's words or their start, or it
/// has a big back catalogue ([`BIG_CATALOGUE`]) and the query is its title's words ("lex" → Lex
/// Fridman Podcast, 504 episodes, under a 300K-audience artist named LEX). A one-episode show titled
/// like an artist ("Drake Podcast") isn't popular, and a show that only shares a word with a big
/// artist's name doesn't displace their music ("taylor" keeps Taylor Swift's songs first).
pub fn leads_popular(query: &str, s: &Show, strong_music: bool) -> bool {
    let m = text_match(query, s);
    let charted = s.chart_rank.is_some();
    let eps = s.episode_count.unwrap_or(0);
    if !(charted || eps >= ESTABLISHED_EPISODES) {
        return false;
    }
    if m >= MATCH_HOST {
        return true;
    }
    !strong_music && ((charted && m >= MATCH_PREFIX) || (eps >= BIG_CATALOGUE && m >= MATCH_WORDS))
}

/// Episodes from which an uncharted show counts as big for [`leads_popular`].
pub const BIG_CATALOGUE: u32 = 300;

/// A YouTube card that keeps the music above a partly matching show: a song or album titled like
/// the query ([`strong_music_card`]), or an artist named like it or whose name begins with it,
/// with an audience of [`STRONG_MUSIC_AUDIENCE`] (or none shown).
pub fn strong_music_for_shelf(query: &str, card: &BrowseItem) -> bool {
    match card.kind {
        "artist" => {
            let (q, t) = (norm(query), norm(&card.title));
            !q.is_empty()
                && (t == q || t.starts_with(&format!("{q} ")))
                && card_audience(card).map_or(true, |n| n >= STRONG_MUSIC_AUDIENCE)
        }
        _ => strong_music_card(query, card),
    }
}

/// See [`strong_music_for_shelf`].
pub const STRONG_MUSIC_AUDIENCE: f64 = 1e6;

/// "98M monthly audience" / "2.45K subscribers" on an artist card → 98_000_000 / 2_450.
pub fn card_audience(card: &BrowseItem) -> Option<f64> {
    let sub = card.subtitle.as_deref()?;
    sub.split(" • ").find_map(|part| {
        let lower = part.to_ascii_lowercase();
        if !(lower.contains("subscriber") || lower.contains("audience") || lower.contains("listener")) {
            return None;
        }
        let num = lower.split_whitespace().next()?;
        let (digits, mult) = match num.chars().last()? {
            'k' => (&num[..num.len() - 1], 1e3),
            'm' => (&num[..num.len() - 1], 1e6),
            'b' => (&num[..num.len() - 1], 1e9),
            _ => (num, 1.0),
        };
        digits.replace(',', "").parse::<f64>().ok().map(|n| n * mult)
    })
}

/// A YouTube card that a host's show must not displace: a song or album titled like the query
/// (or starting with it), or an artist named like it with a real audience (100K+). "chris williamson" has a 5-subscriber
/// namesake artist; "taylor swift" a 100M one.
pub fn strong_music_card(query: &str, card: &BrowseItem) -> bool {
    let same = |t: &str| norm(t) == norm(query) || crate::identity::norm(t) == crate::identity::norm(query);
    match card.kind {
        "artist" => same(&card.title) && card_audience(card).map_or(true, |n| n >= STRONG_ARTIST_AUDIENCE),
        // A song or album whose title is the query or begins with it ("sweet child" → Sweet Child
        // O' Mine): the query is music.
        "song" | "album" => {
            let (t, q) = (norm(&card.title), norm(query));
            same(&card.title) || (!q.is_empty() && format!("{t} ").starts_with(&format!("{q} ")))
        }
        _ => false,
    }
}

/// An artist card with at least this audience keeps the Top result against a host's show.
pub const STRONG_ARTIST_AUDIENCE: f64 = 100_000.0;

/// The show that should be the search page's Top result, like Spotify's ("joe rogan" → The Joe
/// Rogan Experience, "the daily" → The Daily, "chris williamson" → Modern Wisdom).
///
/// - **With a YouTube card** (stream: search2): only for a person's name ([`person_query`]), and
///   only over a card that isn't [`strong_music_card`]: their [`established`] show (their own, or
///   one whose title names them) with the biggest back catalogue. Any other card is kept: a show
///   titled like a common word ("hello", "animals", "lofi") would otherwise take it.
/// - **Without one** (stream: podtop): a show titled exactly like the query takes it, or one
///   whose title holds the query, or whose host is the query, and is [`established`]; when the
///   page's fallback artist row *is* the query ("joe rogan" has a 6.7K-subscriber artist
///   channel), only an established show does, so a fan show named after a musician never does.
///   An established exact title or host's show beats a small exact title ("alex cooper" → Call
///   Her Daddy, not a 6-episode show called "Alex Cooper").
///
/// Ties go to the bigger back catalogue, then list order. `yt_card` is YouTube's Top card after
/// [`crate::identity::rerank_search_all`], `artist` the first artist card, `shows`
/// `podcast_search`'s merged list. Returns the index into `shows`.
pub fn top_show(
    query: &str,
    yt_card: Option<&BrowseItem>,
    artist: Option<&BrowseItem>,
    shows: &[Show],
) -> Option<usize> {
    let person = person_query(query, shows);
    let eps = |s: &Show| s.episode_count.unwrap_or(0);
    // stream: search-pop — YouTube's Top card is an episode of the show the query names.
    if let Some(i) = yt_card.and_then(|c| show_of_episode_card(query, c, shows)) {
        return Some(i);
    }
    // ...or a charting show answers the typed text better than the card.
    if let Some(i) = yt_card.and_then(|c| charting_show_over_card(query, Some(c), shows)) {
        return Some(i);
    }
    if let Some(card) = yt_card {
        if !person || strong_music_card(query, card) || artist.is_some_and(|a| strong_music_card(query, a)) {
            return None;
        }
        return shows
            .iter()
            .enumerate()
            .filter(|(_, s)| established(s) && (host_match(query, s) || show_match(query, s) >= 1))
            .max_by_key(|&(i, s)| (hostlike(query, s), eps(s), std::cmp::Reverse(i)))
            .map(|(i, _)| i);
    }
    let artist_is_query = artist.is_some_and(|a| {
        crate::identity::norm(&a.title) == crate::identity::norm(query) || norm(&a.title) == norm(query)
    });
    shows
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let (m, est) = (show_match(query, s), established(s));
            let m = if m == 0 && host_match(query, s) { 1 } else { m };
            let ok = m > 0 && (if artist_is_query { est } else { m == 2 || est });
            let tier = show_relevance(query, s, person);
            // The back catalogue only breaks ties among established shows; small namesakes keep
            // list order (YouTube first).
            // stream: search-pop — among established equals the more popular show (score, in
            // millionths); small namesakes keep list order (YouTube first), as before.
            let pop = if est { (show_score(query, s) * 1e6) as u64 } else { 0 };
            ok.then_some((i, tier, m, est, if tier == 3 { eps(s) } else { 0 }, hostlike(query, s), pop))
        })
        // For a person, their biggest show wins over a title that merely names them ("Chris
        // Williamson's Podcast", 100 episodes, vs Modern Wisdom, 1,172); for a show's name the
        // closest title still goes first ("the daily" over "The Daily Show"), then the more
        // popular ("rogan": The Joe Rogan Experience, #3 on the chart, over fan shows).
        .max_by_key(|&(i, tier, m, est, n, host, pop)| {
            let (m, est, host) = (u32::from(m), u32::from(est), u32::from(host));
            if person { (tier, host, n, m, est, 0, std::cmp::Reverse(i)) } else { (tier, m, est, 0, 0, pop, std::cmp::Reverse(i)) }
        })
        .map(|(i, ..)| i)
        // stream: search-pop — a half-typed title ("smartle") isn't a word run: a charting show
        // still takes the place of an artist row that matches the typed text less.
        .or_else(|| charting_show_over_card(query, artist, shows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::podcasts::{itunes, youtube};

    fn fixture(name: &str) -> serde_json::Value {
        let p = format!("{}/src/podcasts/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn normalises() {
        assert_eq!(norm("The Daily"), "daily");
        assert_eq!(norm("Café  Podcast!"), "cafe");
        assert_eq!(norm("99% Invisible"), "99 invisible");
        assert_eq!(norm("The Show"), "the show", "all-stopword titles keep their words");
        assert_eq!(key("Huberman Lab", "Andrew Huberman"), "huberman lab|andrew huberman");
    }

    #[test]
    fn folds_the_same_show_from_both_sources() {
        let yt = youtube::parse_show_search(&fixture("yt_search_podcasts.json"));
        let dir = itunes::parse_shows(&fixture("itunes_shows.json"));
        let n_yt = yt.len();
        let merged = merge_shows(yt, dir.clone());
        let h = &merged[0];
        assert_eq!(h.id, "MPSPPLPNW_gerXa4Pc8S2qoUQc5e8Ir97RLuVW", "YouTube ranks first");
        assert_eq!(h.feed_url.as_deref(), Some("https://feeds.megaphone.fm/hubermanlab"));
        assert_eq!(h.itunes_id, Some(1545953110));
        let hubermans = merged.iter().filter(|s| norm(&s.title) == "huberman lab").count();
        assert_eq!(hubermans, 1, "{merged:#?}");
        assert!(merged.len() < n_yt + dir.len());
    }

    #[test]
    fn keeps_different_shows_with_the_same_title() {
        let mk = |id: &str, t: &str, a: &str, yt: bool| Show {
            id: id.into(),
            title: t.into(),
            author: a.into(),
            yt_browse_id: yt.then(|| id.to_owned()),
            feed_url: (!yt).then(|| id.to_owned()),
            ..Default::default()
        };
        let yt = vec![mk("MPSP1", "Search Engine", "PJ Vogt", true), mk("MPSP2", "Search Engine", "SEO Guys", true)];
        let dir = vec![mk("https://f/1", "Search Engine", "Jigsaw Productions", false)];
        let merged = merge_shows(yt, dir);
        assert_eq!(merged.len(), 3, "ambiguous title and no author match: listed separately");
        // Same feed from two directories appears once.
        let dir = vec![mk("https://f/x", "X", "A", false), mk("https://f/x", "X!", "B", false)];
        assert_eq!(merge_shows(vec![], dir).len(), 1);
    }

    #[test]
    fn dedupes_episodes() {
        let e = |t: &str, s: &str, id: &str| Episode { id: id.into(), title: t.into(), show_title: s.into(), ..Default::default() };
        let merged = merge_episodes(
            vec![e("Ep 1: Hello", "The Show X", "yt1")],
            vec![e("EP 1 — hello", "Show X", "PODURL:a"), e("Ep 2", "Show X", "PODURL:b")],
        );
        assert_eq!(merged.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["yt1", "PODURL:b"]);
    }

    // ── stream: podtop ──
    fn show(t: &str, a: &str) -> Show {
        Show { id: format!("MPSP{t}"), title: t.into(), author: a.into(), ..Default::default() }
    }
    fn card(kind: &'static str, t: &str, sub: &str) -> innertube::BrowseItem {
        innertube::BrowseItem {
            kind,
            id: "x".into(),
            title: t.into(),
            subtitle: Some(sub.into()),
            thumbnail: None,
            duration: None,
            album_id: None,
            artist_runs: Vec::new(),
            play_count: None,
            is_video: false,
            is_upload: false,
            explicit: false,
        }
    }

    fn big(mut s: Show) -> Show {
        s.yt_browse_id = Some(s.id.clone());
        s.feed_url = Some(format!("https://feeds/{}", s.id));
        s
    }

    #[test]
    fn show_takes_the_top_result_when_youtube_has_no_card() {
        let shows = [show("Some Other Show", "X"), show("The Daily", "The New York Times")];
        let news = card("artist", "The Daily News", "Artist");
        assert_eq!(top_show("the daily", None, Some(&news), &shows), Some(1));
        // An artist row named like the query: only an established show (YouTube + directory).
        let rogan = card("artist", "Joe Rogan", "Artist");
        let jre = [show("Joe Rogan impressions", "Fans"), big(show("The Joe Rogan Experience", "PowerfulJRE"))];
        assert_eq!(top_show("joe rogan", None, Some(&rogan), &jre), Some(1));
        assert_eq!(top_show("joe rogan", None, Some(&rogan), &jre[..1]), None);
        // The exact title beats a longer one; established breaks the tie.
        let cj = [show("Crime Junkie: #1 True Crime Podcast", "Crime Junkie"), show("Crime Junkie", "a"), big(show("Crime Junkie", "Audiochuck"))];
        assert_eq!(top_show("crime junkie", None, None, &cj), Some(2));
        // A word run of the title needs an established show.
        assert_eq!(top_show("huberman", None, None, &[show("Huberman Lab", "Scicomm Media")]), None);
        assert_eq!(top_show("huberman", None, None, &[big(show("Huberman Lab", "Scicomm Media"))]), Some(0));
    }

    #[test]
    fn music_keeps_its_top_result() {
        // Any YouTube card is kept: a corrected typo, a song, a playlist for a common word.
        let ts = card("artist", "The Weeknd", "Artist");
        assert_eq!(top_show("the weekend", Some(&ts), Some(&ts), &[big(show("The Weekend Show", "M"))]), None);
        let song = card("song", "Hello Cotto", "Song • X");
        assert_eq!(top_show("hello", Some(&song), None, &[show("hello", "someone")]), None);
        // No card, the artist is the query: a small fan show named after them doesn't take it.
        let tsw = card("artist", "Taylor Swift", "Artist");
        assert_eq!(top_show("taylor swift", None, Some(&tsw), &[show("Taylor Swift Podcast", "Swifties")]), None);
    }

    // ── stream: search2 ──
    #[test]
    fn a_person_finds_their_show() {
        let mw = big(show("Modern Wisdom", "Chris Williamson"));
        let fan = show("Best of Chris Williamson Podcasts", "Quoting Giants");
        let shows = vec![fan.clone(), show("Other", "X"), mw.clone()];
        // Ranked: the host's established show first, then the title run, then the rest.
        let ranked = rank_shows("Chris Williamson", shows.clone());
        assert_eq!(ranked[0].title, "Modern Wisdom");
        assert_eq!(ranked[1].title, fan.title);
        // Top result: over a tiny namesake artist card, and with no card at all.
        let tiny = card("artist", "Chris Williamson", "Artist • 5 subscribers");
        assert_eq!(top_show("chris williamson", Some(&tiny), Some(&tiny), &shows), Some(2));
        assert_eq!(top_show("chris williamson", None, None, &shows), Some(2));
        // Not over a real artist, a song titled like the query, or for a one-word name.
        let star = card("artist", "Chris Williamson", "Artist • 2.1M monthly audience");
        assert_eq!(top_show("chris williamson", Some(&star), None, &shows), None);
        let adele = big(show("Adele Talks", "Adele"));
        assert_eq!(top_show("adele", Some(&card("song", "Hello", "Song • Adele")), None, &[adele]), None);
        // A host's show that isn't established doesn't displace a YouTube card.
        let small = show("Chris Williamson Thoughts", "Chris Williamson");
        assert_eq!(top_show("chris williamson", Some(&tiny), None, &[small]), None);
    }

    #[test]
    fn reads_card_audiences() {
        assert_eq!(card_audience(&card("artist", "a", "Artist • 2.45K subscribers")), Some(2450.0));
        assert_eq!(card_audience(&card("artist", "a", "Artist • 98M monthly audience")), Some(98e6));
        assert_eq!(card_audience(&card("artist", "a", "Artist")), None);
    }

    // ── stream: search-pop ──
    fn pop(t: &str, a: &str, eps: Option<u32>, chart: Option<u32>) -> Show {
        Show { episode_count: eps, chart_rank: chart, ..show(t, a) }
    }

    #[test]
    fn text_match_grades_partial_queries() {
        let tcs = show("The Tucker Carlson Show", "Tucker Carlson Network");
        assert_eq!(text_match("the tucker carlson show", &tcs), MATCH_EXACT);
        assert_eq!(text_match("tucker", &tcs), MATCH_WORDS);
        assert_eq!(text_match("tucker carl", &tcs), MATCH_PREFIX);
        assert_eq!(text_match("smartle", &show("SmartLess", "x")), MATCH_PREFIX);
        assert_eq!(text_match("diary of a c", &show("The Diary Of A CEO with Steven Bartlett", "DOAC")), MATCH_PREFIX);
        assert_eq!(text_match("network", &tcs), MATCH_AUTHOR);
        assert_eq!(text_match("carlson tuck", &tcs), MATCH_LOOSE);
        assert_eq!(text_match("rogan", &tcs), MATCH_NONE);
        assert_eq!(text_match("jay shetty", &show("On Purpose with Jay Shetty", "Perfect Strangers")), MATCH_HOST);
    }

    #[test]
    fn popularity_is_log_scaled_and_the_chart_counts_most() {
        let p = |eps, chart| popularity(&pop("x", "y", eps, chart));
        assert_eq!(p(None, None), 0.0);
        assert!(p(None, Some(1)) > p(None, Some(50)) && p(None, Some(50)) > p(None, Some(200)));
        assert!((p(None, Some(1)) - W_CHART).abs() < 1e-9 && (p(None, Some(200)) - W_CHART / 2.0).abs() < 1e-9);
        assert!(p(Some(1000), None) > p(Some(100), None) && p(Some(100), None) > p(Some(10), None));
        // The 200th chart place beats any back catalogue.
        assert!(p(None, Some(200)) > p(Some(5000), None));
    }

    #[test]
    fn popular_shows_rank_first_for_partial_queries() {
        // "tucker": the charting show over a long-running namesake and fan uploads.
        let shows = vec![
            pop("Andrew Tate and Tucker Carlson interview", "Matrix Mindshift", None, None),
            pop("Ross Tucker Football Podcast", "NFL Football", Some(1394), None),
            pop("The Tucker Carlson Show", "Tucker Carlson Network", Some(470), Some(52)),
        ];
        assert_eq!(rank_shows("tucker", shows)[0].title, "The Tucker Carlson Show");
        // "lex": a 504-episode show over a 4-episode exact title.
        let shows = vec![pop("Lex & Podcast", "Lex & Talk", Some(4), None), pop("Lex Fridman Podcast", "Lex Fridman", Some(504), None)];
        assert_eq!(rank_shows("lex", shows)[0].title, "Lex Fridman Podcast");
        // An exact title with an audience still beats a bigger show that merely holds the word.
        let shows = vec![pop("Serial Killers", "Parcast", Some(900), None), pop("Serial", "Serial Productions", Some(60), None)];
        assert_eq!(rank_shows("serial podcast", shows)[0].title, "Serial");
    }

    #[test]
    fn an_empty_author_never_takes_another_show_s_feed() {
        let yt = |id: &str, t: &str, a: &str| Show { yt_browse_id: Some(id.into()), ..show(t, a) };
        let dir = Show { feed_url: Some("https://feeds/jre".into()), itunes_id: Some(360084272), episode_count: Some(2000), ..show("The Joe Rogan Experience", "Joe Rogan") };
        // "Joe Rogan Podcast Experience" normalises to the same title ("the", "podcast" are filler).
        let merged = merge_shows(vec![yt("MPSPfan", "Joe Rogan Podcast Experience", ""), yt("MPSPjre", "The Joe Rogan Experience", "PowerfulJRE")], vec![dir.clone()]);
        let jre = merged.iter().find(|s| s.yt_browse_id.as_deref() == Some("MPSPjre")).unwrap();
        assert_eq!(jre.itunes_id, Some(360084272), "{merged:#?}");
        assert!(merged.iter().find(|s| s.yt_browse_id.as_deref() == Some("MPSPfan")).unwrap().feed_url.is_none());
        // A unique title on both sides still folds, whatever the authors.
        let merged = merge_shows(vec![yt("MPSPx", "Call Her Daddy", "")], vec![Show { feed_url: Some("f".into()), ..show("Call Her Daddy", "Alex Cooper") }]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].dir_author.as_deref(), Some("Alex Cooper"));
    }

    #[test]
    fn youtube_s_episode_card_or_a_weaker_card_gives_way_to_the_show() {
        let tcs = pop("The Tucker Carlson Show", "Tucker Carlson Network", Some(470), Some(52));
        let shows = vec![pop("Ross Tucker Football Podcast", "NFL", Some(1394), None), tcs];
        let ep = card("playlist", "The Hunter Biden Interview", "Episode • Aug 10 • The Tucker Carlson Show");
        assert_eq!(episode_card_show(&ep).as_deref(), Some("The Tucker Carlson Show"));
        assert_eq!(top_show("tucker", Some(&ep), None, &shows), Some(1));
        // Typing the episode's own title keeps the episode.
        assert_eq!(top_show("hunter biden interview", Some(&ep), None, &shows), None);
        // A charting show over a card that matches the typed text less.
        let cj = vec![pop("Crime Junkie", "Audiochuck", Some(535), Some(4))];
        assert_eq!(top_show("crime jun", Some(&card("song", "Crime", "Song • Quasimoto")), None, &cj), Some(0));
        assert_eq!(top_show("smartle", None, Some(&card("artist", "LE SSERAFIM", "Artist • 9M monthly audience")), &[pop("SmartLess", "SmartLess", None, Some(17))]), Some(0));
        // Not over a card titled like the query, nor for a show off the chart.
        let crime_song = card("song", "Crime Junkie", "Song • Someone");
        assert_eq!(top_show("crime junkie", Some(&crime_song), None, &cj), None);
        assert_eq!(top_show("crime jun", Some(&card("song", "Crime", "Song • Q")), None, &[pop("Crime Junkie", "A", Some(535), None)]), None);
    }

    #[test]
    fn the_podcasts_shelf_goes_first_only_for_a_popular_show() {
        // "drake": a one-episode show titled like the artist doesn't push Drake's songs down.
        let drake = big(pop("Drake Podcast", "AI Podcast Chaos", Some(1), None));
        assert!(!leads_popular("drake", &drake, true));
        assert!(!leads_popular("drake", &drake, false));
        // "lex": a big show under a modest namesake artist; not under a big artist's name.
        let lex = pop("Lex Fridman Podcast", "Lex Fridman", Some(504), None);
        let lex_artist = card("artist", "LEX", "Artist • 300K monthly audience");
        assert!(!strong_music_for_shelf("lex", &lex_artist));
        assert!(leads_popular("lex", &lex, false));
        let ts = card("artist", "Taylor Swift", "Artist • 412M monthly audience");
        assert!(strong_music_for_shelf("taylor", &ts));
        assert!(!leads_popular("taylor", &pop("Dr Taylor Marshall Podcast", "x", Some(1128), None), true));
        // A show titled exactly like the query, with an audience, always leads.
        assert!(leads_popular("the daily", &pop("The Daily", "NYT", Some(2600), Some(1)), true));
    }
}
