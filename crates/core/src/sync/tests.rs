//! Sync: merge correctness between two (or three) in-memory devices, the LAN transport and pairing
//! over loopback, and the relay against an in-process mailbox.

use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use super::relay::{Mailbox, Memory};
use super::store::{self, Op};
use super::*;

/// A device with every table the app has, sync installed, not paired.
fn device(mailbox: Option<Mailbox>) -> Arc<Engine> {
    let db = Arc::new(Db::open(std::path::Path::new(":memory:")).unwrap());
    crate::smart::migrate(&db);
    crate::polish::migrate(&db);
    crate::history::migrate(&db);
    crate::podcasts::store::init(&db.conn()).unwrap();
    crate::audiobooks::store::init(&db.conn()).unwrap();
    Engine::new(db, None, mailbox)
}

/// Pair without the network: same group on both, `a` the inviter.
fn pair(a: &Arc<Engine>, b: &Arc<Engine>) {
    let key = crypto::random_bytes::<32>();
    let pair_id = crypto::hex(&crypto::random_bytes::<16>());
    for (e, base) in [(a, 1), (b, 0)] {
        let c = e.db.conn();
        save_group(&c, &pair_id, &key, 0).unwrap();
        store::enable(&c, base).unwrap();
    }
    let (da, db_) = (a.device_id(), b.device_id());
    upsert_peer(&a.db.conn(), &db_, Some("b"), Some("ios"), None, None);
    upsert_peer(&b.db.conn(), &da, Some("a"), Some("mac"), None, None);
}

/// What `from` has that `to` hasn't acknowledged, the way the LAN exchange picks it.
fn outbox(from: &Engine, to: &Engine) -> Vec<Op> {
    let c = from.db.conn();
    let sent: i64 = c.query_row("SELECT sent FROM sync_peers WHERE dev = ?1", [to.device_id()], |r| r.get(0)).unwrap();
    let upto = store::head(&c);
    let (ops, cursor, _) = store::ops_since(&c, sent, upto, Some(&to.device_id()), None, usize::MAX / 2).unwrap();
    c.execute("UPDATE sync_peers SET sent = ?2 WHERE dev = ?1", params![to.device_id(), cursor]).unwrap();
    ops
}

/// One direction of delivery.
fn send(from: &Engine, to: &Engine) -> Applied {
    let ops = outbox(from, to);
    to.apply(&ops)
}

/// Both directions, until quiet.
fn sync(a: &Engine, b: &Engine) {
    for _ in 0..4 {
        send(a, b);
        send(b, a);
    }
}

fn sql(e: &Engine, s: &str) {
    e.db.conn().execute_batch(s).unwrap();
}

fn rows(e: &Engine, q: &str) -> Vec<String> {
    let c = e.db.conn();
    let mut st = c.prepare(q).unwrap();
    let r = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
    r.map(Result::unwrap).collect()
}

fn one<T: rusqlite::types::FromSql>(e: &Engine, q: &str) -> Option<T> {
    e.db.conn().query_row(q, [], |r| r.get(0)).optional().unwrap()
}

fn like(e: &Engine, vid: &str) {
    e.db.conn()
        .execute(
            "INSERT OR REPLACE INTO liked_songs(video_id, title, artists, song_json, added_at) VALUES(?1, ?1, 'x', '{}', 100)",
            [vid],
        )
        .unwrap();
}

fn unlike(e: &Engine, vid: &str) {
    e.db.conn().execute("DELETE FROM liked_songs WHERE video_id = ?1", [vid]).unwrap();
}

fn liked(e: &Engine) -> Vec<String> {
    rows(e, "SELECT video_id FROM liked_songs ORDER BY video_id")
}

fn set(e: &Engine, k: &str, v: &str) {
    e.db.set_setting(k, v);
}

/// The SQLite clock is in milliseconds: make sure the next write reads a later one.
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(3));
}

fn own_ops(e: &Engine) -> usize {
    let c = e.db.conn();
    store::ops_since(&c, 0, i64::MAX, None, Some(&store::device_id(&c)), usize::MAX / 2).unwrap().0.len()
}

/// Playlist helpers in the shape library.rs writes them.
fn new_playlist(e: &Engine, title: &str) -> i64 {
    let c = e.db.conn();
    c.execute("INSERT INTO local_playlists(title, description, created_at, updated_at) VALUES(?1, '', 100, 100)", [title])
        .unwrap();
    c.last_insert_rowid()
}

