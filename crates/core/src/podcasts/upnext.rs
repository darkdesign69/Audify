//! Podcast up next (stream: podcast-upnext): what plays after an episode, Spotify's prediction.
//!
//! When an episode is played from a show page, Your Episodes or anywhere else, the queue becomes:
//!
//! 1. **History**: the episode that was playing before, if any, so Previous (and the Now Playing
//!    swipe) goes back to it and resumes where it was left.
//! 2. **The episode played.**
//! 3. **From the same show** ([`next_in_show`]): the next newer unplayed episode; for a serial show
//!    only ever a newer one, for an episodic show the next unplayed one in the list it was played
//!    from when nothing newer is left. Your Episodes (several shows) goes down its own list.
//!    Part of the context, like the rest of an album: it plays with Autoplay off too.
//! 4. **Autoplay** (only with the Autoplay setting on, appended by the queue's autoplay trigger,
//!    `AppState::extend_podcasts`):
//!    - the newest unplayed episode of each other followed show, the shows you listen to most
//!      first ([`rank_followed`], from `episode_listens`);
//!    - one popular recent episode from a show like it ([`pick_discovery`]): Apple's chart for the
//!      show's genre, the same cached feeds the Podcasts home shelves read.
//!
//! Each predicted row carries its `reason` ("From …", "Because you follow …", "Popular in …"),
//! which both queue panels show. Episodes never seed music radio, and songs never lead here.

use std::collections::{HashMap, HashSet};

use innertube::SongItem;
use rusqlite::Connection;

use super::{is_episode_item, now_secs, search::norm, store, Episode, Sort};

/// Followed shows' new episodes in one autoplay batch.
pub const FOLLOWED_MAX: usize = 5;
/// A followed show's episode counts as new for this long.
pub const FRESH_WINDOW: i64 = 30 * 86_400;
/// How many of a show's stored episodes [`next_in_show`] looks through.
const SHOW_SCAN: usize = 500;

/// A predicted episode and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub episode: Episode,
    pub reason: String,
}

impl Pick {
    /// The queue row.
    pub fn song(&self) -> SongItem {
        SongItem { reason: Some(self.reason.clone()), ..self.episode.song() }
    }
}

pub fn from_show(show: &str) -> String {
    format!("From {show}")
}

pub fn because_you_follow(show: &str) -> String {
    format!("Because you follow {show}")
}

/// `genre` is Apple's category name; `None` for the overall chart.
pub fn popular_in(genre: Option<&str>, show: &str) -> String {
    match genre {
        Some(g) => format!("Popular in {g} · {show}"),
        None => format!("Popular podcasts · {show}"),
    }
}

/// One show, however it was named: the show id when both sides know it, else the title.
fn same_show(a: &Episode, b: &Episode) -> bool {
    match (&a.show_id, &b.show_id) {
        (Some(x), Some(y)) => x == y,
        _ => !a.show_title.is_empty() && norm(&a.show_title) == norm(&b.show_title),
    }
}

/// The episode after `list[start]`, from the same show (or from the list, for Your Episodes).
///
/// - `list`: what it was played from, in that order (a show page, newest or oldest first; Your
///   Episodes; a single episode).
/// - `show_eps`: the show's other stored episodes, in any order.
/// - `serial`: the feed asks for oldest-first listening.
///
/// The next **newer** unplayed episode wins (the following one of a serial, the next one out of an
/// episodic show you're catching up on). With nothing newer, an episodic show goes on down the
/// list the user came from, then to the next older stored episode; a serial stops.
pub fn next_in_show(
    list: &[Episode],
    start: usize,
    show_eps: &[Episode],
    serial: bool,
) -> Option<Episode> {
    let cur = list.get(start)?;
    let open = |e: &Episode| !e.progress.played && e.id != cur.id;
    if !list.iter().all(|e| same_show(e, cur)) {
        // Your Episodes and other mixed lists: their own order.
        return list[start + 1..].iter().find(|e| open(e)).cloned();
    }
    let mut seen = HashSet::new();
    let pool: Vec<&Episode> = list
        .iter()
        .chain(show_eps)
        .filter(|e| same_show(e, cur) && seen.insert(e.id.as_str()))
        .collect();
    if let Some(at) = cur.published_at {
        let newer =
            pool.iter().filter(|e| open(e)).filter(|e| e.published_at.is_some_and(|p| p > at));
        if let Some(e) = newer.min_by_key(|e| e.published_at) {
            return Some((*e).clone());
        }
        if serial {
            return None;
        }
    }
    if let Some(e) = list[start + 1..].iter().find(|e| open(e)) {
        return Some(e.clone());
    }
    // Played from its own page (a list of one): the next older one the show has.
    let at = cur.published_at?;
    pool.into_iter()
        .filter(|e| open(e))
        .filter(|e| e.published_at.is_some_and(|p| p < at))
        .max_by_key(|e| e.published_at)
        .cloned()
}

