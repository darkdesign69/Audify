//! Live checks against the real services. Not in the default run:
//! `LIBRARY_PATH=$(brew --prefix)/lib cargo test -p limusic-app --lib podcasts::live -- --ignored --nocapture`

use super::*;

async fn enclosure_reachable(url: &str) -> u16 {
    crate::http::client()
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-1023")
        .header(reqwest::header::USER_AGENT, "Encore/1.0")
        .send()
        .await
        .map(|r| r.status().as_u16())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore]
async fn live_two_real_feeds() {
    for url in ["https://feeds.simplecast.com/54nAGcIl", "https://feeds.megaphone.fm/hubermanlab"] {
        let (show, eps, etag, lm) = rss::fetch_show(url).await.unwrap();
        println!(
            "{url}: {:?} by {:?}, {} episodes, etag={etag:?} last-modified={lm:?}",
            show.title,
            show.author,
            eps.len()
        );
        assert!(!show.title.is_empty());
        assert!(eps.len() > 10);
        let e = &eps[0];
        println!(
            "  newest: {:?} at {:?}, {:?}s, notes {} chars, enclosure {:?}",
            e.title,
            e.published_at,
            e.duration_secs,
            e.description.as_deref().map(str::len).unwrap_or(0),
            e.enclosure_url
        );
        assert!(e.duration_secs.unwrap_or(0) > 60);
        assert!(e.published_at.is_some());
        let status = enclosure_reachable(e.enclosure_url.as_deref().unwrap()).await;
        println!("  enclosure GET (range) → {status}");
        assert!(status == 200 || status == 206);
        // A second fetch with the validators should be cheap (304) when the host supports it.
        if etag.is_some() || lm.is_some() {
            match rss::fetch(url, etag.as_deref(), lm.as_deref()).await.unwrap() {
                rss::Fetched::NotModified => println!("  conditional GET → 304"),
                rss::Fetched::Body { .. } => println!("  conditional GET → 200 (host ignores validators)"),
            }
        }
    }
}