fn add_track(e: &Engine, pl: i64, vid: &str, pos: i64) {
    e.db.conn()
        .execute(
            "INSERT INTO local_playlist_tracks(playlist_id, video_id, song_json, added_at, pos) VALUES(?1, ?2, '{}', 100, ?3)",
            params![pl, vid, pos],
        )
        .unwrap();
}

fn playlist_id(e: &Engine, title: &str) -> i64 {
    one(e, &format!("SELECT id FROM local_playlists WHERE title = '{title}'")).unwrap()
}

fn tracks(e: &Engine, pl: i64) -> Vec<String> {
    rows(e, &format!("SELECT video_id FROM local_playlist_tracks WHERE playlist_id = {pl} ORDER BY pos, video_id"))
}

// ── merge ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn nothing_is_logged_until_paired() {
    let a = device(None);
    like(&a, "v1");
    set(&a, "crossfade", "true");
    assert_eq!(one::<i64>(&a, "SELECT COUNT(*) FROM sync_field"), Some(0));
}

#[test]
fn concurrent_edits_last_writer_wins_per_field() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    set(&a, "crossfade", "true");
    tick();
    set(&b, "crossfade", "false");
    // Different fields of one playlist edited on each side: both edits survive.
    let pl = new_playlist(&a, "Mix");
    sync(&a, &b);
    let pb = playlist_id(&b, "Mix");
    sql(&a, &format!("UPDATE local_playlists SET description = 'from a' WHERE id = {pl}"));
    tick();
    sql(&b, &format!("UPDATE local_playlists SET title = 'Mix B' WHERE id = {pb}"));
    sync(&a, &b);
    for e in [&a, &b] {
        assert_eq!(e.db.get_setting("crossfade").as_deref(), Some("false"), "the later write wins");
        assert_eq!(rows(e, "SELECT title || '|' || description FROM local_playlists"), vec!["Mix B|from a"]);
    }
}

#[test]
fn device_local_settings_never_sync() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    set(&a, "volume", "12");
    set(&a, "session_cookie", "secret");
    set(&a, "downloads_dir", "/Users/x");
    set(&a, "locale", "de");
    sync(&a, &b);
    assert_eq!(b.db.get_setting("volume"), None);
    assert_eq!(b.db.get_setting("session_cookie"), None);
    assert_eq!(b.db.get_setting("downloads_dir"), None);
    assert_eq!(b.db.get_setting("locale").as_deref(), Some("de"));
}

#[test]
fn like_unlike_races() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    // Both like the same song offline, then one unlikes it later: gone everywhere.
    like(&a, "x");
    tick();
    like(&b, "x");
    tick();
    unlike(&a, "x");
    sync(&a, &b);
    assert!(liked(&a).is_empty() && liked(&b).is_empty());
    // Unliked on one side, liked again later on the other: back everywhere.
    like(&a, "y");
    sync(&a, &b);
    unlike(&a, "y");
    tick();
    like(&b, "y");
    sync(&a, &b);
    assert_eq!(liked(&a), vec!["y"]);
    assert_eq!(liked(&b), vec!["y"]);
}

