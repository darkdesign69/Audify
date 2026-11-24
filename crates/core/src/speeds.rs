//! Two speeds, never one: music and spoken word (podcast episodes, audiobooks) each keep their own.
//!
//! The owner's report: "When I put the podcast on 2x, now all songs are also on 2x." mpv has one
//! tempo, so the core keeps both and applies the one that belongs to whatever is playing, on every
//! track start (gapless advances and autoplay from an episode into music included).
//!
//! - **Music**: the session's Speed & pitch effects (tempo, pitch, room). Normally 1x, changed only
//!   from the music Speed & pitch sheet. Session-only, as before.
//! - **Spoken word**: one podcast speed, Spotify's model, remembered across launches in the
//!   `episode_speed` setting (the key the iPhone already used). Episodes play flat: no pitch, no
//!   room, whatever the music is set to.
//!
//! A change lands in the bucket it is aimed at (`Target`), or the playing item's when the caller
//! does not say. Changing one never touches the other.

use player::Reverb;

/// Tempo, semitones, room: what `Player::set_playback_params` takes.
pub type Effects = (f64, i32, Reverb);

/// The setting the spoken-word speed is kept under.
pub const EPISODE_SPEED_KEY: &str = "episode_speed";

/// Which speed a change is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Music,
    Spoken,
}

impl Target {
    /// `"music"` / `"episode"` from the command's optional `target` arg.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "music" | "song" => Some(Self::Music),
            "episode" | "podcast" | "spoken" | "audiobook" => Some(Self::Spoken),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Speeds {
    pub music: Effects,
    pub spoken: f64,
}

impl Default for Speeds {
    fn default() -> Self {
        Self { music: (1.0, 0, Reverb::default()), spoken: 1.0 }
    }
}

impl Speeds {
    /// With the remembered spoken-word speed (the setting's text; junk or out of range is 1x).
    pub fn with_spoken(saved: Option<&str>) -> Self {
        let spoken = saved
            .and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|s| player::SPEED_RANGE.contains(s))
            .unwrap_or(1.0);
        Self { spoken, ..Self::default() }
    }

    /// What an item plays with: spoken word at its own speed, flat; music with its effects.
    pub fn for_item(&self, spoken: bool) -> Effects {
        if spoken {
            (self.spoken, 0, Reverb { enabled: false, ..self.music.2 })
        } else {
            self.music
        }
    }

    /// Keep a change in its own bucket. A spoken-word change keeps only the tempo: an episode has
    /// no pitch or room to remember.
    pub fn set(&mut self, target: Target, effects: Effects) {
        match target {
            Target::Music => self.music = effects,
            Target::Spoken => self.spoken = effects.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Reverb {
        Reverb { enabled: true, ..Reverb::default() }
    }

    #[test]
    fn an_episode_at_2x_leaves_the_next_song_at_1x() {
        let mut s = Speeds::default();
        s.set(Target::Spoken, (2.0, 0, Reverb::default()));
        assert_eq!(s.for_item(true).0, 2.0);
        assert_eq!(s.for_item(false), (1.0, 0, Reverb::default()));
    }

    #[test]
    fn music_at_1_2x_leaves_an_episode_at_its_own_speed() {
        let mut s = Speeds::with_spoken(Some("1.5"));
        s.set(Target::Music, (1.2, 3, room()));
        assert_eq!(s.for_item(false), (1.2, 3, room()));
        let ep = s.for_item(true);
        assert_eq!((ep.0, ep.1, ep.2.enabled), (1.5, 0, false), "episodes play flat");
        // And back: the song gets its 1.2x, pitch and room again.
        assert_eq!(s.for_item(false), (1.2, 3, room()));
    }

    #[test]
    fn the_saved_spoken_speed_is_checked() {
        assert_eq!(Speeds::with_spoken(Some("2.5")).spoken, 2.5);
        assert_eq!(Speeds::with_spoken(Some("3")).spoken, 3.0);
        for junk in [None, Some(""), Some("fast"), Some("9"), Some("0.1"), Some("NaN")] {
            assert_eq!(Speeds::with_spoken(junk).spoken, 1.0, "{junk:?}");
        }
    }

    #[test]
    fn targets_parse() {
        assert_eq!(Target::parse("music"), Some(Target::Music));
        assert_eq!(Target::parse("episode"), Some(Target::Spoken));
        assert_eq!(Target::parse("nope"), None);
    }
}