/// Shows already in the queue from the playing row on, by id and by title: they get no second row.
#[derive(Debug, Default, Clone)]
pub struct Taken {
    pub ids: HashSet<String>,
    pub shows: HashSet<String>,
}

impl Taken {
    pub fn add(&mut self, e: &Episode) {
        self.ids.insert(e.id.clone());
        if let Some(s) = &e.show_id {
            self.shows.insert(s.clone());
        }
        if !e.show_title.is_empty() {
            self.shows.insert(norm(&e.show_title));
        }
    }

    pub fn has_show(&self, e: &Episode) -> bool {
        e.show_id.as_ref().is_some_and(|s| self.shows.contains(s))
            || self.shows.contains(&norm(&e.show_title))
    }
}

/// Followed shows' newest unplayed episodes, one per show, the shows you listen to most first
/// (`listen`: seconds heard per show id); shows never heard keep the newest-episode order.
/// `fresh` is newest first ([`store::new_episodes`]). Shows already `taken` are skipped.
pub fn rank_followed(
    fresh: &[Episode],
    listen: &HashMap<String, i64>,
    taken: &Taken,
    n: usize,
) -> Vec<Pick> {
    let mut shows = HashSet::new();
    let mut per_show: Vec<&Episode> = Vec::new();
    for e in fresh {
        if e.progress.played || taken.ids.contains(&e.id) || taken.has_show(e) {
            continue;
        }
        let key = e.show_id.clone().unwrap_or_else(|| norm(&e.show_title));
        if shows.insert(key) {
            per_show.push(e);
        }
    }
    let heard = |e: &Episode| e.show_id.as_ref().and_then(|s| listen.get(s)).copied().unwrap_or(0);
    // Stable: equal listening keeps the newest episode first.
    per_show.sort_by_key(|e| std::cmp::Reverse(heard(e)));
    per_show
        .into_iter()
        .take(n)
        .map(|e| Pick { episode: e.clone(), reason: because_you_follow(&e.show_title) })
        .collect()
}