#[test]
fn playlist_add_remove_reorder() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    // B has a playlist of its own first, so the same playlist gets a different local id there.
    new_playlist(&b, "Other");
    let pa = new_playlist(&a, "Road");
    for (i, v) in ["a", "b", "c", "d"].iter().enumerate() {
        add_track(&a, pa, v, i as i64);
    }
    // Pinned and filed in a folder by its LOCALPLAYLIST:<id>.
    sql(&a, &format!("INSERT INTO lib_pins(item_id, kind, position, pinned_at) VALUES('LOCALPLAYLIST:{pa}', 'playlist', 0, 100)"));
    sync(&a, &b);
    let pb = playlist_id(&b, "Road");
    assert_ne!(pa, pb);
    assert_eq!(tracks(&b, pb), vec!["a", "b", "c", "d"]);
    assert_eq!(rows(&b, "SELECT item_id FROM lib_pins"), vec![format!("LOCALPLAYLIST:{pb}")]);

    // Offline: B reverses the order, A removes "b" and adds "e".
    for (i, v) in ["d", "c", "b", "a"].iter().enumerate() {
        sql(&b, &format!("UPDATE local_playlist_tracks SET pos = {i} WHERE playlist_id = {pb} AND video_id = '{v}'"));
    }
    sql(&a, &format!("DELETE FROM local_playlist_tracks WHERE playlist_id = {pa} AND video_id = 'b'"));
    add_track(&a, pa, "e", 9);
    sync(&a, &b);
    // The reorder doesn't bring the removed track back, the add lands, both agree on the order.
    assert_eq!(tracks(&a, pa), vec!["d", "c", "a", "e"]);
    assert_eq!(tracks(&b, pb), tracks(&a, pa));

    // Adding the same song twice is two rows (duplicates are allowed), and both travel.
    add_track(&b, pb, "a", 10);
    sync(&a, &b);
    assert_eq!(tracks(&a, pa), vec!["d", "c", "a", "e", "a"]);

    // Deleting the playlist (hard purge) removes it and its pin everywhere.
    sql(&a, &format!("DELETE FROM local_playlist_tracks WHERE playlist_id = {pa}; DELETE FROM local_playlists WHERE id = {pa}; DELETE FROM lib_pins;"));
    sync(&a, &b);
    assert_eq!(rows(&b, "SELECT title FROM local_playlists"), vec!["Other"]);
    assert_eq!(one::<i64>(&b, &format!("SELECT COUNT(*) FROM local_playlist_tracks WHERE playlist_id = {pb}")), Some(0));
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM lib_pins"), Some(0));
}

#[test]
fn progress_updates_and_episode_snapshot() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    sql(&a, "INSERT INTO podcast_episodes(id, show_id, title, first_seen) VALUES('PODURL:1', 'show', 'Ep 1', 5)");
    let put = |e: &Engine, pos: f64| {
        e.db.conn()
            .execute(
                "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, updated_at) VALUES('PODURL:1', ?1, 3600, 0, 1)
                 ON CONFLICT(episode_id) DO UPDATE SET position_s = excluded.position_s",
                [pos],
            )
            .unwrap();
    };
    put(&a, 100.0);
    sql(&a, "INSERT INTO podcast_saved(episode_id, saved_at) VALUES('PODURL:1', 7)");
    sync(&a, &b);
    // B never loaded the show, but it can show the episode now.
    assert_eq!(one::<String>(&b, "SELECT title FROM podcast_episodes WHERE id = 'PODURL:1'").as_deref(), Some("Ep 1"));
    assert_eq!(one::<f64>(&b, "SELECT position_s FROM podcast_progress"), Some(100.0));
    assert_eq!(one::<i64>(&b, "SELECT saved_at FROM podcast_saved"), Some(7));
    // Many updates offline cost one field each.
    for p in 1..=50 {
        put(&b, 100.0 + p as f64);
    }
    let ops = outbox(&b, &a);
    assert_eq!(ops.len(), 1);
    a.apply(&ops);
    assert_eq!(one::<f64>(&a, "SELECT position_s FROM podcast_progress"), Some(150.0));
    // Marked played on A later wins over B's earlier position.
    tick();
    sql(&a, "UPDATE podcast_progress SET played = 1, played_at = 9");
    sync(&a, &b);
    assert_eq!(one::<i64>(&b, "SELECT played FROM podcast_progress"), Some(1));
    assert_eq!(one::<f64>(&b, "SELECT position_s FROM podcast_progress"), Some(150.0));
}

#[test]
fn deletes_propagate() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    sql(&a, "INSERT INTO library_items(id, kind, title, card_json, added_at) VALUES('MPRE1', 'album', 'Album', '{}', 1);
             INSERT INTO podcast_follows(id, title, followed_at) VALUES('show1', 'Show', 1);
             INSERT INTO lib_folders(id, name, position, created_at, updated_at) VALUES('FOLDER:1', 'F', 0, 1, 1);");
    sync(&a, &b);
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM library_items"), Some(1));
    sql(&b, "DELETE FROM library_items; DELETE FROM podcast_follows; DELETE FROM lib_folders;");
    sync(&a, &b);
    for e in [&a, &b] {
        assert_eq!(one::<i64>(e, "SELECT (SELECT COUNT(*) FROM library_items) + (SELECT COUNT(*) FROM podcast_follows) + (SELECT COUNT(*) FROM lib_folders)"), Some(0));
    }
    // Clearing history deletes listens on both.
    sql(&a, "INSERT INTO listens(video_id, played_at, secs) VALUES('v', 10, 30)");
    sync(&a, &b);
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM listens"), Some(1));
    b.db.clear_listens();
    sync(&a, &b);
    assert_eq!(one::<i64>(&a, "SELECT COUNT(*) FROM listens"), Some(0));
}