#[tokio::test]
#[ignore]
async fn live_youtube_podcast_and_directory() {
    let it = innertube::InnerTube::new(innertube::Session::default(), None).unwrap();
    let clients = innertube::Clients::bundled();
    let c = clients.get(innertube::METADATA_CLIENT).unwrap();
    let shows = youtube::search_shows(&it, c, "huberman lab").await.unwrap();
    println!("YT shows: {:?}", shows.iter().take(3).map(|s| (&s.title, &s.author, &s.id)).collect::<Vec<_>>());
    let show = shows.first().expect("a show");
    let page = youtube::fetch_show(&it, c, &show.id, Sort::Newest).await.unwrap();
    println!("{} episodes on page 1, more: {}", page.episodes.len(), page.continuation.is_some());
    let e = &page.episodes[0];
    println!("newest: {:?} {:?} {:?}s", e.title, e.published_at, e.duration_secs);
    assert!(e.published_at.is_some() && e.duration_secs.is_some());
    let oldest = youtube::fetch_show(&it, c, &show.id, Sort::Oldest).await.unwrap();
    println!("oldest: {:?}", oldest.episodes.first().map(|e| &e.title));
    assert!(oldest.episodes.first().unwrap().published_at < e.published_at);
    let more = youtube::fetch_more(&it, c, page.continuation.as_deref().unwrap(), &show.id, &show.title).await.unwrap();
    println!("page 2: {} episodes", more.0.len());
    let full = youtube::fetch_episode(&it, c, &e.id).await.unwrap();
    println!("episode page notes: {} chars", full.description.as_deref().map(str::len).unwrap_or(0));
    let eps = youtube::search_episodes(&it, c, "huberman sleep").await.unwrap();
    println!("YT episode search: {} rows, first {:?}", eps.len(), eps.first().map(|e| (&e.title, &e.show_title)));

    let dir = std::env::temp_dir().join(format!("encore-podcasts-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = crate::db::Db::open(&dir.join("t.sqlite")).unwrap();
    store::init(&db.podcasts_conn()).unwrap();
    let it_shows = itunes::search_shows(&db, "huberman lab").await.unwrap();
    let merged = search::merge_shows(shows.clone(), it_shows.clone());
    println!(
        "merged: {} (yt {} + itunes {}); first = {:?} feed {:?}",
        merged.len(),
        shows.len(),
        it_shows.len(),
        merged[0].title,
        merged[0].feed_url
    );
    let segs = sponsorblock::segments(&db, &e.id).await;
    println!("sponsorblock segments for {}: {segs:?}", e.id);
    // A popular non-podcast video with known segments proves the API path end to end.
    let known = sponsorblock::segments(&db, "dQw4w9WgXcQ").await;
    println!("sponsorblock sanity (dQw4w9WgXcQ): {} segments", known.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// stream: chapters. Coverage benchmark: for ~30 popular shows, the newest episodes on RSS and on
/// YouTube, which source has chapters (each source on its own, and the first-hit chain). Writes
/// `$CHAPTERS_BENCH_OUT` (TSV) when set.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_chapters_coverage() {
    use futures_util::StreamExt;
    const SHOWS: [&str; 30] = [
        "The Joe Rogan Experience", "Huberman Lab", "Lex Fridman Podcast", "The Daily", "Call Her Daddy",
        "Crime Junkie", "This American Life", "Stuff You Should Know", "SmartLess", "The Diary Of A CEO",
        "Modern Wisdom", "Freakonomics Radio", "Planet Money", "Radiolab", "Hidden Brain",
        "The Tim Ferriss Show", "My First Million", "All-In with Chamath, Jason, Sacks & Friedberg", "Acquired", "Pod Save America",
        "The Rest Is History", "The Mel Robbins Podcast", "Armchair Expert with Dax Shepard", "Conan O'Brien Needs A Friend", "99% Invisible",
        "Hard Fork", "Darknet Diaries", "Syntax - Tasty Web Development Treats", "The Changelog", "Podcasting 2.0",
    ];
    const PER_SHOW: usize = 5;
    let dir = std::env::temp_dir().join(format!("encore-chapters-live-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = crate::db::Db::open(&dir.join("t.sqlite")).unwrap();
    store::init(&db.podcasts_conn()).unwrap();
    let it = innertube::InnerTube::new(innertube::Session::default(), None).unwrap();
    let clients = innertube::Clients::bundled();
    let (meta, web) = (clients.get(innertube::METADATA_CLIENT).unwrap(), clients.get("WEB").unwrap());
    let n = |v: Vec<chapters::Chapter>, d: Option<i64>| chapters::clean(v, d.map(|d| d as f64)).len();
    let mut rows: Vec<String> = vec!["show\tside\tepisode\tfeed\tid3\tnotes\tyoutube\tdescription\tsponsorblock".into()];
    for name in SHOWS {
        // RSS: the directory's first hit for the name.
        let feed = itunes::search_shows(&db, name).await.ok().and_then(|s| s.into_iter().find_map(|s| s.feed_url));
        if let Some(feed) = feed {
            match rss::fetch_show(&feed).await {
                Ok((_, eps, _, _)) => {
                    let eps: Vec<Episode> = eps.into_iter().take(PER_SHOW).collect();
                    let out = futures_util::stream::iter(eps.iter().map(|e| async move {
                        let f = match &e.chapters_url { Some(u) => n(chapters::from_feed(u).await, e.duration_secs), None => 0 };
                        let i = n(chapters::from_id3(e.enclosure_url.as_deref().unwrap_or_default()).await, e.duration_secs);
                        let notes = e.description.as_deref().map(sanitize::to_text).unwrap_or_default();
                        let d = n(chapters::parse_description(&notes), e.duration_secs);
                        format!("{name}\trss\t{}\t{f}\t{i}\t{d}\t\t\t", e.title.replace('\t', " "))
                    }))
                    .buffered(5)
                    .collect::<Vec<_>>()
                    .await;
                    rows.extend(out);
                }
                Err(e) => println!("{name}: feed {feed} failed: {e}"),
            }
        } else {
            println!("{name}: no feed in the directory");
        }
        // YouTube: the show search's first hit.
        let show = youtube::search_shows(&it, meta, name).await.ok().and_then(|s| s.into_iter().next());
        if let Some(show) = show {
            if let Ok(page) = youtube::fetch_show(&it, meta, &show.id, Sort::Newest).await {
                let eps: Vec<Episode> = page.episodes.into_iter().take(PER_SHOW).collect();
                let (it, db) = (&it, &db);
                let out = futures_util::stream::iter(eps.iter().map(|e| async move {
                    let v = it.podcast_watch_json(web, &e.id).await.unwrap_or_default();
                    let y = n(chapters::parse_youtube(&v), e.duration_secs);
                    let d = n(chapters::parse_description(&chapters::youtube_description(&v).unwrap_or_default()), e.duration_secs);
                    let s = n(sponsorblock::chapters(db, &e.id).await, e.duration_secs);
                    format!("{name}\tyoutube\t{}\t\t\t\t{y}\t{d}\t{s}", e.title.replace('\t', " "))
                }))
                .buffered(5)
                .collect::<Vec<_>>()
                .await;
                rows.extend(out);
            }
        } else {
            println!("{name}: not on YouTube Music");
        }
    }
    // Summary: share of episodes with chapters, per source and for the first-hit chain.
    let cols: Vec<Vec<&str>> = rows.iter().skip(1).map(|r| r.split('\t').collect()).collect();
    let has = |r: &Vec<&str>, i: usize| r.get(i).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0) >= 2;
    for side in ["rss", "youtube"] {
        let rs: Vec<&Vec<&str>> = cols.iter().filter(|r| r[1] == side).collect();
        let shows: std::collections::HashSet<&str> = rs.iter().map(|r| r[0]).collect();
        let idx: &[usize] = if side == "rss" { &[3, 4, 5] } else { &[6, 7, 8] };
        let mut line = format!("{side}: {} episodes from {} shows", rs.len(), shows.len());
        for &i in idx {
            let hit = rs.iter().filter(|r| has(r, i)).count();
            let show_hit: std::collections::HashSet<&str> = rs.iter().filter(|r| has(r, i)).map(|r| r[0]).collect();
            line += &format!(" | {} {hit}/{} eps, {} shows", rows[0].split('\t').nth(i).unwrap(), rs.len(), show_hit.len());
        }
        let any = rs.iter().filter(|r| idx.iter().any(|&i| has(r, i))).count();
        line += &format!(" | any {any}/{}", rs.len());
        println!("{line}");
    }
    if let Ok(out) = std::env::var("CHAPTERS_BENCH_OUT") {
        std::fs::write(out, rows.join("\n")).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
