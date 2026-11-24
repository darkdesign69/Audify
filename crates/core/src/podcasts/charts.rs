//! Apple's Top Shows chart as a popularity signal for podcast search (stream: search-pop,
//! docs/streams/search-pop.md).
//!
//! The iTunes RSS feed (`/rss/toppodcasts/limit=200/json`) lists the country's 200 most popular
//! shows with their collection id, title and publisher. Search marks every show on it with its
//! place (`Show::chart_rank`): by collection id when the show came from (or was folded with) the
//! directory, else by title, so a YouTube-only row of a charting show gets it too.
//!
//! The feed is fetched once per country and kept in memory for [`TTL`] (on disk for 24 h by
//! `itunes::get_json`). Search never waits on it for long: a cold fetch runs in the background and
//! search goes on without the chart after [`WAIT`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Show;
use crate::db::Db;

/// One chart entry, in chart order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChartRow {
    /// iTunes collection id.
    pub id: i64,
    pub title: String,
    pub artist: String,
}

/// Rows of the iTunes RSS chart (`feed.entry[]`) or the marketing-tools chart (`feed.results[]`).
pub fn parse(v: &Value) -> Vec<ChartRow> {
    let feed = &v["feed"];
    if let Some(rows) = feed["results"].as_array() {
        return rows
            .iter()
            .filter_map(|r| {
                Some(ChartRow {
                    id: r["id"].as_str()?.parse().ok()?,
                    title: r["name"].as_str()?.to_owned(),
                    artist: r["artistName"].as_str().unwrap_or_default().to_owned(),
                })
            })
            .collect();
    }
    let entries = match &feed["entry"] {
        Value::Array(a) => a.clone(),
        e @ Value::Object(_) => vec![e.clone()],
        _ => Vec::new(),
    };
    entries
        .iter()
        .filter_map(|e| {
            Some(ChartRow {
                id: e["id"]["attributes"]["im:id"].as_str()?.parse().ok()?,
                title: e["im:name"]["label"].as_str()?.to_owned(),
                artist: e["im:artist"]["label"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

/// Mark each show on the chart with its place (1-based). A chart entry goes to the show with its
/// collection id; failing that, to the show with its title whose author is the chart's publisher,
/// else to the first show with that title (YouTube's own order, popularity-aware, comes first in
/// the merged list). Each entry marks one show at most.
pub fn apply(shows: &mut [Show], chart: &[ChartRow]) {
    if chart.is_empty() || shows.is_empty() {
        return;
    }
    let mut taken = vec![false; shows.len()];
    let by_id: HashMap<i64, usize> = shows.iter().enumerate().filter_map(|(i, s)| Some((s.itunes_id?, i))).collect();
    let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in shows.iter().enumerate() {
        by_title.entry(plain(&s.title)).or_default().push(i);
        by_key.entry(title_key(&s.title)).or_default().push(i);
    }
    let none = Vec::new();
    for (pos, row) in chart.iter().enumerate() {
        let rank = pos as u32 + 1;
        let free = |v: &Vec<usize>| -> Vec<usize> { v.iter().copied().filter(|&i| !taken[i] && shows[i].chart_rank.is_none()).collect() };
        let hit = by_id.get(&row.id).copied().filter(|&i| !taken[i]).or_else(|| {
            let mut same = free(by_title.get(&plain(&row.title)).unwrap_or(&none));
            if same.is_empty() {
                same = free(by_key.get(&title_key(&row.title)).unwrap_or(&none));
            }
            same.iter()
                .copied()
                .find(|&i| {
                    let named = |a: &str| !a.trim().is_empty() && super::search::authors_match(a, &row.artist);
                    named(&shows[i].author) || shows[i].dir_author.as_deref().is_some_and(named)
                })
                .or(same.first().copied())
        });
        if let Some(i) = hit {
            taken[i] = true;
            shows[i].chart_rank = Some(shows[i].chart_rank.map_or(rank, |r| r.min(rank)));
        }
    }
}

/// A title folded for comparing with the chart: lowercase, accents and punctuation dropped, and a
/// leading "the" and a trailing "podcast" left out, but no other word ("The Daily Show" is not
/// "The Daily", which [`norm`]'s filler list would make it).
pub fn plain(title: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let folded: String = title
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .to_lowercase();
    let mut w: Vec<&str> = folded.split_whitespace().collect();
    if w.len() > 1 && w[0] == "the" {
        w.remove(0);
    }
    if w.len() > 1 && w.last() == Some(&"podcast") {
        w.pop();
    }
    w.join(" ")
}

/// A title without the host or tagline one platform adds: "The Diary Of A CEO with Steven
/// Bartlett" and "The Diary Of A CEO" are one show, as are "Armchair Expert with Dax Shepard |
/// Podcast" and "Armchair Expert with Dax Shepard". Cut at the first " with ", " | ", ": " or
/// " - ", then [`plain`].
pub fn title_key(title: &str) -> String {
    let low = title.to_lowercase();
    let cut = [" with ", " | ", ": ", " - "].iter().filter_map(|p| low.find(p)).min().unwrap_or(low.len());
    let k = plain(&low[..cut]);
    if k.is_empty() { plain(title) } else { k }
}

/// How long the parsed chart is kept in memory.
pub const TTL: Duration = Duration::from_secs(6 * 3600);
/// How long a search waits for a chart that isn't cached yet.
pub const WAIT: Duration = Duration::from_millis(300);

type Cache = Mutex<HashMap<String, (Instant, Arc<Vec<ChartRow>>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

async fn fetch(db: &Db, cc: &str) -> Vec<ChartRow> {
    let url = format!("https://itunes.apple.com/{cc}/rss/toppodcasts/limit=200/json");
    super::itunes::get_json(db, &url, &[]).await.map(|v| parse(&v)).unwrap_or_default()
}

/// The country's chart: from memory, else fetched (waiting at most [`WAIT`]; the fetch finishes in
/// the background and fills the cache for the next search). Empty when Apple is unreachable.
pub async fn top(db: &Arc<Db>) -> Arc<Vec<ChartRow>> {
    let cc = super::itunes::country(db).to_lowercase();
    if let Some((at, rows)) = cache().lock().unwrap().get(&cc) {
        if at.elapsed() < TTL {
            return rows.clone();
        }
    }
    let task = tokio::spawn({
        let db = db.clone();
        let cc = cc.clone();
        async move {
            let rows = Arc::new(fetch(&db, &cc).await);
            if !rows.is_empty() {
                cache().lock().unwrap().insert(cc, (Instant::now(), rows.clone()));
            }
            rows
        }
    });
    tokio::time::timeout(WAIT, task).await.ok().and_then(Result::ok).unwrap_or_default()
}

/// Fetch the chart in the background at startup, so the first search doesn't go without it.
pub fn prewarm(db: Arc<Db>) {
    crate::rt::spawn(async move {
        let _ = top(&db).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(id: &str, t: &str, a: &str, itunes: Option<i64>) -> Show {
        Show { id: id.into(), title: t.into(), author: a.into(), itunes_id: itunes, ..Default::default() }
    }

    #[test]
    fn parses_both_feeds() {
        let rss = serde_json::json!({"feed": {"entry": [
            {"im:name": {"label": "The Daily"}, "im:artist": {"label": "The New York Times"}, "id": {"attributes": {"im:id": "1200361736"}}},
            {"im:name": {"label": "The Joe Rogan Experience"}, "im:artist": {"label": "Joe Rogan"}, "id": {"attributes": {"im:id": "360084272"}}}
        ]}});
        let rows = parse(&rss);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], ChartRow { id: 360084272, title: "The Joe Rogan Experience".into(), artist: "Joe Rogan".into() });
        let mt = serde_json::json!({"feed": {"results": [{"id": "1200361736", "name": "The Daily", "artistName": "The New York Times"}]}});
        assert_eq!(parse(&mt)[0].title, "The Daily");
    }

    #[test]
    fn marks_shows_by_id_then_title() {
        let chart = vec![
            ChartRow { id: 1, title: "The Daily".into(), artist: "The New York Times".into() },
            ChartRow { id: 3, title: "The Joe Rogan Experience".into(), artist: "Joe Rogan".into() },
        ];
        let mut shows = vec![
            show("MPSPa", "The Joe Rogan Experience", "PowerfulJRE", None),
            show("MPSPb", "The Joe Rogan Experience", "Podcast Summaries", None),
            show("feed", "Daily (renamed)", "NYT", Some(1)),
            show("MPSPc", "The Daily", "A fan", None),
        ];
        apply(&mut shows, &chart);
        assert_eq!(shows[2].chart_rank, Some(1), "by collection id");
        assert_eq!(shows[3].chart_rank, None, "the entry is taken");
        assert_eq!(shows[0].chart_rank, Some(2), "first show with the title");
        assert_eq!(shows[1].chart_rank, None);
        // A title without the host the chart names still matches.
        let mut shows = vec![show("MPSPd", "The Diary Of A CEO", "The Diary Of A CEO", None)];
        apply(&mut shows, &[ChartRow { id: 55, title: "The Diary Of A CEO with Steven Bartlett".into(), artist: "DOAC".into() }]);
        assert_eq!(shows[0].chart_rank, Some(1));
        assert_eq!(title_key("Armchair Expert with Dax Shepard | Podcast"), title_key("Armchair Expert with Dax Shepard"));
        assert_ne!(title_key("The Daily Show"), title_key("The Daily"));
        // The publisher's own row wins over an earlier namesake.
        let mut shows = vec![show("x", "The Daily", "A fan", None), show("y", "The Daily", "The New York Times", None)];
        apply(&mut shows, &chart);
        assert_eq!((shows[0].chart_rank, shows[1].chart_rank), (None, Some(1)));
    }
}