#[test]
fn redelivery_is_idempotent_and_never_echoes() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    like(&a, "x");
    set(&a, "autoplay", "false");
    let ops = outbox(&a, &b);
    let first = b.apply(&ops);
    assert!(first.fields > 0);
    let head = store::head(&b.db.conn());
    let again = b.apply(&ops);
    assert_eq!(again.fields, 0);
    assert!(again.entities.is_empty());
    assert_eq!(store::head(&b.db.conn()), head, "re-delivery writes nothing");
    // B recorded no ops of its own for what it applied: nothing to echo back.
    assert_eq!(own_ops(&b), 0);
    assert!(outbox(&b, &a).is_empty());
}

#[test]
fn out_of_order_delivery_converges() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    like(&a, "x");
    set(&a, "crossfade_secs", "4");
    let first = outbox(&a, &b);
    tick();
    unlike(&a, "x");
    set(&a, "crossfade_secs", "8");
    like(&a, "y");
    let second = outbox(&a, &b);
    // The later batch arrives first.
    b.apply(&second);
    b.apply(&first);
    assert_eq!(liked(&b), vec!["y"]);
    assert_eq!(b.db.get_setting("crossfade_secs").as_deref(), Some("8"));
    // Even op by op in reverse.
    let c = device(None);
    pair(&a, &c);
    let mut all = store::all_ops(&a.db.conn()).unwrap();
    all.reverse();
    for op in &all {
        c.apply(std::slice::from_ref(op));
    }
    assert_eq!(liked(&c), vec!["y"]);
}

#[test]
fn a_track_arriving_before_its_playlist_waits() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    let pl = new_playlist(&a, "Late");
    add_track(&a, pl, "t", 0);
    let ops = outbox(&a, &b);
    let (tracks_first, rest): (Vec<Op>, Vec<Op>) = ops.into_iter().partition(|o| o.e == "local_playlist_tracks");
    b.apply(&tracks_first);
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM local_playlist_tracks"), Some(0));
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM sync_pending"), Some(1));
    b.apply(&rest);
    assert_eq!(tracks(&b, playlist_id(&b, "Late")), vec!["t"]);
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM sync_pending"), Some(0));
}

#[test]
fn offline_changes_queue_and_go_in_one_exchange() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    sync(&a, &b);
    for i in 0..20 {
        like(&a, &format!("v{i:02}"));
    }
    sql(&a, "INSERT INTO listens(video_id, played_at, secs) VALUES('v00', 1000, 10)");
    sql(&a, "UPDATE listens SET secs = 200");
    let pending = a.status()["pending"].as_i64().unwrap();
    assert!(pending > 0, "status shows the queue");
    send(&a, &b);
    assert_eq!(liked(&b).len(), 20);
    assert_eq!(one::<i64>(&b, "SELECT secs FROM listens"), Some(200));
    assert_eq!(a.status()["pending"].as_i64(), Some(0));
}

