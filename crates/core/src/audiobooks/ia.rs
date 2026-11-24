//! Internet Archive and LibriVox: URLs and response parsing (pure, fixture-tested).

use serde_json::Value;

use super::{show_id, Audiobook, AudiobookPage};
use crate::podcasts::{rss_id, sanitize, Episode};

const SEARCH: &str = "https://archive.org/advancedsearch.php";
const FIELDS: [&str; 6] = ["identifier", "title", "creator", "runtime", "language", "subject"];

/// Genre shelves and tiles: (IA subject, label, tile colour).
pub const GENRES: [(&str, &str, &str); 12] = [
    ("mystery", "Mystery", "#be123c"),
    ("romance", "Romance", "#15803d"),
    ("adventure", "Adventure", "#9d2bb0"),
    ("science fiction", "Science fiction", "#0e6ba8"),
    ("fantasy", "Fantasy", "#a16207"),
    ("horror", "Horror", "#3f7d20"),
    ("children", "Kids", "#b0279f"),
    ("humor", "Comedy", "#d6246e"),
    ("history", "History", "#0b7a75"),
    ("biography", "Biography", "#6a2fd8"),
    ("philosophy", "Philosophy", "#1f4fa8"),
    ("poetry", "Poetry", "#c2410c"),
];

pub fn genre_label(subject: &str) -> Option<&'static str> {
    GENRES.iter().find(|g| g.0.eq_ignore_ascii_case(subject)).map(|g| g.1)
}

