//! Encore audio stream: the sleep timer's rules, kept free of clocks and mpv so they test in
//! microseconds. The app (`src-tauri/src/audio.rs`) ticks [`SleepTimer::tick`] a few times a
//! second and does what it says.

use serde::{Deserialize, Serialize};

/// How long the music fades out before the timer stops it (Spotify fades over the last seconds).
pub const FADE_SECS: f64 = 10.0;

/// The fade's floor. -50 dB under a normal listening level is inaudible in a bedroom.
pub const FADE_FLOOR_DB: f64 = -50.0;

/// The durations Spotify offers, in minutes.
pub const PRESET_MINUTES: [u32; 6] = [5, 10, 15, 30, 45, 60];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SleepMode {
    /// Stop at a wall-clock moment, `ends_at` in the caller's clock (seconds).
    #[serde(rename_all = "camelCase")]
    At { ends_at: f64 },
    /// Stop when the track (or podcast episode) that is playing ends.
    EndOfTrack,
}

/// What the driver should do after a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Nothing to do yet; the fade gain is 0 dB.
    Wait,
    /// Inside the fade: set the extra gain to this many dB (<= 0).
    Fade(f64),
    /// Time's up: pause, then reset the gain and disarm.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SleepTimer {
    pub mode: Option<SleepMode>,
}

impl SleepTimer {
    pub fn set_minutes(&mut self, now: f64, minutes: f64) {
        self.mode = Some(SleepMode::At { ends_at: now + minutes.max(0.0) * 60.0 });
    }

    pub fn set_end_of_track(&mut self) {
        self.mode = Some(SleepMode::EndOfTrack);
    }

    pub fn cancel(&mut self) {
        self.mode = None;
    }

    pub fn armed(&self) -> bool {
        self.mode.is_some()
    }

    /// Seconds until the timer fires, for the UI countdown. `None` for end-of-track (the UI
    /// shows the track's own remaining time) or when off.
    pub fn remaining(&self, now: f64) -> Option<f64> {
        match self.mode? {
            SleepMode::At { ends_at } => Some((ends_at - now).max(0.0)),
            SleepMode::EndOfTrack => None,
        }
    }

    /// One step. `track_left` is the playing track's remaining *listening* time in seconds
    /// (already divided by the playback speed), `None` when nothing is loaded or it is unknown
    /// (a live stream).
    ///
    /// End-of-track never returns `Stop` itself: the player holds the file at its end
    /// (`keep-open`) and pauses, which is sample-exact, while this only fades.
    pub fn tick(&self, now: f64, track_left: Option<f64>) -> Action {
        match self.mode {
            None => Action::Wait,
            Some(SleepMode::At { ends_at }) => {
                let left = ends_at - now;
                if left <= 0.0 {
                    Action::Stop
                } else {
                    fade(left)
                }
            }
            Some(SleepMode::EndOfTrack) => track_left.map_or(Action::Wait, fade),
        }
    }
}

fn fade(left: f64) -> Action {
    if left >= FADE_SECS {
        Action::Wait
    } else {
        Action::Fade(fade_gain_db(left))
    }
}

/// Extra gain with `left` seconds of the fade to go: 0 dB at the start, the floor at the end.
/// Linear in amplitude (equal steps of loudness feel abrupt at the bottom); in dB that is a
/// gentle start and a quick last second, which is how a hand on a volume knob sounds.
pub fn fade_gain_db(left: f64) -> f64 {
    let x = (left / FADE_SECS).clamp(0.0, 1.0);
    if x <= 0.0 {
        return FADE_FLOOR_DB;
    }
    (20.0 * x.log10()).max(FADE_FLOOR_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_mode_counts_down_fades_and_stops() {
        let mut t = SleepTimer::default();
        assert_eq!(t.tick(0.0, Some(100.0)), Action::Wait);
        assert_eq!(t.remaining(0.0), None);
        t.set_minutes(1000.0, 5.0);
        assert!(t.armed());
        assert_eq!(t.remaining(1000.0), Some(300.0));
        assert_eq!(t.remaining(2000.0), Some(0.0));
        assert_eq!(t.tick(1000.0, Some(5.0)), Action::Wait, "track ends don't matter here");
        assert_eq!(t.tick(1289.0, None), Action::Wait);
        let Action::Fade(start) = t.tick(1290.5, None) else { panic!() };
        let Action::Fade(late) = t.tick(1299.5, None) else { panic!() };
        assert!(start > -1.0 && start <= 0.0, "{start}");
        assert!(late < start && late >= FADE_FLOOR_DB, "{late}");
        assert_eq!(t.tick(1300.0, None), Action::Stop);
        assert_eq!(t.tick(5000.0, None), Action::Stop);
        t.cancel();
        assert_eq!(t.tick(1300.0, None), Action::Wait);
        assert!(!t.armed());
    }

    #[test]
    fn end_of_track_fades_on_the_track_clock() {
        let mut t = SleepTimer::default();
        t.set_end_of_track();
        assert_eq!(t.remaining(0.0), None);
        assert_eq!(t.tick(0.0, None), Action::Wait, "nothing loaded");
        assert_eq!(t.tick(0.0, Some(60.0)), Action::Wait);
        assert!(matches!(t.tick(0.0, Some(5.0)), Action::Fade(g) if g < -5.0 && g > -7.0));
        assert!(matches!(t.tick(0.0, Some(0.0)), Action::Fade(g) if g == FADE_FLOOR_DB));
    }

    #[test]
    fn fade_curve_is_monotonic() {
        assert_eq!(fade_gain_db(FADE_SECS), 0.0);
        assert_eq!(fade_gain_db(20.0), 0.0);
        assert_eq!(fade_gain_db(-1.0), FADE_FLOOR_DB);
        let mut last = 1.0;
        for i in (0..=100).rev() {
            let g = fade_gain_db(i as f64 / 10.0);
            assert!(g <= last);
            last = g;
        }
        assert!((fade_gain_db(5.0) + 6.02).abs() < 0.01, "half amplitude at the midpoint");
    }

    #[test]
    fn serde_shape_for_the_ui() {
        let j = serde_json::to_string(&SleepMode::At { ends_at: 12.5 }).unwrap();
        assert_eq!(j, r#"{"kind":"at","endsAt":12.5}"#);
        let j = serde_json::to_string(&SleepMode::EndOfTrack).unwrap();
        assert_eq!(j, r#"{"kind":"endOfTrack"}"#);
    }
}
