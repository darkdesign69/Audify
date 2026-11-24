//! Played / resume rules, lifted from AntennaPod (`smartMarkAsPlayedSecs`, default 30 s) and
//! Pocket Casts (an episode counts as finished in its last stretch, and resuming rewinds a few
//! seconds so the listener catches the sentence they were in). Pure functions, unit-tested.

/// Within this many seconds of the end, an episode counts as played (AntennaPod's default).
pub const PLAYED_TAIL_SECS: f64 = 30.0;
/// Past this fraction, an episode counts as played (catches long outros and end-of-show ads).
pub const PLAYED_FRACTION: f64 = 0.95;
/// Below this position an episode isn't "started": no resume, not in Continue listening.
pub const MIN_STARTED_SECS: f64 = 10.0;
/// How far back a resume starts (Pocket Casts / AntennaPod rewind on resume).
pub const RESUME_REWIND_SECS: f64 = 5.0;
/// How often a playing episode's position is written (seconds of playback).
pub const SAVE_EVERY_SECS: f64 = 5.0;

/// Whether listening up to `pos` of `dur` finishes the episode.
///
/// For episodes under a minute the 30 s tail would mark them played on the first tick, so only
/// the fraction applies there.
pub fn is_played(pos: f64, dur: f64) -> bool {
    if !(dur > 0.0) || !(pos > 0.0) {
        return false;
    }
    let frac = pos / dur >= PLAYED_FRACTION;
    if dur <= 2.0 * PLAYED_TAIL_SECS {
        return frac;
    }
    frac || dur - pos <= PLAYED_TAIL_SECS
}

/// Where to start an episode saved at `pos` (of `dur`, 0 when unknown). `None` = from the top:
/// never started, or already played (a replay starts over, like Spotify).
pub fn resume_from(pos: f64, dur: f64, played: bool) -> Option<f64> {
    if played || !(pos >= MIN_STARTED_SECS) {
        return None;
    }
    if dur > 0.0 && is_played(pos, dur) {
        return None;
    }
    Some((pos - RESUME_REWIND_SECS).max(0.0))
}

/// The row's progress bar: 0..=1, 1 once played.
pub fn fraction(pos: f64, dur: f64, played: bool) -> f64 {
    if played {
        return 1.0;
    }
    if !(dur > 0.0) || !(pos > 0.0) {
        return 0.0;
    }
    (pos / dur).clamp(0.0, 1.0)
}

/// Started and not finished: what "Continue listening" lists (the SQL in `store` mirrors it).
#[allow(dead_code)]
pub fn in_progress(pos: f64, played: bool) -> bool {
    !played && pos >= MIN_STARTED_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn played_in_the_last_30_seconds_or_past_95_percent() {
        // One hour.
        assert!(!is_played(0.0, 3600.0));
        assert!(!is_played(1800.0, 3600.0));
        // An hour: 95 % (3 min left) comes before the 30 s tail.
        assert!(is_played(3420.0, 3600.0));
        assert!(!is_played(3419.0, 3600.0));
        // Ten minutes: the tail rule kicks in before 95 %.
        assert!(is_played(570.0, 600.0));
        assert!(!is_played(560.0, 600.0));
        // Short clip: only the fraction.
        assert!(!is_played(5.0, 40.0));
        assert!(is_played(38.0, 40.0));
        // Unknown duration never auto-plays.
        assert!(!is_played(500.0, 0.0));
    }

    #[test]
    fn resume_rewinds_and_skips_unstarted_or_finished() {
        assert_eq!(resume_from(0.0, 3600.0, false), None);
        assert_eq!(resume_from(9.0, 3600.0, false), None);
        assert_eq!(resume_from(100.0, 3600.0, false), Some(95.0));
        assert_eq!(resume_from(12.0, 0.0, false), Some(7.0));
        assert_eq!(resume_from(100.0, 3600.0, true), None);
        assert_eq!(resume_from(3590.0, 3600.0, false), None);
    }

    #[test]
    fn fraction_for_progress_bars() {
        assert_eq!(fraction(0.0, 100.0, false), 0.0);
        assert_eq!(fraction(25.0, 100.0, false), 0.25);
        assert_eq!(fraction(25.0, 0.0, false), 0.0);
        assert_eq!(fraction(0.0, 0.0, true), 1.0);
        assert_eq!(fraction(200.0, 100.0, false), 1.0);
        assert!(in_progress(60.0, false));
        assert!(!in_progress(60.0, true));
        assert!(!in_progress(3.0, false));
    }
}