/// Card art: the Archive's item tile (LibriVox uploads the cover as the tile).
pub fn tile_url(id: &str) -> String {
    format!("https://archive.org/services/img/{id}")
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Lucene-safe user text: letters, digits and spaces only (quotes and operators can't break out).
fn clean_query(q: &str) -> String {
    q.chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// An advancedsearch URL over LibriVox books, most downloaded first.
fn search_url(q: &str, rows: usize, page: usize) -> String {
    let base = format!("collection:librivoxaudio AND mediatype:audio AND NOT identifier:Librivox* AND ({q})");
    let fl: String = FIELDS.iter().map(|f| format!("&fl%5B%5D={f}")).collect();
    format!("{SEARCH}?q={}{fl}&sort%5B%5D=downloads+desc&rows={rows}&page={page}&output=json", enc(&base))
}

/// Books whose title or author matches `query`. `None` when the query has no searchable text.
pub fn query_url(query: &str, rows: usize) -> Option<String> {
    let q = clean_query(query);
    if q.is_empty() {
        return None;
    }
    Some(search_url(&format!("title:({q}) OR creator:({q})"), rows, 1))
}

/// English books, most downloaded first; with a subject for a genre shelf.
pub fn popular_url(subject: Option<&str>, rows: usize, page: usize) -> String {
    let lang = "language:(eng OR english)";
    match subject {
        Some(s) => search_url(&format!("{lang} AND subject:\"{}\"", clean_query(s)), rows, page),
        None => search_url(lang, rows, page),
    }
}

pub fn metadata_url(id: &str) -> String {
    format!("https://archive.org/metadata/{}", enc(id))
}

pub fn librivox_url(librivox_id: &str) -> String {
    format!("https://librivox.org/api/feed/audiobooks?id={}&extended=1&format=json", enc(librivox_id))
}

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(x) => Some(x.trim().to_owned()).filter(|x| !x.is_empty()),
        Value::Array(a) => a.iter().find_map(Value::as_str).map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn list(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::String(x)) => x.split(';').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

/// `"13:06:44"` / `"18:52"` / `"1132.5"` → seconds.
pub fn parse_length(t: &str) -> Option<i64> {
    let t = t.trim();
    if t.contains(':') {
        return sanitize::parse_clock(t);
    }
    t.parse::<f64>().ok().filter(|x| *x > 0.0).map(|x| x.round() as i64)
}

/// Our genre labels for a book's subjects (in [`GENRES`] order, the first three).
fn genres_of(subjects: &[String]) -> Vec<String> {
    GENRES
        .iter()
        .filter(|g| subjects.iter().any(|s| s.trim().eq_ignore_ascii_case(g.0)))
        .map(|g| g.1.to_owned())
        .take(3)
        .collect()
}

fn language_name(code: &str) -> String {
    match code.to_ascii_lowercase().as_str() {
        "eng" | "en" | "english" => "English".into(),
        "ger" | "deu" | "de" | "german" => "German".into(),
        "fre" | "fra" | "fr" | "french" => "French".into(),
        "spa" | "es" | "spanish" => "Spanish".into(),
        "ita" | "it" | "italian" => "Italian".into(),
        "dut" | "nld" | "nl" | "dutch" => "Dutch".into(),
        _ => code.to_owned(),
    }
}

/// advancedsearch docs → books (card fields only).
pub fn parse_search(v: &Value) -> Vec<Audiobook> {
    let docs = v.pointer("/response/docs").and_then(Value::as_array);
    docs.map(|d| {
        d.iter()
            .filter_map(|r| {
                let id = s(r, "identifier")?;
                Some(Audiobook {
                    thumbnail: Some(tile_url(&id)),
                    title: s(r, "title")?,
                    author: s(r, "creator").unwrap_or_default(),
                    duration_secs: s(r, "runtime").and_then(|t| parse_length(&t)),
                    language: s(r, "language").map(|l| language_name(&l)),
                    genres: genres_of(&list(r, "subject")),
                    id,
                    ..Default::default()
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// `"Pride and Prejudice (version 3)"` → `"pride and prejudice"`: one card per book on a shelf.
pub fn title_key(title: &str) -> String {
    let t = title.to_lowercase();
    let t = match t.find(" (version") {
        Some(i) => &t[..i],
        None => t.as_str(),
    };
    let t = t.split(", by ").next().unwrap_or(t);
    t.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep the most downloaded recording of each title (the list arrives most downloaded first).
pub fn dedupe(books: Vec<Audiobook>) -> Vec<Audiobook> {
    let mut seen = std::collections::HashSet::new();
    books.into_iter().filter(|b| seen.insert((title_key(&b.title), b.author.to_lowercase()))).collect()
}

/// LibriVox boilerplate the Archive puts around the summary.
fn strip_boilerplate(text: &str) -> String {
    let drop = |p: &str| {
        let l = p.to_lowercase();
        l.contains("librivox recording of")
            || l.contains("for more information on our readers")
            || l.contains("for more free audiobooks")
            || l.contains("download m4b")
            || l.starts_with("http")
    };
    text.lines().map(str::trim).filter(|p| !p.is_empty() && !drop(p)).collect::<Vec<_>>().join("\n")
}

/// `<em>Pride and Prejudice</em> is…<br />(Summary from Wikipedia)` → plain text.
pub fn plain(html: &str) -> String {
    let with_breaks = html
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace("</div>", "\n")
        .replace("</p>", "\n");
    with_breaks.lines().map(sanitize::to_text).map(|l| l.trim().to_owned()).collect::<Vec<_>>().join("\n")
}

/// The chapter files: one MP3 derivative per chapter, in reading order.
fn fmt(f: &Value) -> &str {
    f.get("format").and_then(Value::as_str).unwrap_or("")
}

fn chapter_files(files: &[Value]) -> Vec<&Value> {
    let pick = ["64Kbps MP3", "VBR MP3", "128Kbps MP3"]
        .into_iter()
        .find(|want| files.iter().any(|f| fmt(f) == *want))
        .unwrap_or("64Kbps MP3");
    let mut out: Vec<&Value> = files.iter().filter(|f| fmt(f) == pick).collect();
    // Order by the original's track number ("3/37"), else by file name.
    let track = |f: &Value| -> Option<u32> {
        let t = s(f, "track").or_else(|| {
            let orig = s(f, "original")?;
            files.iter().find(|o| s(o, "name").as_deref() == Some(orig.as_str())).and_then(|o| s(o, "track"))
        })?;
        t.split('/').next()?.trim().parse().ok()
    };
    out.sort_by(|a, b| {
        (track(a).unwrap_or(u32::MAX), s(a, "name")).cmp(&(track(b).unwrap_or(u32::MAX), s(b, "name")))
    });
    out
}

/// The cover image: the uploaded JPEG (not its thumb), else the item tile.
fn cover(id: &str, files: &[Value]) -> String {
    files
        .iter()
        .filter(|f| s(f, "format").as_deref() == Some("JPEG"))
        .filter_map(|f| s(f, "name"))
        .find(|n| !n.starts_with("__"))
        .map(|n| format!("https://archive.org/download/{id}/{}", enc(&n)))
        .unwrap_or_else(|| tile_url(id))
}

/// `archive.org/metadata/<id>` → the book page (no narrator yet; [`apply_librivox`] adds it).
/// Also returns the LibriVox project id (`call_number`) for that lookup.
pub fn parse_metadata(v: &Value) -> Option<(AudiobookPage, Option<String>)> {
    let m = v.get("metadata")?;
    let id = s(m, "identifier")?;
    let files = v.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    let thumb = cover(&id, &files);
    let title = s(m, "title").unwrap_or_else(|| id.clone());
    let chapters: Vec<Episode> = chapter_files(&files)
        .into_iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let name = s(f, "name")?;
            let url = format!("https://archive.org/download/{id}/{}", enc(&name));
            Some(Episode {
                id: rss_id(&url),
                show_id: Some(show_id(&id)),
                show_title: title.clone(),
                title: s(f, "title").unwrap_or_else(|| format!("Chapter {}", i + 1)),
                duration_secs: s(f, "length").and_then(|l| parse_length(&l)),
                thumbnail: Some(thumb.clone()),
                enclosure_url: Some(url),
                ..Default::default()
            })
        })
        .collect();
    if chapters.is_empty() {
        return None;
    }
    let total: i64 = chapters.iter().filter_map(|c| c.duration_secs).sum();
    let description = s(m, "description").map(|d| strip_boilerplate(&plain(&d))).filter(|d| !d.is_empty());
    let book = Audiobook {
        title,
        author: s(m, "creator").unwrap_or_default(),
        thumbnail: Some(thumb),
        description,
        duration_secs: s(m, "runtime").and_then(|t| parse_length(&t)).or(Some(total).filter(|t| *t > 0)),
        chapter_count: Some(chapters.len() as u32),
        genres: genres_of(&list(m, "subject")),
        language: s(m, "language").map(|l| language_name(&l)),
        id: id.clone(),
        ..Default::default()
    };
    let lv = s(m, "call_number").filter(|c| c.chars().all(|ch| ch.is_ascii_digit()));
    Some((AudiobookPage { book, chapters, ..Default::default() }, lv))
}

/// "Kristen McQuillin" for a solo reading, "A and B" for two, else "Various narrators".
pub fn narrator(readers: &[String]) -> Option<String> {
    let mut uniq: Vec<&String> = Vec::new();
    for r in readers {
        if !uniq.contains(&r) {
            uniq.push(r);
        }
    }
    match uniq.as_slice() {
        [] => None,
        [one] => Some((*one).clone()),
        [a, b] => Some(format!("{a} and {b}")),
        _ => Some("Various narrators".into()),
    }
}

/// Reader names drop LibriVox's "(1969-2023)" life dates.
fn reader_name(n: &str) -> String {
    match n.find(" (") {
        Some(i) if n.ends_with(')') => n[..i].trim().to_owned(),
        _ => n.trim().to_owned(),
    }
}

/// Add LibriVox's narrator, genres and clean summary to a page built from the Archive.
pub fn apply_librivox(page: &mut AudiobookPage, v: &Value) {
    let Some(b) = v.pointer("/books/0") else { return };
    let readers: Vec<String> = b
        .get("sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|sec| sec.get("readers").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|r| s(&r, "display_name").map(|n| reader_name(&n)))
        .collect();
    if let Some(n) = narrator(&readers) {
        page.book.narrator = Some(n);
    }
    let genres: Vec<String> = b
        .get("genres")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| s(g, "name"))
        .take(3)
        .collect();
    if !genres.is_empty() {
        page.book.genres = genres;
    }
    if let Some(d) = s(b, "description").map(|d| plain(&d)).filter(|d| !d.trim().is_empty()) {
        page.book.description = Some(d.trim().to_owned());
    }
    page.librivox_url = s(b, "url_librivox");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/src/audiobooks/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn search_parses_books_most_downloaded_first() {
        let books = parse_search(&fixture("ia_search_pride.json"));
        assert!(books.len() >= 4);
        let b = &books[0];
        assert_eq!(b.id, "pride_and_prejudice_librivox");
        assert_eq!(b.title, "Pride and Prejudice");
        assert_eq!(b.author, "Jane Austen");
        assert_eq!(b.duration_secs, Some(13 * 3600 + 6 * 60 + 44));
        assert_eq!(b.thumbnail.as_deref(), Some("https://archive.org/services/img/pride_and_prejudice_librivox"));
        // Shelves keep one card per title; search keeps every recording.
        let one = dedupe(books.clone());
        assert_eq!(one.iter().filter(|b| title_key(&b.title) == "pride and prejudice").count(), 1);
        assert!(one.len() < books.len());
    }

    #[test]
    fn metadata_gives_chapters_in_order_with_cover() {
        let (page, lv) = parse_metadata(&fixture("ia_metadata_pride.json")).unwrap();
        assert_eq!(lv.as_deref(), Some("253"));
        assert_eq!(page.book.author, "Jane Austen");
        assert_eq!(
            page.book.thumbnail.as_deref(),
            Some("https://archive.org/download/pride_and_prejudice_librivox/Pride_Prejudice_1104.jpg")
        );
        let c = &page.chapters;
        assert!(c.len() >= 10);
        assert_eq!(c[0].title, "Chapters 1-3");
        assert_eq!(c[2].title, "Chapter 6");
        assert_eq!(c[0].duration_secs, Some(18 * 60 + 52));
        assert_eq!(
            c[0].id,
            "PODURL:https://archive.org/download/pride_and_prejudice_librivox/prideandprejudice_01-03_austen_64kb.mp3"
        );
        assert_eq!(c[0].show_id.as_deref(), Some("abook:pride_and_prejudice_librivox"));
        let d = page.book.description.unwrap();
        assert!(d.starts_with("Pride and Prejudice is the most famous"), "{d}");
        assert!(!d.to_lowercase().contains("librivox recording"));
    }

    #[test]
    fn librivox_adds_narrators_genres_summary() {
        let (mut page, _) = parse_metadata(&fixture("ia_metadata_pride.json")).unwrap();
        apply_librivox(&mut page, &fixture("librivox_253.json"));
        assert_eq!(page.book.narrator.as_deref(), Some("Various narrators"));
        assert_eq!(page.book.genres, vec!["Romance".to_owned()]);
        assert!(page.book.description.unwrap().contains("(Summary from Wikipedia)"));
        assert_eq!(page.librivox_url.as_deref(), Some("https://librivox.org/pride-and-prejudice-by-jane-austen/"));
    }

    #[test]
    fn narrator_wording() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(narrator(&v(&["Ann", "Ann"])).as_deref(), Some("Ann"));
        assert_eq!(narrator(&v(&["Ann", "Bo"])).as_deref(), Some("Ann and Bo"));
        assert_eq!(narrator(&v(&["Ann", "Bo", "Cy"])).as_deref(), Some("Various narrators"));
        assert_eq!(narrator(&[]), None);
        assert_eq!(reader_name("Kara Shallenberg (1969-2023)"), "Kara Shallenberg");
    }

    #[test]
    fn urls_escape_user_text() {
        let u = query_url("pride \"and\" prejudice) OR x:", 20).unwrap();
        assert!(u.contains("title%3A%28pride%20and%20prejudice%20OR%20x%29"), "{u}");
        assert!(query_url("  ()  ", 20).is_none());
        assert!(popular_url(Some("science fiction"), 12, 1).contains("subject%3A%22science%20fiction%22"));
        assert_eq!(parse_length("1132.5"), Some(1133));
        assert_eq!(genre_label("Mystery"), Some("Mystery"));
    }
}