#[test]
fn listens_are_append_only_and_deduplicated() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    sql(&a, "INSERT INTO listen_songs(video_id, song_json) VALUES('v', '{\"t\":1}');
             INSERT INTO listens(video_id, played_at, secs) VALUES('v', 100, 5);");
    sql(&b, "INSERT INTO listens(video_id, played_at, secs) VALUES('v', 200, 50);");
    sync(&a, &b);
    sync(&a, &b);
    for e in [&a, &b] {
        assert_eq!(rows(e, "SELECT played_at || ':' || secs FROM listens ORDER BY played_at"), vec!["100:5", "200:50"]);
    }
    // The playing listen's seconds grow on A and follow.
    sql(&a, "UPDATE listens SET secs = 60 WHERE played_at = 100");
    sync(&a, &b);
    assert_eq!(one::<i64>(&b, "SELECT secs FROM listens WHERE played_at = 100"), Some(60));
    // Skips and the private flag (a reference to the listen's local id) travel too.
    sql(&a, "INSERT INTO smart_events(video_id, kind, secs, at) VALUES('v', 'skip', 3, 300);
             INSERT INTO smart_private_listens(listen_id) SELECT id FROM listens WHERE played_at = 200;");
    sync(&a, &b);
    assert_eq!(one::<String>(&b, "SELECT kind FROM smart_events").as_deref(), Some("skip"));
    assert_eq!(
        one::<i64>(&b, "SELECT l.played_at FROM smart_private_listens p JOIN listens l ON l.id = p.listen_id"),
        Some(200)
    );
}

#[test]
fn blocked_artists_merge_as_a_set() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    let blocked = |e: &Engine| {
        let mut v: Vec<String> = crate::blocked::list(&e.db).into_iter().map(|x| x.key().to_string()).collect();
        v.sort();
        v
    };
    crate::blocked::block(&a.db, crate::blocked::BlockedArtist { id: Some("UCa".into()), name: "A".into() });
    crate::blocked::block(&b.db, crate::blocked::BlockedArtist { id: None, name: "Bee".into() });
    sync(&a, &b);
    assert_eq!(blocked(&a), vec!["Bee", "UCa"]);
    assert_eq!(blocked(&b), vec!["Bee", "UCa"]);
    tick();
    crate::blocked::unblock(&b.db, "UCa");
    sync(&a, &b);
    assert_eq!(blocked(&a), vec!["Bee"]);
}

#[test]
fn initial_snapshot_merges_two_full_libraries() {
    let (a, b) = (device(None), device(None));
    // Before pairing, each has its own library.
    like(&a, "both");
    like(&a, "only-a");
    like(&b, "both");
    like(&b, "only-b");
    let pa = new_playlist(&a, "A list");
    add_track(&a, pa, "s1", 0);
    let pb = new_playlist(&b, "B list");
    add_track(&b, pb, "s2", 0);
    set(&a, "crossfade", "true");
    set(&b, "crossfade", "false");
    set(&b, "shuffle_mode", "standard");
    sql(&a, "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, updated_at) VALUES('e', 10, 100, 0, 1000)");
    sql(&b, "INSERT INTO podcast_progress(episode_id, position_s, duration_s, played, updated_at) VALUES('e', 90, 100, 0, 2000)");
    sql(&a, "INSERT INTO listens(video_id, played_at, secs) VALUES('s1', 1, 40)");
    sql(&b, "INSERT INTO listens(video_id, played_at, secs) VALUES('s2', 2, 40)");
    pair(&a, &b);
    sync(&a, &b);
    for e in [&a, &b] {
        assert_eq!(liked(e), vec!["both", "only-a", "only-b"], "likes are a union");
        assert_eq!(rows(e, "SELECT title FROM local_playlists ORDER BY title"), vec!["A list", "B list"]);
        assert_eq!(e.db.get_setting("crossfade").as_deref(), Some("true"), "the inviting device's settings win ties");
        assert_eq!(e.db.get_setting("shuffle_mode").as_deref(), Some("standard"), "settings only one side had come over");
        assert_eq!(one::<f64>(e, "SELECT position_s FROM podcast_progress"), Some(90.0), "the newer progress wins");
        assert_eq!(one::<i64>(e, "SELECT COUNT(*) FROM listens"), Some(2));
    }
    assert_eq!(tracks(&a, playlist_id(&a, "B list")), vec!["s2"]);
    assert_eq!(tracks(&b, playlist_id(&b, "A list")), vec!["s1"]);
    // And after the first exchange, nothing more flows.
    assert!(outbox(&a, &b).is_empty() && outbox(&b, &a).is_empty());
}

#[test]
fn three_devices_converge_through_one_in_the_middle() {
    let (a, b, c) = (device(None), device(None), device(None));
    pair(&a, &b);
    // C joins B's group (same key), knows only B.
    {
        let g = b.group().unwrap();
        let conn = c.db.conn();
        save_group(&conn, &g.pair_id, &g.key, 0).unwrap();
        store::enable(&conn, 0).unwrap();
    }
    upsert_peer(&b.db.conn(), &c.device_id(), None, None, None, None);
    upsert_peer(&c.db.conn(), &b.device_id(), None, None, None, None);
    like(&a, "from-a");
    like(&c, "from-c");
    for _ in 0..3 {
        sync(&a, &b);
        sync(&b, &c);
    }
    for e in [&a, &b, &c] {
        assert_eq!(liked(e), vec!["from-a", "from-c"]);
    }
}

