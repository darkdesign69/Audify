//! The audiobook commands, shared by the desktop's Tauri wrappers (`src-tauri/src/audiobooks.rs`)
//! and the iOS core (`crates/ffi`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{book_id, book_progress, ia, resume, store, Audiobook, AudiobookPage};
use crate::podcasts::store as pstore;
use crate::smart::home::{item, shelf, Shelf, ShelfItem};
use crate::state::AppState;

const UA: &str = "Spotless/1.0";
/// Shelves and search change slowly; a book's files never do (the cache table keeps a week).
/// A TTL of 0 means "don't cache the raw body" (the parsed book page is cached instead).
const LIST_TTL: i64 = 24 * 3600;
const PAGE_TTL: i64 = 7 * 86_400;
const SHELF_LEN: usize = 16;
/// Genres with their own shelf on the Audiobooks home (the rest are tiles).
const SHELF_GENRES: usize = 6;

async fn get_json(st: &AppState, url: &str, ttl: i64, timeout: Duration) -> Result<Value, String> {
    if ttl > 0 {
        if let Some(v) = pstore::cache_get(&st.db.podcasts_conn(), url, ttl).and_then(|b| serde_json::from_str(&b).ok()) {
            return Ok(v);
        }
    }
    let resp = crate::http::client()
        .get(url)
        .header(reqwest::header::USER_AGENT, UA)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if ttl > 0 {
        pstore::cache_put(&st.db.podcasts_conn(), url, &body);
    }
    Ok(v)
}

fn mark(st: &AppState, books: &mut [Audiobook]) {
    let conn = st.db.podcasts_conn();
    for b in books.iter_mut() {
        b.saved = store::is_saved(&conn, &b.id);
    }
}

/// Books whose title or author matches, most listened first (every recording of a title).
pub async fn audiobook_search(state: &Arc<AppState>, query: String) -> Result<Vec<Audiobook>, String> {
    let Some(url) = ia::query_url(&query, 30) else { return Ok(Vec::new()) };
    let mut books = ia::parse_search(&get_json(state, &url, LIST_TTL, Duration::from_secs(12)).await?);
    mark(state, &mut books);
    Ok(books)
}

/// A genre's books (`genre` is an [`ia::GENRES`] subject; empty or `popular` = all), one card per
/// title, `page` from 1.
pub async fn audiobook_genre(state: &Arc<AppState>, genre: String, page: Option<usize>) -> Result<Vec<Audiobook>, String> {
    let subject = Some(genre.trim()).filter(|g| !g.is_empty() && *g != "popular");
    let url = ia::popular_url(subject, 60, page.unwrap_or(1).max(1));
    let mut books = ia::dedupe(ia::parse_search(&get_json(state, &url, LIST_TTL, Duration::from_secs(12)).await?));
    mark(state, &mut books);
    Ok(books)
}

pub fn book_item(b: &Audiobook) -> ShelfItem {
    ShelfItem {
        subtitle: Some(b.author.clone()).filter(|a| !a.is_empty()),
        thumbnail: b.thumbnail.clone(),
        progress: b.progress,
        ..item("audiobook", &b.id, &b.title)
    }
}

/// The genre tiles; `art` maps a genre to its top book's cover (the tile's art, the way podcast
/// categories take their chart's first show).
fn genre_tiles(art: &HashMap<&str, String>) -> Shelf {
    let tiles = ia::GENRES
        .iter()
        .map(|(subject, label, color)| ShelfItem {
            params: Some((*subject).to_owned()),
            color: Some((*color).to_owned()),
            thumbnail: art.get(subject).cloned(),
            ..item("audiobook_genre", subject, label)
        })
        .collect();
    shelf("ab_genres", "Browse by genre", "moods", "local", tiles)
}

/// The Audiobooks home (Home's chip and Browse ▸ Audiobooks): continue listening, your books,
/// popular, genre tiles (each with its top book's cover), then a shelf per leading genre. Shelves
/// that fail to load are left out.
pub async fn audiobook_home(state: &Arc<AppState>) -> Vec<Shelf> {
    let mut out = Vec::new();
    {
        let conn = state.db.podcasts_conn();
        let cont = store::continue_listening(&conn, SHELF_LEN).unwrap_or_default();
        if !cont.is_empty() {
            out.push(shelf("ab_continue", "Continue listening", "cards", "local", cont.iter().map(book_item).collect()));
        }
        let saved = store::saved(&conn).unwrap_or_default();
        if !saved.is_empty() {
            out.push(shelf("ab_saved", "Your audiobooks", "cards", "local", saved.iter().map(book_item).collect()));
        }
    }
    // Popular plus every genre: the leading genres get a shelf, each genre's top book is its tile's art.
    let wanted: Vec<Option<&str>> = std::iter::once(None).chain(ia::GENRES.iter().map(|g| Some(g.0))).collect();
    let lists = futures_util::future::join_all(wanted.iter().map(|g| {
        let url = ia::popular_url(*g, SHELF_LEN * 2, 1);
        async move { get_json(state, &url, LIST_TTL, Duration::from_secs(10)).await.ok() }
    }))
    .await;
    let lists: Vec<(Option<&str>, Vec<Audiobook>)> = wanted
        .into_iter()
        .zip(lists)
        .map(|(g, v)| (g, v.map(|v| ia::dedupe(ia::parse_search(&v)).into_iter().take(SHELF_LEN).collect()).unwrap_or_default()))
        .collect();
    out.extend(home_shelves(&lists));
    out
}

