//! Books in SQLite (the podcasts connection): every opened book's card (for Continue listening)
//! and the saved ones (Your Library). Chapter progress lives in `podcast_progress`.

use rusqlite::{params, Connection, OptionalExtension};

use super::{Audiobook, SHOW_PREFIX};
use crate::podcasts::now_secs;

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS audiobook_books (
            id        TEXT PRIMARY KEY,
            json      TEXT NOT NULL,
            saved_at  INTEGER,
            opened_at INTEGER NOT NULL
        );",
    )
}

fn card(b: &Audiobook) -> String {
    // The card only: chapters and progress are read fresh.
    let c = Audiobook { saved: false, progress: None, ..b.clone() };
    serde_json::to_string(&c).unwrap_or_default()
}

/// Remember a book the user opened (keeps its saved state).
pub fn remember(conn: &Connection, b: &Audiobook) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO audiobook_books(id, json, opened_at) VALUES(?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, opened_at = excluded.opened_at",
        params![b.id, card(b), now_secs()],
    )?;
    Ok(())
}

pub fn set_saved(conn: &Connection, b: &Audiobook, saved: bool) -> rusqlite::Result<()> {
    if saved {
        conn.execute(
            "INSERT INTO audiobook_books(id, json, saved_at, opened_at) VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json, saved_at = excluded.saved_at",
            params![b.id, card(b), now_secs()],
        )?;
    } else {
        conn.execute("UPDATE audiobook_books SET saved_at = NULL WHERE id = ?1", [&b.id])?;
    }
    Ok(())
}

pub fn is_saved(conn: &Connection, id: &str) -> bool {
    conn.query_row("SELECT saved_at FROM audiobook_books WHERE id = ?1", [id], |r| r.get::<_, Option<i64>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
        .is_some()
}

/// Listening per book from its chapters: (book id, fraction heard, last listened, all played).
fn listening(conn: &Connection) -> rusqlite::Result<Vec<(String, f64, i64, bool)>> {
    let mut st = conn.prepare_cached(
        "SELECT e.show_id,
                SUM(COALESCE(e.duration_s, p.duration_s, 0)),
                SUM(CASE WHEN p.played = 1 THEN COALESCE(e.duration_s, p.duration_s, 0)
                         ELSE MIN(COALESCE(p.position_s, 0), COALESCE(e.duration_s, p.duration_s, 0)) END),
                MAX(COALESCE(p.updated_at, 0)),
                MIN(COALESCE(p.played, 0))
         FROM podcast_episodes e LEFT JOIN podcast_progress p ON p.episode_id = e.id
         WHERE e.show_id LIKE ?1
         GROUP BY e.show_id",
    )?;
    let rows = st.query_map([format!("{SHOW_PREFIX}%")], |r| {
        let sid: String = r.get(0)?;
        let total: f64 = r.get::<_, Option<f64>>(1)?.unwrap_or(0.0);
        let heard: f64 = r.get::<_, Option<f64>>(2)?.unwrap_or(0.0);
        let at: i64 = r.get(3)?;
        let done: i64 = r.get(4)?;
        let frac = if total > 0.0 { (heard / total).clamp(0.0, 1.0) } else { 0.0 };
        Ok((sid.trim_start_matches(SHOW_PREFIX).to_owned(), frac, at, done == 1))
    })?;
    rows.collect()
}

fn books(conn: &Connection, sql: &str) -> rusqlite::Result<Vec<(Audiobook, Option<i64>)>> {
    let mut st = conn.prepare(sql)?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|(j, saved)| serde_json::from_str::<Audiobook>(&j).ok().map(|b| (b, saved)))
        .collect())
}

/// Saved books, most recently saved first, with their progress.
pub fn saved(conn: &Connection) -> rusqlite::Result<Vec<Audiobook>> {
    let prog = listening(conn)?;
    let rows = books(conn, "SELECT json, saved_at FROM audiobook_books WHERE saved_at IS NOT NULL ORDER BY saved_at DESC")?;
    Ok(rows
        .into_iter()
        .map(|(mut b, _)| {
            b.saved = true;
            b.progress = prog.iter().find(|p| p.0 == b.id && p.2 > 0).map(|p| p.1);
            b
        })
        .collect())
}

/// Started and not finished, most recently listened first.
pub fn continue_listening(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<Audiobook>> {
    let mut prog: Vec<_> = listening(conn)?.into_iter().filter(|p| p.2 > 0 && !p.3).collect();
    prog.sort_by_key(|p| std::cmp::Reverse(p.2));
    let all = books(conn, "SELECT json, saved_at FROM audiobook_books")?;
    Ok(prog
        .into_iter()
        .filter_map(|(id, frac, _, _)| {
            let (mut b, saved) = all.iter().find(|(b, _)| b.id == id).cloned()?;
            b.saved = saved.is_some();
            b.progress = Some(frac);
            Some(b)
        })
        .take(limit)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::podcasts::{store as pstore, Episode};

    fn conn() -> Connection {
        let c = pstore::test_conn();
        init(&c).unwrap();
        c
    }

    fn book(id: &str) -> Audiobook {
        Audiobook { id: id.into(), title: format!("T {id}"), ..Default::default() }
    }

    #[test]
    fn save_and_continue() {
        let c = conn();
        let b = book("b1");
        remember(&c, &b).unwrap();
        assert!(!is_saved(&c, "b1"));
        set_saved(&c, &b, true).unwrap();
        assert!(is_saved(&c, "b1"));
        // Opening it again keeps it saved.
        remember(&c, &b).unwrap();
        assert!(is_saved(&c, "b1"));
        assert_eq!(saved(&c).unwrap().len(), 1);
        assert!(continue_listening(&c, 10).unwrap().is_empty());

        let eps: Vec<Episode> = (0..2)
            .map(|i| Episode {
                id: format!("PODURL:https://x/{i}.mp3"),
                show_id: Some("abook:b1".into()),
                title: format!("Ch {i}"),
                duration_secs: Some(100),
                ..Default::default()
            })
            .collect();
        pstore::upsert_episodes(&c, &eps).unwrap();
        pstore::save_position(&c, &eps[0].id, 50.0, 100.0).unwrap();
        let cont = continue_listening(&c, 10).unwrap();
        assert_eq!(cont.len(), 1);
        assert!((cont[0].progress.unwrap() - 0.25).abs() < 1e-9);
        assert!(cont[0].saved);

        set_saved(&c, &b, false).unwrap();
        assert!(saved(&c).unwrap().is_empty());
        // Every chapter played: no longer "continue".
        for e in &eps {
            pstore::set_played(&c, &e.id, true).unwrap();
        }
        assert!(continue_listening(&c, 10).unwrap().is_empty());
    }
}
