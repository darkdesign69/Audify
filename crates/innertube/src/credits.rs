//! ── lane magic-d1 ── Song credits (MAGIC-GAPS #6): YouTube Music's "View song credits" dialog.
//!
//! The menu item browses `MPTC<videoId>` and gets back a popup rather than a page (shape captured
//! live 2026-11-15, fixtures `tests/fixtures/credits_*.json`):
//!
//! ```text
//! onResponseReceivedActions[0].openPopupAction.popup.dismissableDialogRenderer
//!   .sections[].dismissableDialogContentSectionRenderer { title.runs ("Written by"),
//!                                                         subtitle.runs (names, "\n" between) }
//!   .metadata.musicMultiRowListItemRenderer { title, subtitle (artist runs) }
//! ```
//!
//! Only songs (ATV) carry sections; a music video (OMV) answers with the header and no sections,
//! so the caller retries on the video's audio twin. Serialised as
//! `{ sections: [{ title, people: [{ name, artistId? }] }], source? }` (the D6 card's contract). The last section is the label that supplied
//! the data ("Music metadata provided by"), which Spotify shows as "Source".

use serde::Serialize;
use serde_json::Value;

use crate::clients::YouTubeClient;
use crate::models::context::Context;
use crate::transport::{Error, InnerTube};

/// One credited person or act.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreditPerson {
    pub name: String,
    /// The artist's channel (`UC…`) when YouTube links it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
}

/// "Written by" → [Freddie Mercury].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreditSection {
    /// The role as YouTube words it: "Written by".
    pub title: String,
    pub people: Vec<CreditPerson>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SongCredits {
    pub sections: Vec<CreditSection>,
    /// Who supplied the data (the record label), Spotify's "Source".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl SongCredits {
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

fn names(runs: Option<&Value>) -> Vec<CreditPerson> {
    let Some(runs) = runs.and_then(Value::as_array) else { return Vec::new() };
    let mut out: Vec<CreditPerson> = Vec::new();
    for r in runs {
        let text = r.get("text").and_then(Value::as_str).unwrap_or("");
        let id = r
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(Value::as_str)
            .filter(|b| b.starts_with("UC"))
            .map(str::to_owned);
        // One run per name, "\n" runs between them; a run can also hold "A\nB". Never split on
        // commas: "Earth, Wind & Fire" is one act.
        for part in text.split('\n').map(str::trim).filter(|p| !p.is_empty()) {
            if !out.iter().any(|n| n.name == part) {
                out.push(CreditPerson { name: part.to_owned(), artist_id: id.clone() });
            }
        }
    }
    out
}

/// Is this the data-source section? Its title is the only one that is not a role. English is
/// matched by wording; in any locale it is the last section and YouTube's metadata label.
fn is_source(title: &str) -> bool {
    let t = title.to_lowercase();
    t.contains("provided by") || t.contains("metadata")
}

pub fn parse_credits(value: &Value) -> SongCredits {
    let mut out = SongCredits::default();
    let Some(sections) = value
        .pointer("/onResponseReceivedActions/0/openPopupAction/popup/dismissableDialogRenderer/sections")
        .and_then(Value::as_array)
    else {
        return out;
    };
    for s in sections {
        let Some(sec) = s.get("dismissableDialogContentSectionRenderer") else { continue };
        let role: String = sec
            .pointer("/title/runs")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        let role = role.trim().to_owned();
        let names = names(sec.pointer("/subtitle/runs"));
        if role.is_empty() || names.is_empty() {
            continue;
        }
        if is_source(&role) {
            out.source = Some(names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", "));
            continue;
        }
        out.sections.push(CreditSection { title: role, people: names });
    }
    // The header's artist runs carry channel ids the sections lack: lend them to matching names.
    let header = value.pointer(
        "/onResponseReceivedActions/0/openPopupAction/popup/dismissableDialogRenderer/metadata/musicMultiRowListItemRenderer/subtitle/runs",
    );
    let linked: Vec<CreditPerson> = names(header).into_iter().filter(|n| n.artist_id.is_some()).collect();
    for g in &mut out.sections {
        for n in &mut g.people {
            if n.artist_id.is_none() {
                n.artist_id = linked.iter().find(|l| l.name == n.name).and_then(|l| l.artist_id.clone());
            }
        }
    }
    out
}

impl InnerTube {
    /// The credits of `video_id`. Empty for a music video (ask its audio twin) or a song YouTube
    /// has no credits for. Anonymous: credits are public and need no account.
    pub async fn song_credits(
        &self,
        client: &YouTubeClient,
        video_id: &str,
    ) -> Result<SongCredits, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            context: Context,
            browse_id: String,
        }
        let body = Body {
            context: self.context_anonymous(client),
            browse_id: format!("MPTC{video_id}"),
        };
        let value = self.post("browse", client, &body, false).await?;
        Ok(parse_credits(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn song_credits_parse_roles_names_and_source() {
        let c = parse_credits(&fixture("credits_song.json"));
        let roles: Vec<&str> = c.sections.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(roles, ["Performed by", "Written by", "Produced by"]);
        let produced: Vec<&str> = c.sections[2].people.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(produced, ["Queen", "Roy Thomas Baker"]);
        assert_eq!(c.source.as_deref(), Some("Columbia"));
        // "Queen" picks up the header's channel link; Freddie Mercury has none.
        assert_eq!(c.sections[0].people[0].artist_id.as_deref(), Some("UCEPMVbUzImPl4p8k4LkGevA"));
        assert_eq!(c.sections[1].people[0].artist_id, None);
    }

    #[test]
    fn serialises_to_the_ui_contract() {
        let c = parse_credits(&fixture("credits_song.json"));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["sections"][0]["title"], "Performed by");
        assert_eq!(v["sections"][0]["people"][0]["artistId"], "UCEPMVbUzImPl4p8k4LkGevA");
        assert!(v["sections"][1]["people"][0].get("artistId").is_none());
        assert_eq!(v["source"], "Columbia");
    }

    #[test]
    fn a_music_video_has_no_credits() {
        assert!(parse_credits(&fixture("credits_video.json")).is_empty());
        assert!(parse_credits(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn names_split_on_newlines_only_without_repeats() {
        let runs = serde_json::json!([{ "text": "A\nB" }, { "text": "\n" }, { "text": "B" }, { "text": "Earth, Wind & Fire" }]);
        let n: Vec<String> = names(Some(&runs)).into_iter().map(|n| n.name).collect();
        assert_eq!(n, ["A", "B", "Earth, Wind & Fire"]);
    }
}