/// Popular, the genre tiles, then the leading genres' shelves, from each list's books (`None` is
/// Popular). Empty lists get no shelf; a genre's first book with a cover is its tile's art.
fn home_shelves(lists: &[(Option<&str>, Vec<Audiobook>)]) -> Vec<Shelf> {
    let art: HashMap<&str, String> =
        lists.iter().filter_map(|(g, books)| Some(((*g)?, books.iter().find_map(|b| b.thumbnail.clone())?))).collect();
    let mut out = Vec::new();
    for (g, books) in lists.iter().take(SHELF_GENRES + 1) {
        if books.is_empty() {
            continue;
        }
        let items = books.iter().map(book_item).collect();
        match g {
            None => out.push(shelf("ab_popular", "Popular audiobooks", "cards", "youtube", items)),
            Some(s) => {
                let label = ia::genre_label(s).unwrap_or(s);
                let mut sh = shelf(&format!("ab_genre:{s}"), label, "cards", "youtube", items);
                sh.subtitle = Some("Audiobooks".into());
                out.push(sh);
            }
        }
    }
    // The tiles go right after Popular (first when Popular didn't load).
    let at = usize::from(out.first().is_some_and(|s| s.id == "ab_popular"));
    out.insert(at, genre_tiles(&art));
    out
}

async fn fetch_page(state: &Arc<AppState>, id: &str) -> Result<AudiobookPage, String> {
    let key = format!("abook:page:{id}");
    if let Some(p) = pstore::cache_get(&state.db.podcasts_conn(), &key, PAGE_TTL).and_then(|b| serde_json::from_str(&b).ok()) {
        return Ok(p);
    }
    let meta = get_json(state, &ia::metadata_url(id), 0, Duration::from_secs(15)).await?;
    let (mut page, lv) = ia::parse_metadata(&meta).ok_or("This audiobook has no chapters to play")?;
    // Narrator and genres are a nice-to-have: LibriVox's API can be slow.
    if let Some(lv) = lv {
        if let Ok(v) = get_json(state, &ia::librivox_url(&lv), 0, Duration::from_secs(6)).await {
            ia::apply_librivox(&mut page, &v);
        }
    }
    pstore::cache_put(&state.db.podcasts_conn(), &key, &serde_json::to_string(&page).unwrap_or_default());
    Ok(page)
}

/// The book page with live progress, saved state and where Play continues.
pub async fn audiobook_page(state: &Arc<AppState>, id: String) -> Result<AudiobookPage, String> {
    let mut page = fetch_page(state, &id).await?;
    let conn = state.db.podcasts_conn();
    pstore::upsert_episodes(&conn, &page.chapters).map_err(|e| e.to_string())?;
    pstore::decorate(&conn, &mut page.chapters);
    page.resume = resume(&page.chapters);
    page.book.progress = book_progress(&page.chapters);
    let _ = store::remember(&conn, &page.book);
    page.book.saved = store::is_saved(&conn, &id);
    Ok(page)
}

/// The book a playing chapter belongs to (Now Playing's chapter list), or `None` for anything else.
pub async fn audiobook_for_chapter(state: &Arc<AppState>, id: String) -> Result<Option<AudiobookPage>, String> {
    let show = pstore::get_episode(&state.db.podcasts_conn(), &id).and_then(|e| e.show_id);
    match show.as_deref().and_then(book_id) {
        Some(b) => audiobook_page(state, b.to_owned()).await.map(Some),
        None => Ok(None),
    }
}

/// Add to / remove from Your Library; returns the new state.
pub fn audiobook_save(state: &Arc<AppState>, book: Audiobook, saved: bool) -> Result<bool, String> {
    if book.id.is_empty() {
        return Err("audiobook id missing".into());
    }
    store::set_saved(&state.db.podcasts_conn(), &book, saved).map_err(|e| e.to_string())?;
    Ok(saved)
}

/// Your Library's audiobooks, most recently saved first.
pub fn audiobook_saved(state: &Arc<AppState>) -> Result<Vec<Audiobook>, String> {
    store::saved(&state.db.podcasts_conn()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(id: &str, art: bool) -> Audiobook {
        Audiobook { id: id.into(), title: id.into(), thumbnail: art.then(|| ia::tile_url(id)), ..Default::default() }
    }

    #[test]
    fn genre_tiles_take_their_top_books_cover() {
        let lists = vec![
            (None, vec![book("top", true)]),
            (Some("mystery"), vec![book("holmes", true), book("poirot", true)]),
            (Some("romance"), vec![book("bare", false), book("pride", true)]),
            (Some("poetry"), vec![]),
        ];
        let out = home_shelves(&lists);
        let ids: Vec<&str> = out.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["ab_popular", "ab_genres", "ab_genre:mystery", "ab_genre:romance"]);
        let tiles = &out[1];
        assert_eq!(tiles.items.len(), ia::GENRES.len(), "every genre keeps its tile");
        let art = |g: &str| tiles.items.iter().find(|t| t.id == g).and_then(|t| t.thumbnail.clone());
        assert_eq!(art("mystery"), Some(ia::tile_url("holmes")));
        assert_eq!(art("romance"), Some(ia::tile_url("pride")), "the first book with a cover");
        assert_eq!(art("poetry"), None, "no books: a plain colour tile");
    }

    #[test]
    fn tiles_lead_when_popular_fails() {
        let out = home_shelves(&[(None, vec![]), (Some("mystery"), vec![book("holmes", true)])]);
        assert_eq!(out[0].id, "ab_genres");
        assert_eq!(out[1].id, "ab_genre:mystery");
    }
}