/// Seconds heard per show, from `episode_listens`: the episode's stored show, else the show it
/// was played from.
pub fn listen_secs(conn: &Connection) -> HashMap<String, i64> {
    let Ok(mut st) = conn.prepare(
        "SELECT COALESCE(e.show_id, l.show_id) AS sid, SUM(l.secs) FROM episode_listens l
         LEFT JOIN podcast_episodes e ON e.id = l.video_id
         WHERE sid IS NOT NULL GROUP BY sid",
    ) else {
        return HashMap::new();
    };
    st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// The stored episode behind a queue row, or one built from the row (title and show name only).
pub fn episode_of(conn: &Connection, s: &SongItem) -> Episode {
    store::get_episode(conn, &s.video_id).unwrap_or_else(|| Episode {
        id: s.video_id.clone(),
        title: s.title.clone(),
        show_title: s.artists.clone(),
        thumbnail: s.thumbnail.clone(),
        ..Default::default()
    })
}

/// The queue for an episode played from `items` (its list) at `start`: the episode before it
/// (`history`, the row that was playing), the episode, and the next one from its show. Returns the
/// rows and where the played episode is in them. Rows that aren't episodes are left out.
pub fn context_queue(
    conn: &Connection,
    items: &[SongItem],
    start: usize,
    history: Option<SongItem>,
) -> (Vec<SongItem>, usize) {
    let Some(picked) = items.get(start).cloned() else { return (Vec::new(), 0) };
    let rows: Vec<&SongItem> = items.iter().filter(|s| is_episode_item(s)).collect();
    let at = rows.iter().position(|s| s.video_id == picked.video_id).unwrap_or(0);
    let list: Vec<Episode> = rows.iter().map(|s| episode_of(conn, s)).collect();
    let (show_eps, serial) = match list.get(at).and_then(|e| e.show_id.clone()) {
        Some(sid) => (
            store::show_episodes(conn, &sid, Sort::Newest, 0, SHOW_SCAN).unwrap_or_default(),
            store::is_serial(conn, &sid),
        ),
        None => (Vec::new(), false),
    };
    let mut out = Vec::new();
    if let Some(h) = history.filter(|h| is_episode_item(h) && h.video_id != picked.video_id) {
        out.push(SongItem { queued: false, queued_end: false, autoplay: false, reason: None, ..h });
    }
    let index = out.len();
    out.push(SongItem { reason: None, autoplay: false, ..picked });
    if let Some(next) = next_in_show(&list, at, &show_eps, serial) {
        out.push(Pick { reason: from_show(&next.show_title), episode: next }.song());
    }
    (out, index)
}

/// What is in the queue from `current` on, for [`followed`] and [`discover`] to skip.
pub fn taken(conn: &Connection, items: &[SongItem], current: usize) -> Taken {
    let mut t = Taken::default();
    for s in items {
        t.ids.insert(s.video_id.clone());
    }
    for s in items.iter().skip(current).filter(|s| is_episode_item(s)) {
        t.add(&episode_of(conn, s));
    }
    t
}

/// The followed-shows block of an autoplay batch.
pub fn followed(conn: &Connection, taken: &Taken) -> Vec<Pick> {
    let fresh = store::new_episodes(conn, now_secs() - FRESH_WINDOW, 200).unwrap_or_default();
    rank_followed(&fresh, &listen_secs(conn), taken, FOLLOWED_MAX)
}

/// The first show on a genre chart (`chart_ids`, in chart order) that isn't yours or already
/// queued, with its newest episode you haven't played.
pub fn pick_discovery(
    chart_ids: &[String],
    shows: &HashMap<String, super::Show>,
    eps: &HashMap<String, Episode>,
    taken: &Taken,
    genre: Option<&str>,
) -> Option<Pick> {
    chart_ids.iter().filter_map(|id| shows.get(id)).find_map(|show| {
        let e = eps.get(&show.id)?;
        let open = !e.progress.played
            && !taken.ids.contains(&e.id)
            && !taken.has_show(e)
            && !taken.shows.contains(&norm(&show.title));
        open.then(|| Pick { episode: e.clone(), reason: popular_in(genre, &show.title) })
    })
}

/// Chart shows looked up per batch: enough to step over the ones you follow.
const DISCOVERY_LOOKUP: usize = 8;

/// One popular recent episode from a show like `seed_show` (network, through the cached iTunes
/// feeds the Podcasts home already reads: the show's genre, that genre's chart, one lookup).
/// `taken` should also hold your followed shows. `None` offline or when nothing new is left.
pub async fn discover(db: &crate::db::Db, seed_show: &str, taken: &Taken) -> Option<Pick> {
    use crate::smart::podhome;
    let genre = if seed_show.is_empty() { None } else { podhome::genre_of(db, seed_show).await };
    let chart = podhome::chart(db, genre.as_deref()).await;
    let ids: Vec<String> = chart.into_iter().map(|e| e.id).take(DISCOVERY_LOOKUP).collect();
    let (shows, mut eps) = podhome::lookup(db, &ids).await;
    let mut list: Vec<Episode> = eps.values().cloned().collect();
    {
        let conn = db.podcasts_conn();
        // Stored so the episode page and resume work like any other RSS episode.
        let _ = store::upsert_episodes(&conn, &list);
        store::decorate(&conn, &mut list);
    }
    for e in list {
        if let Some(s) = e.show_id.clone() {
            eps.insert(s, e);
        }
    }
    let name =
        genre.as_deref().and_then(|g| podhome::CATEGORIES.iter().find(|c| c.0 == g)).map(|c| c.1);
    pick_discovery(&ids, &shows, &eps, taken, name)
}

/// Your followed shows, for [`discover`] to step over.
pub fn add_follows(conn: &Connection, taken: &mut Taken) {
    for s in store::follows(conn).unwrap_or_default() {
        taken.shows.insert(s.id.clone());
        taken.shows.insert(norm(&s.title));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::podcasts::{Progress, Show};

    fn ep(id: &str, show: &str, at: i64) -> Episode {
        Episode {
            id: id.into(),
            show_id: Some(show.into()),
            show_title: format!("{show} Show"),
            title: format!("Ep {id}"),
            published_at: Some(at),
            ..Default::default()
        }
    }

    fn played(mut e: Episode) -> Episode {
        e.progress = Progress { played: true, ..Default::default() };
        e
    }

    fn ids(v: &[Pick]) -> Vec<&str> {
        v.iter().map(|p| p.episode.id.as_str()).collect()
    }

    #[test]
    fn episodic_show_goes_to_the_next_newer_unplayed_episode() {
        // A show page, newest first: d c b a. Playing b goes to c (newer), skipping played ones.
        let page = vec![
            ep("d", "S", 400),
            played(ep("c", "S", 300)),
            ep("b", "S", 200),
            ep("a", "S", 100),
        ];
        assert_eq!(next_in_show(&page, 2, &[], false).unwrap().id, "d");
        let page = vec![ep("d", "S", 400), ep("c", "S", 300), ep("b", "S", 200), ep("a", "S", 100)];
        assert_eq!(next_in_show(&page, 2, &[], false).unwrap().id, "c");
        // The newest: nothing newer, so on down the list it came from.
        assert_eq!(next_in_show(&page, 0, &[], false).unwrap().id, "c");
        // Sorted oldest first, the list and "newer" agree.
        let oldest: Vec<Episode> = page.iter().rev().cloned().collect();
        assert_eq!(next_in_show(&oldest, 0, &[], false).unwrap().id, "b");
    }

    #[test]
    fn serial_show_only_moves_forward() {
        let page = vec![ep("c", "S", 300), ep("b", "S", 200), ep("a", "S", 100)];
        assert_eq!(next_in_show(&page, 2, &[], true).unwrap().id, "b");
        assert_eq!(next_in_show(&page, 0, &[], true), None, "the latest: a serial doesn't go back");
    }

    #[test]
    fn a_single_episode_uses_the_shows_stored_episodes() {
        let one = vec![ep("b", "S", 200)];
        let stored =
            vec![ep("c", "S", 300), ep("b", "S", 200), ep("a", "S", 100), ep("x", "Other", 250)];
        assert_eq!(next_in_show(&one, 0, &stored, false).unwrap().id, "c");
        let one = vec![ep("c", "S", 300)];
        assert_eq!(
            next_in_show(&one, 0, &stored, false).unwrap().id,
            "b",
            "the newest: the next older one"
        );
        assert_eq!(next_in_show(&one, 0, &stored, true), None);
    }

    #[test]
    fn your_episodes_goes_down_its_own_list() {
        let saved = vec![ep("x", "A", 100), played(ep("y", "B", 900)), ep("z", "C", 50)];
        assert_eq!(next_in_show(&saved, 0, &[], false).unwrap().id, "z");
        assert_eq!(next_in_show(&saved, 2, &[], false), None);
    }

    #[test]
    fn followed_shows_rank_by_listening() {
        // Newest first, as `store::new_episodes` returns them.
        let fresh = vec![
            ep("a2", "A", 900),
            ep("b1", "B", 800),
            ep("a1", "A", 700),
            ep("c1", "C", 600),
            ep("d1", "D", 500),
        ];
        let listen: HashMap<String, i64> = [("C".to_owned(), 9000), ("B".to_owned(), 300)].into();
        let mut taken = Taken::default();
        taken.add(&ep("now", "D", 1));
        let picks = rank_followed(&fresh, &listen, &taken, 5);
        // C (most heard), B, then A (never heard: newest first); D is the show playing.
        assert_eq!(ids(&picks), vec!["c1", "b1", "a2"]);
        assert_eq!(picks[0].reason, "Because you follow C Show");
        assert_eq!(picks[0].song().reason.as_deref(), Some("Because you follow C Show"));
        assert!(picks[0].song().is_episode);
        assert_eq!(rank_followed(&fresh, &listen, &taken, 1).len(), 1);
        // A queued episode is skipped, and so is its show.
        taken.ids.insert("c1".into());
        assert_eq!(ids(&rank_followed(&fresh, &listen, &taken, 5)), vec!["b1", "a2"]);
    }

    #[test]
    fn discovery_skips_your_shows() {
        let show =
            |id: &str, t: &str| Show { id: id.into(), title: t.into(), ..Default::default() };
        let shows: HashMap<String, Show> = [
            ("1".into(), show("f1", "Yours")),
            ("2".into(), show("f2", "New One")),
            ("3".into(), show("f3", "Third")),
        ]
        .into();
        let eps: HashMap<String, Episode> = [
            ("f1".into(), Episode { show_title: "Yours".into(), ..ep("e1", "f1", 9) }),
            ("f2".into(), Episode { show_title: "New One".into(), ..ep("e2", "f2", 9) }),
            ("f3".into(), Episode { show_title: "Third".into(), ..ep("e3", "f3", 9) }),
        ]
        .into();
        let mut taken = Taken::default();
        taken.shows.insert(norm("Yours"));
        let chart: Vec<String> = vec!["1".into(), "2".into(), "3".into()];
        let p = pick_discovery(&chart, &shows, &eps, &taken, Some("Business")).unwrap();
        assert_eq!(p.episode.id, "e2");
        assert_eq!(p.reason, "Popular in Business · New One");
        taken.ids.insert("e2".into());
        assert_eq!(
            pick_discovery(&chart, &shows, &eps, &taken, None).unwrap().reason,
            "Popular podcasts · Third"
        );
    }

    fn conn() -> Connection {
        let c = store::test_conn();
        c.execute_batch(
            "CREATE TABLE episode_listens (id INTEGER PRIMARY KEY, video_id TEXT NOT NULL,
               played_at INTEGER NOT NULL, secs INTEGER NOT NULL, song_json TEXT NOT NULL, show_id TEXT);",
        )
        .unwrap();
        c
    }

    #[test]
    fn context_queue_keeps_history_and_adds_the_shows_next() {
        let c = conn();
        store::upsert_episodes(&c, &[ep("a", "S", 100), ep("b", "S", 200), ep("c", "S", 300)])
            .unwrap();
        let rows: Vec<SongItem> = [ep("c", "S", 300), ep("b", "S", 200), ep("a", "S", 100)]
            .iter()
            .map(Episode::song)
            .collect();
        let before = SongItem {
            video_id: "old".into(),
            is_episode: true,
            queued: true,
            ..Default::default()
        };
        let (q, at) = context_queue(&c, &rows, 2, Some(before));
        let got: Vec<&str> = q.iter().map(|s| s.video_id.as_str()).collect();
        assert_eq!((got, at), (vec!["old", "a", "b"], 1));
        assert!(!q[0].queued);
        assert_eq!(q[2].reason.as_deref(), Some("From S Show"));
        assert!(!q[2].autoplay, "the show's own next plays with Autoplay off too");
        // A song before it is no episode history; a serial at its latest has no next.
        store::set_serial(&c, "S", true);
        let song = SongItem { video_id: "song".into(), ..Default::default() };
        let (q, at) = context_queue(&c, &rows, 0, Some(song));
        assert_eq!((q.len(), at), (1, 0));
    }

    #[test]
    fn listening_is_summed_per_show() {
        let c = conn();
        store::upsert_episodes(&c, &[ep("a", "S", 100)]).unwrap();
        c.execute_batch(
            "INSERT INTO episode_listens(video_id, played_at, secs, song_json, show_id) VALUES
             ('a', 1, 100, '{}', NULL), ('a', 2, 50, '{}', 'ctx'), ('zz', 3, 7, '{}', 'T'), ('yy', 4, 1, '{}', NULL);",
        )
        .unwrap();
        let m = listen_secs(&c);
        assert_eq!(m.get("S"), Some(&150));
        assert_eq!(m.get("T"), Some(&7));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn followed_reads_the_store() {
        let c = conn();
        let now = now_secs();
        for (id, title) in [("A", "A Show"), ("B", "B Show")] {
            store::follow(&c, &Show { id: id.into(), title: title.into(), ..Default::default() })
                .unwrap();
        }
        store::upsert_episodes(
            &c,
            &[ep("a1", "A", now - 100), ep("b1", "B", now - 50), ep("old", "B", now - 90 * 86_400)],
        )
        .unwrap();
        c.execute("INSERT INTO episode_listens(video_id, played_at, secs, song_json) VALUES('a0', 1, 500, '{}')", []).unwrap();
        store::upsert_episodes(&c, &[ep("a0", "A", now - 900)]).unwrap();
        store::set_played(&c, "a0", true).unwrap();
        let picks = followed(&c, &Taken::default());
        assert_eq!(ids(&picks), vec!["a1", "b1"]);
        let mut t = Taken::default();
        add_follows(&c, &mut t);
        assert!(t.has_show(&ep("x", "A", 1)));
    }

    #[test]
    fn serial_feeds_are_detected() {
        let c = conn();
        assert!(!store::is_serial(&c, "S"));
        store::set_serial(&c, "S", true);
        assert!(store::is_serial(&c, "S"));
    }
}
