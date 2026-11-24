//! Explicit filter at playback time.
//!
//! The smart layer keeps explicit songs out of radio, autoplay and played lists. Songs the user
//! queues by hand ("Play next", "Add to queue") still land in the queue, as in Spotify, which shows
//! them greyed out and never plays them. So, with the filter on, playback steps over an explicit
//! row however it got there, exactly like an unavailable one: `next_playable_index` (state.rs) asks
//! [`blocks`], and `start_current` skips a blocked row it lands on with a "Skipped (explicit)"
//! notice. The UI greys rows by [`reasons`] (command `playback_skip_reasons`, and the
//! `skipReasons` map in the queue snapshot).
//!
//! A row is explicit when it wears YouTube's badge, or when the smart layer learned the flag for
//! its videoId (`smart_explicit`: earlier badges, Deezer lookups).

use std::collections::HashMap;

use innertube::SongItem;
use rusqlite::OptionalExtension;

use crate::db::Db;

fn filter_on(db: &Db) -> bool {
    crate::smart::settings::explicit_filter(db)
}

fn exempt(item: &SongItem) -> bool {
    crate::podcasts::is_episode_item(item) || crate::local::is_local_song(&item.video_id)
}

fn learned_explicit(db: &Db, video_id: &str) -> bool {
    db.with_conn(|c| {
        c.query_row("SELECT explicit FROM smart_explicit WHERE video_id = ?1", [video_id], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()
        .ok()
        .flatten()
        .flatten()
            == Some(1)
    })
}

/// Why playback won't play `item` right now, beyond `unavailable`: `Some("explicit")` with the
/// filter on. `None` when no database is installed (unit tests of the queue).
pub fn reason(item: &SongItem) -> Option<&'static str> {
    let db = super::db()?;
    reason_in(db, item)
}

pub fn reason_in(db: &Db, item: &SongItem) -> Option<&'static str> {
    if exempt(item) {
        return None;
    }
    // "Hide song" (everywhere): never played again, however it got into the queue (Spotify).
    if hidden_everywhere(db, &item.video_id) {
        return Some("hidden");
    }
    if !filter_on(db) {
        return None;
    }
    (item.explicit || learned_explicit(db, &item.video_id)).then_some("explicit")
}

fn hidden_everywhere(db: &Db, video_id: &str) -> bool {
    db.with_conn(|c| {
        c.query_row(
            "SELECT 1 FROM hidden_songs WHERE video_id = ?1 AND context_id = ''",
            [video_id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
    })
}

/// Whether `item` is explicit, filter or not: its own badge, or the flag the smart layer learned for
/// its videoId (radio/autoplay rows carry no badge). The `now-playing` event's `explicit`, which the
/// iOS lock screen and Dynamic Island show as the "E" badge, as Spotify does.
pub fn is_explicit(item: &SongItem) -> bool {
    is_explicit_in(super::db().map(|d| &**d), item)
}

fn is_explicit_in(db: Option<&Db>, item: &SongItem) -> bool {
    if item.explicit {
        return true;
    }
    if exempt(item) {
        return false;
    }
    db.is_some_and(|db| learned_explicit(db, &item.video_id))
}

/// [`reason`] as a yes/no, for the queue's skip logic.
pub fn blocks(item: &SongItem) -> bool {
    // ── stream: downloads ── offline: only downloads and local files play.
    if crate::downloads::offline_blocks(&item.video_id) {
        return true;
    }
    // ── end stream: downloads ──
    reason(item).is_some()
}

/// Every row playback will step over, by videoId: `"explicit"` (filter on) or `"unavailable"`
/// (no client could resolve it). One settings read and one table read for the whole list.
pub fn reasons(db: &Db, items: &[SongItem]) -> HashMap<String, &'static str> {
    let on = filter_on(db);
    let known = if on { crate::smart::explicit::known(db) } else { HashMap::new() };
    let hidden: std::collections::HashSet<String> =
        db.with_conn(|c| crate::library::hidden_ids(c)).into_iter().collect();
    items
        .iter()
        .filter_map(|i| {
            let r = if i.unavailable {
                Some("unavailable")
            } else if crate::downloads::offline_blocks(&i.video_id) {
                Some("offline") // stream: downloads
            } else if !exempt(i) && hidden.contains(&i.video_id) {
                Some("hidden")
            } else if on && !exempt(i) && (i.explicit || known.get(&i.video_id) == Some(&true)) {
                Some("explicit")
            } else {
                None
            };
            r.map(|r| (i.video_id.clone(), r))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, explicit: bool) -> SongItem {
        SongItem { video_id: id.into(), title: id.into(), explicit, ..Default::default() }
    }

    #[test]
    fn explicit_rows_are_blocked_only_with_the_filter_on() {
        let db = crate::polish::test_db();
        let badged = song("badged", true);
        let learned = song("learned", false);
        let clean = song("clean", false);
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO smart_explicit(video_id, explicit, source, at) VALUES('learned', 1, 'deezer', 0)",
                [],
            )
            .unwrap();
        });
        assert_eq!(reason_in(&db, &badged), None, "filter off");
        db.set_setting("explicit_filter", "true");
        assert_eq!(reason_in(&db, &badged), Some("explicit"));
        assert_eq!(reason_in(&db, &learned), Some("explicit"));
        assert_eq!(reason_in(&db, &clean), None);

        let ep = SongItem { is_episode: true, ..song("ep", true) };
        assert_eq!(reason_in(&db, &ep), None, "episodes are never filtered");

        let dead = SongItem { unavailable: true, ..song("dead", false) };
        let r = reasons(&db, &[badged, learned, clean, dead]);
        assert_eq!(r.get("badged"), Some(&"explicit"));
        assert_eq!(r.get("learned"), Some(&"explicit"));
        assert_eq!(r.get("dead"), Some(&"unavailable"));
        assert!(!r.contains_key("clean"));
    }

    #[test]
    fn the_explicit_badge_does_not_depend_on_the_filter() {
        let db = crate::polish::test_db();
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO smart_explicit(video_id, explicit, source, at) VALUES('learned', 1, 'deezer', 0)",
                [],
            )
            .unwrap();
        });
        assert!(is_explicit_in(Some(&db), &song("badged", true)), "filter off, badge shown");
        assert!(is_explicit_in(Some(&db), &song("learned", false)));
        assert!(!is_explicit_in(Some(&db), &song("clean", false)));
        assert!(!is_explicit_in(None, &song("learned", false)), "no database: the row's own flag");
        assert!(is_explicit_in(None, &song("badged", true)));
    }

    #[test]
    fn songs_hidden_everywhere_are_blocked_filter_or_not() {
        let db = crate::polish::test_db();
        let s = song("gone", false);
        assert_eq!(reason_in(&db, &s), None);
        db.with_conn(|c| crate::library::hide_song(c, "gone", None, None, 1)).unwrap();
        assert_eq!(reason_in(&db, &s), Some("hidden"), "hidden everywhere");
        assert_eq!(reasons(&db, &[s.clone()]).get("gone"), Some(&"hidden"));
        // Hidden only in one playlist: not a global block.
        let other = song("there", false);
        db.with_conn(|c| crate::library::hide_song(c, "there", None, Some("LOCALPLAYLIST:1"), 1)).unwrap();
        assert_eq!(reason_in(&db, &other), None);
        db.with_conn(|c| crate::library::unhide_song(c, "gone", None)).unwrap();
        assert_eq!(reason_in(&db, &s), None, "unhidden plays again");
    }
}
