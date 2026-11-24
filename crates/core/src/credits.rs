//! ── lane magic-d1 ── Song credits (MAGIC-GAPS #6) for the track menu's "Show credits" and the
//! Now Playing card. Parsing lives in `innertube::credits`; this picks which id to ask.

use std::sync::Arc;

use innertube::credits::SongCredits;
use innertube::SongItem;
use serde::Serialize;

use crate::state::AppState;

/// What both apps show: YouTube's role sections (the credits sheet) and the same credits as one
/// row per person with every role they had (the Now Playing card: "Thom Yorke · Writer, Producer").
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(flatten)]
    pub credits: SongCredits,
    pub people: Vec<CreditRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditRow {
    pub name: String,
    /// The artist's channel when YouTube links it: a link plus Follow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    pub roles: Vec<String>,
}

impl From<SongCredits> for Credits {
    fn from(credits: SongCredits) -> Self {
        let people = credit_people(&credits);
        Credits { credits, people }
    }
}

/// "Produced by" → "Producer"; an unknown "X by" → "X"; anything else as YouTube wrote it.
pub fn role_label(role: &str) -> String {
    let r = role.trim();
    let known = match r.to_lowercase().as_str() {
        "performed by" => "Main Artist",
        "written by" => "Writer",
        "composed by" => "Composer",
        "lyrics by" => "Lyricist",
        "produced by" => "Producer",
        "arranged by" => "Arranger",
        "mixed by" => "Mixing Engineer",
        "mastered by" => "Mastering Engineer",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let lower = r.to_lowercase();
    match lower.strip_suffix(" by").map(|_| r[..r.len() - 3].trim_end()) {
        Some(w) if !w.is_empty() => {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
        _ => r.to_owned(),
    }
}

/// People in credit order (first role first), one row each, roles deduplicated. A name matched
/// case-insensitively keeps its first spelling and picks up a channel id from any of its rows.
pub fn credit_people(c: &SongCredits) -> Vec<CreditRow> {
    let mut out: Vec<CreditRow> = Vec::new();
    for s in &c.sections {
        let role = role_label(&s.title);
        for p in &s.people {
            let name = p.name.trim();
            if name.is_empty() {
                continue;
            }
            let key = name.to_lowercase();
            let i = match out.iter().position(|r| r.name.to_lowercase() == key) {
                Some(i) => i,
                None => {
                    out.push(CreditRow { name: name.to_owned(), artist_id: None, roles: Vec::new() });
                    out.len() - 1
                }
            };
            if out[i].artist_id.is_none() {
                out[i].artist_id.clone_from(&p.artist_id);
            }
            if !out[i].roles.contains(&role) {
                out[i].roles.push(role.clone());
            }
        }
    }
    out
}

/// Credits for `item`. YouTube keeps them on the song (ATV), so a music video asks its audio twin
/// when it has none of its own. A local file or podcast episode has none.
pub async fn song_credits(state: &Arc<AppState>, item: SongItem) -> Result<Credits, String> {
    raw_credits(state, item).await.map(Credits::from)
}

async fn raw_credits(state: &Arc<AppState>, item: SongItem) -> Result<SongCredits, String> {
    if crate::local::is_local_song(&item.video_id) || crate::podcasts::is_episode_item(&item) {
        return Ok(SongCredits::default());
    }
    let client = state.clients.get(innertube::METADATA_CLIENT).ok_or("no metadata client")?;
    let credits =
        state.it.song_credits(client, &item.video_id).await.map_err(|e| e.to_string())?;
    if !credits.is_empty() || !item.is_video {
        return Ok(credits);
    }
    let twin = match item.counterpart.clone() {
        Some(c) => Some(c),
        None => crate::canonical::audio_seed(state, &item).await,
    };
    match twin.filter(|t| t != &item.video_id) {
        Some(t) => state.it.song_credits(client, &t).await.map_err(|e| e.to_string()),
        None => Ok(credits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use innertube::credits::{CreditPerson, CreditSection};

    fn sec(title: &str, people: &[(&str, Option<&str>)]) -> CreditSection {
        CreditSection {
            title: title.into(),
            people: people.iter().map(|(n, id)| CreditPerson { name: (*n).into(), artist_id: id.map(Into::into) }).collect(),
        }
    }

    #[test]
    fn role_labels() {
        assert_eq!(role_label("Performed by"), "Main Artist");
        assert_eq!(role_label("Produced by"), "Producer");
        assert_eq!(role_label("Written by"), "Writer");
        assert_eq!(role_label("engineered by"), "Engineered");
        assert_eq!(role_label("Gitarre"), "Gitarre");
        assert_eq!(role_label("Featuring"), "Featuring");
    }

    #[test]
    fn one_row_per_person_with_every_role() {
        let c = SongCredits {
            sections: vec![
                sec("Performed by", &[("Radiohead", Some("UCq19"))]),
                sec("Written by", &[("Thom Yorke", None), ("Radiohead", None)]),
                sec("Produced by", &[("Nigel Godrich", None), ("thom yorke", Some("UCty"))]),
                sec("Produced by", &[("  ", None)]),
            ],
            source: Some("XL".into()),
        };
        let p = credit_people(&c);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].name.as_str(), p[0].roles.join(",")), ("Radiohead", "Main Artist,Writer".into()));
        assert_eq!(p[0].artist_id.as_deref(), Some("UCq19"));
        assert_eq!((p[1].name.as_str(), p[1].roles.join(",")), ("Thom Yorke", "Writer,Producer".into()));
        assert_eq!(p[1].artist_id.as_deref(), Some("UCty"), "an id from a later row is picked up");
        assert_eq!(p[2].artist_id, None);
        assert!(credit_people(&SongCredits::default()).is_empty());
        let json = serde_json::to_value(Credits::from(c)).unwrap();
        assert_eq!(json["source"], "XL");
        assert_eq!(json["sections"].as_array().unwrap().len(), 4);
        assert_eq!(json["people"][1]["roles"][1], "Producer");
    }
}
