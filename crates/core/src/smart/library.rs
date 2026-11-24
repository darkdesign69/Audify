//! The library facts the smart layer needs: what is blocked, what is liked, whether the session
//! is private. A trait so Stream B's real library can slot in; [`DbLibrary`] is the temporary
//! implementation over the tables `smart::migrate` creates (see the module docs in `smart`).

use std::collections::HashSet;

use innertube::{BlockList, BrowseItem, SongItem};

use crate::db::Db;

#[allow(dead_code)] // `private_session` is part of the interface Stream B implements.
pub trait Library {
    fn blocked_track_ids(&self) -> HashSet<String>;
    /// `(channel id, name)` pairs, either may be empty.
    fn blocked_artists(&self) -> Vec<(Option<String>, String)>;
    /// `(video_id, liked_at unix secs, song_json)`.
    fn liked(&self) -> Vec<(String, i64, Option<String>)>;
    fn private_session(&self) -> bool;
}

pub struct DbLibrary<'a>(pub &'a Db);

impl Library for DbLibrary<'_> {
    fn blocked_track_ids(&self) -> HashSet<String> {
        self.0.with_conn(|c| {
            let Ok(mut st) = c.prepare("SELECT video_id FROM hidden_songs WHERE context_id = ''") else {
                return HashSet::new();
            };
            st.query_map([], |r| r.get::<_, String>(0))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        })
    }

    fn blocked_artists(&self) -> Vec<(Option<String>, String)> {
        let mut out: Vec<(Option<String>, String)> = self.0.with_conn(|c| {
            let Ok(mut st) = c.prepare("SELECT artist_id, COALESCE(name, '') FROM blocked_artists")
            else {
                return Vec::new();
            };
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map(|rows| {
                    rows.flatten()
                        .map(|(id, name)| match id.strip_prefix("name:") {
                            Some(n) => (None, if name.is_empty() { n.to_owned() } else { name }),
                            None => (Some(id), name),
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        // Limusic's existing settings-backed artist blocklist counts too.
        out.extend(crate::blocked::list(self.0).into_iter().map(|b| (b.id, b.name)));
        out
    }

    fn liked(&self) -> Vec<(String, i64, Option<String>)> {
        self.0.with_conn(|c| {
            let Ok(mut st) =
                c.prepare("SELECT video_id, added_at, song_json FROM liked_songs")
            else {
                return Vec::new();
            };
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        })
    }

    fn private_session(&self) -> bool {
        crate::library::is_private(self.0)
    }
}

/// The hard filter every generator runs (research 07 §4 step 1): blocked songs, blocked artists
/// and, with the explicit filter on, explicit songs.
pub struct Filter {
    tracks: HashSet<String>,
    artists: BlockList,
    explicit_on: bool,
    /// Explicit flags learned so far (badges, Deezer), by videoId.
    explicit: std::collections::HashMap<String, bool>,
}

impl Filter {
    pub fn load(db: &Db) -> Filter {
        let lib = DbLibrary(db);
        let explicit_on = super::settings::explicit_filter(db);
        Filter {
            tracks: lib.blocked_track_ids(),
            artists: BlockList::new(lib.blocked_artists()),
            explicit_on,
            explicit: if explicit_on { super::explicit::known(db) } else { Default::default() },
        }
    }

    #[cfg(test)]
    pub fn new(
        tracks: &[&str],
        artists: Vec<(Option<String>, String)>,
        explicit_on: bool,
    ) -> Filter {
        Filter {
            tracks: tracks.iter().map(|s| s.to_string()).collect(),
            artists: BlockList::new(artists),
            explicit_on,
            explicit: Default::default(),
        }
    }

    pub fn explicit_on(&self) -> bool {
        self.explicit_on
    }

    pub fn learn_explicit(&mut self, id: &str, explicit: bool) {
        self.explicit.insert(id.to_owned(), explicit);
    }

    /// Explicit as far as anything knows: the row's own badge, or a learned flag.
    pub fn is_explicit(&self, s: &SongItem) -> bool {
        s.explicit || self.explicit.get(&s.video_id).copied().unwrap_or(false)
    }

    /// Whether the explicit flag of `s` is unknown (no badge, nothing learned).
    pub fn explicit_unknown(&self, s: &SongItem) -> bool {
        !s.explicit && !self.explicit.contains_key(&s.video_id)
    }

    pub fn is_blocked(&self, s: &SongItem) -> bool {
        self.tracks.contains(&s.video_id) || self.artists.blocks_song(s)
    }

    pub fn is_blocked_card(&self, c: &BrowseItem) -> bool {
        (c.kind == "song" && self.tracks.contains(&c.id)) || self.artists.blocks_card(c)
    }

    /// Never plays: blocked, or explicit while the filter is on.
    pub fn rejects(&self, s: &SongItem) -> bool {
        self.is_blocked(s) || (self.explicit_on && self.is_explicit(s))
    }

    pub fn rejects_card(&self, c: &BrowseItem) -> bool {
        self.is_blocked_card(c) || (self.explicit_on && c.kind == "song" && c.explicit)
    }

    pub fn retain(&self, items: &mut Vec<SongItem>, keep: Option<&str>) {
        items.retain(|i| Some(i.video_id.as_str()) == keep || !self.rejects(i));
    }
}

/// One-call checks for other modules (the interface the lead asked for).
#[allow(dead_code)]
pub fn is_blocked(db: &Db, s: &SongItem) -> bool {
    Filter::load(db).is_blocked(s)
}

pub fn liked_ids(db: &Db) -> HashSet<String> {
    DbLibrary(db).liked().into_iter().map(|(id, _, _)| id).collect()
}

#[allow(dead_code)]
pub fn private_session(db: &Db) -> bool {
    DbLibrary(db).private_session()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::tests::song;

    #[test]
    fn filter_reads_the_tables_and_the_old_artist_list() {
        let db = super::super::test_db();
        db.with_conn(|c| {
            c.execute_batch(
                "INSERT INTO hidden_songs(video_id, context_id, hidden_at) VALUES('hidden', '', 0);
                 INSERT INTO blocked_artists(artist_id, name) VALUES('UCbad', 'Bad Band');
                 INSERT INTO blocked_artists(artist_id, name) VALUES('name:slop ai', 'Slop AI');",
            )
            .unwrap();
        });
        crate::blocked::block(&db, crate::blocked::BlockedArtist { id: None, name: "Old".into() });
        let f = Filter::load(&db);
        assert!(f.is_blocked(&song("hidden", "X", "Y", None, "3:00")));
        assert!(f.is_blocked(&song("a", "X", "Slop AI", None, "3:00")));
        assert!(f.is_blocked(&song("b", "X", "Old", None, "3:00")));
        let mut by_id = song("c", "X", "Renamed", None, "3:00");
        by_id.artist_id = Some("UCbad".into());
        assert!(f.is_blocked(&by_id));
        assert!(!f.is_blocked(&song("ok", "X", "Fine", None, "3:00")));
    }

    #[test]
    fn explicit_only_rejected_when_the_filter_is_on() {
        let mut s = song("e", "X", "Y", None, "3:00");
        s.explicit = true;
        assert!(!Filter::new(&[], vec![], false).rejects(&s));
        assert!(Filter::new(&[], vec![], true).rejects(&s));
        let mut f = Filter::new(&[], vec![], true);
        let plain = song("p", "X", "Y", None, "3:00");
        assert!(f.explicit_unknown(&plain));
        f.learn_explicit("p", true);
        assert!(f.rejects(&plain));
    }
}