#[test]
fn unpairing_stops_capture_and_keeps_data() {
    let (a, b) = (device(None), device(None));
    pair(&a, &b);
    like(&a, "x");
    sync(&a, &b);
    b.forget_group();
    assert!(b.group().is_none());
    assert_eq!(liked(&b), vec!["x"]);
    like(&b, "y");
    assert_eq!(one::<i64>(&b, "SELECT COUNT(*) FROM sync_field"), Some(0));
}

// ── transport ────────────────────────────────────────────────────────────────────────────────

#[test]
fn qr_round_trip() {
    let q = net::qr_payload("012345", &["192.168.1.4".into()], 47615, "dev1", "Mac Studio");
    let p = net::parse_qr(&q).unwrap();
    assert_eq!(p.code, "012345");
    assert_eq!(p.addrs, vec!["192.168.1.4:47615"]);
    assert_eq!(p.dev.as_deref(), Some("dev1"));
    assert_eq!(net::split_addr("[fe80::1]:5"), ("fe80::1".into(), Some(5)));
    assert_eq!(net::split_addr("10.0.0.2:47615"), ("10.0.0.2".into(), Some(47615)));
}

#[tokio::test]
async fn pake_pairing_and_lan_sync_over_loopback() {
    let (mac, phone) = (device(None), device(None));
    like(&mac, "mac-song");
    like(&phone, "phone-song");
    let port = mac.ensure_server_loopback().await;
    phone.ensure_server_loopback().await;
    let p = mac.start_pairing().await.unwrap();
    let code = p["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    let addr = format!("127.0.0.1:{port}");

    // A wrong code fails on both sides and costs a try.
    let wrong = if code == "000000" { "111111" } else { "000000" };
    assert_eq!(phone.join(wrong, &[addr.clone()]).await.unwrap_err(), "wrong_code:2", "with the tries left");
    assert!(phone.group().is_none() && mac.group().is_none());

    // The right one pairs, and the join's first round merges both libraries.
    let st = phone.join(&code, &[addr.clone()]).await.unwrap();
    assert_eq!(st["paired"], true);
    assert_eq!(mac.group().unwrap().pair_id, phone.group().unwrap().pair_id);
    assert_eq!(mac.group().unwrap().key, phone.group().unwrap().key);
    assert_eq!(liked(&phone), vec!["mac-song", "phone-song"]);
    assert_eq!(liked(&mac), vec!["mac-song", "phone-song"]);
    // The code is spent.
    assert_eq!(phone.join(&code, &[addr.clone()]).await.unwrap_err(), "not_pairing");

    // Changes on either side move with the next round (the phone is the client here).
    like(&mac, "later-mac");
    unlike(&phone, "phone-song");
    phone.sync_round(true).await;
    assert_eq!(liked(&mac), vec!["later-mac", "mac-song"]);
    assert_eq!(liked(&phone), liked(&mac));
    // The Mac learned the phone's address and can sync the other way too.
    like(&phone, "from-phone");
    mac.sync_round(true).await;
    assert_eq!(liked(&mac), vec!["from-phone", "later-mac", "mac-song"]);
    assert_eq!(phone.status()["peers"][0]["lan"], true);

    // Unpairing on the phone tells the Mac, and both stop.
    phone.unpair().await;
    assert!(phone.group().is_none());
    assert!(mac.group().is_none(), "the Mac was left alone and unpaired too");
    assert_eq!(liked(&mac).len(), 3, "data stays");
}

#[tokio::test]
async fn pairing_code_gets_three_tries() {
    let (mac, phone) = (device(None), device(None));
    let port = mac.ensure_server_loopback().await;
    let code = mac.start_pairing().await.unwrap()["code"].as_str().unwrap().to_string();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let addr = vec![format!("127.0.0.1:{port}")];
    for left in (0..PAIRING_TRIES).rev() {
        assert_eq!(phone.join(wrong, &addr).await.unwrap_err(), format!("wrong_code:{left}"));
    }
    assert!(mac.status()["pairing"].is_null(), "the last wrong try withdraws the code");
    assert_eq!(phone.join(&code, &addr).await.unwrap_err(), "not_pairing");
}

#[tokio::test]
async fn lan_server_refuses_strangers() {
    let (mac, phone, stranger) = (device(None), device(None), device(None));
    let port = mac.ensure_server_loopback().await;
    pair(&mac, &phone);
    // The stranger has the right pair id but its own key.
    let g = mac.group().unwrap();
    {
        let c = stranger.db.conn();
        save_group(&c, &g.pair_id, &crypto::random_bytes::<32>(), 0).unwrap();
        store::enable(&c, 0).unwrap();
    }
    like(&stranger, "evil");
    let sg = stranger.group().unwrap();
    let peer = Peer { id: mac.device_id(), name: String::new(), platform: String::new(), addr: None, port: None, sent: 0, recv: 0, last_seen: 0 };
    let r = net::exchange(&stranger, &sg, &peer, &format!("127.0.0.1:{port}"), false).await;
    assert_eq!(r.unwrap_err(), "auth");
    assert!(liked(&mac).is_empty());
}

#[tokio::test]
async fn relay_carries_changes_and_recovers_from_expiry() {
    let mem = Arc::new(Mutex::new(Memory::default()));
    let a = device(Some(Mailbox::Memory(mem.clone())));
    let b = device(Some(Mailbox::Memory(mem.clone())));
    pair(&a, &b);
    like(&a, "x");
    a.sync_round(true).await;
    b.sync_round(true).await;
    assert_eq!(liked(&b), vec!["x"]);
    // The relay holds only ciphertext.
    let stored: String = mem.lock().unwrap().items.iter().map(|i| i.sealed.clone()).collect();
    assert!(!stored.contains("liked_songs") && !stored.contains("\"x\""));
    // B posts only its own changes; A doesn't apply its own messages back.
    like(&b, "y");
    b.sync_round(true).await;
    a.sync_round(true).await;
    assert_eq!(liked(&a), vec!["x", "y"]);
    let a_head = store::head(&a.db.conn());
    a.sync_round(true).await;
    assert_eq!(store::head(&a.db.conn()), a_head, "nothing echoes");

    // B is away for longer than the relay keeps messages.
    like(&a, "z1");
    a.sync_round(true).await;
    like(&a, "z2");
    a.sync_round(true).await;
    let last = mem.lock().unwrap().next_id;
    mem.lock().unwrap().expire_through(last);
    like(&a, "z3");
    a.sync_round(true).await;
    b.sync_round(true).await; // sees the gap, asks for a resync; gets z3 meanwhile
    assert_eq!(liked(&b), vec!["x", "y", "z3"]);
    a.sync_round(true).await; // answers with everything
    b.sync_round(true).await;
    assert_eq!(liked(&b), vec!["x", "y", "z1", "z2", "z3"]);

    // B unpairs while A is away; A hears it from the relay, leaves too, and deletes the mailbox.
    b.unpair().await;
    assert!(b.group().is_none());
    a.sync_round(true).await;
    assert!(a.group().is_none());
    assert!(mem.lock().unwrap().items.is_empty(), "the mailbox is gone");
    assert_eq!(liked(&a).len(), 5, "data stays");
}

#[tokio::test]
async fn relay_tampering_is_ignored() {
    let mem = Arc::new(Mutex::new(Memory::default()));
    let a = device(Some(Mailbox::Memory(mem.clone())));
    let b = device(Some(Mailbox::Memory(mem.clone())));
    pair(&a, &b);
    like(&a, "x");
    a.sync_round(true).await;
    {
        let mut m = mem.lock().unwrap();
        let item = m.items[0].clone();
        // A forged message under A's name, and a bit-flipped copy of a real one.
        let mut raw = crypto::unb64(&item.sealed).unwrap();
        let n = raw.len();
        raw[n - 5] ^= 1;
        m.items[0].sealed = crypto::b64(&raw);
        let forged = crypto::BoxKey::for_group(&[9u8; 32]).seal("x", b"{}");
        m.next_id += 1;
        let id = m.next_id;
        m.items.push(relay::Item { id, from: a.device_id(), sealed: forged, at: 0 });
    }
    b.sync_round(true).await;
    assert!(liked(&b).is_empty());
}

#[test]
fn triggers_cover_every_synced_table() {
    let a = device(None);
    let c: &Connection = &a.db.conn();
    for spec in store::SPECS {
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1 AND name LIKE 'sync%'", [spec.table], |r| r.get(0))
            .unwrap();
        assert!(n >= 2, "{} has no capture triggers", spec.table);
    }
}
