//! Encore audio stream: the pure half of the sound chain. Equalizer maths (RBJ biquads, the same
//! formulas FFmpeg's `lowshelf`/`equalizer`/`highshelf` use), the preset table, the mpv `af`
//! string, and Spotify's loudness levels. No libmpv in here, so every rule is unit-testable in
//! microseconds; `lib.rs` owns the mpv side.
//!
//! Design notes: docs/research/21-gap-equalizer.md.

use serde::{Deserialize, Serialize};

/// Number of EQ bands. Spotify's desktop EQ: 60, 150, 400, 1k, 2.4k and 15k Hz.
pub const BANDS: usize = 6;

/// The most a band moves either way, in dB (Spotify's range).
pub const MAX_BAND_DB: f64 = 12.0;

/// The sample rate the preamp and the UI's curve are computed at. The real stream is 44.1 or
/// 48 kHz; the difference is invisible below 15 kHz and the curve's peak is what matters.
pub const CURVE_RATE: f64 = 48_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BandKind {
    LowShelf,
    Peaking,
    HighShelf,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub kind: BandKind,
    pub freq: f64,
    pub q: f64,
}

/// Shelves at the ends (Q 0.7, the Butterworth-ish shelf), peaking filters in between (Q 1).
pub const EQ_BANDS: [Band; BANDS] = [
    Band { kind: BandKind::LowShelf, freq: 60.0, q: 0.7 },
    Band { kind: BandKind::Peaking, freq: 150.0, q: 1.0 },
    Band { kind: BandKind::Peaking, freq: 400.0, q: 1.0 },
    Band { kind: BandKind::Peaking, freq: 1000.0, q: 1.0 },
    Band { kind: BandKind::Peaking, freq: 2400.0, q: 1.0 },
    Band { kind: BandKind::HighShelf, freq: 15000.0, q: 0.7 },
];

/// Normalised biquad coefficients (a0 = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// RBJ Audio EQ Cookbook coefficients, Q form. FFmpeg's af_biquads.c computes exactly these for
/// `t=q`, so the curve the UI draws is the curve the user hears.
pub fn biquad(kind: BandKind, freq: f64, q: f64, gain_db: f64, rate: f64) -> Biquad {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = std::f64::consts::TAU * freq / rate;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        BandKind::Peaking => (
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        ),
        BandKind::LowShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) - (a - 1.0) * cos + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - k),
                (a + 1.0) + (a - 1.0) * cos + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - k,
            )
        }
        BandKind::HighShelf => {
            let k = 2.0 * a.sqrt() * alpha;
            (
                a * ((a + 1.0) + (a - 1.0) * cos + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - k),
                (a + 1.0) - (a - 1.0) * cos + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - k,
            )
        }
    };
    Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

impl Biquad {
    /// |H(e^jw)| in dB at `freq` (the DSSSP / cookbook magnitude formula).
    pub fn magnitude_db(&self, freq: f64, rate: f64) -> f64 {
        let w = std::f64::consts::TAU * freq / rate;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

/// The six bands' summed response in dB at `freq` (cascaded biquads multiply, so dB adds).
pub fn response_db(gains: &[f64; BANDS], freq: f64, rate: f64) -> f64 {
    EQ_BANDS
        .iter()
        .zip(gains)
        .filter(|(_, g)| **g != 0.0)
        .map(|(b, g)| biquad(b.kind, b.freq, b.q, *g, rate).magnitude_db(freq, rate))
        .sum()
}

/// Highest point of the composite curve between 20 Hz and 20 kHz (or Nyquist), 240 log steps.
pub fn peak_db(gains: &[f64; BANDS]) -> f64 {
    let (lo, hi) = (20f64.ln(), 20_000f64.min(CURVE_RATE / 2.0 - 1.0).ln());
    (0..=240)
        .map(|i| (lo + (hi - lo) * i as f64 / 240.0).exp())
        .map(|f| response_db(gains, f, CURVE_RATE))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Pre-gain that keeps the boosted curve's peak at 0 dB, so a +12 dB band doesn't clip. Never
/// positive; capped at -12 dB (beyond that the limiter is the backstop).
pub fn auto_preamp(gains: &[f64; BANDS]) -> f64 {
    let p = -peak_db(gains).max(0.0);
    // Two decimals: a value the af string and the UI agree on, and no "-0" noise.
    (p.max(-MAX_BAND_DB) * 100.0).round() / 100.0 + 0.0
}

/// The user's equalizer, as persisted and as the player applies it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EqSettings {
    pub enabled: bool,
    pub gains: [f64; BANDS],
}

impl Default for EqSettings {
    fn default() -> Self {
        Self { enabled: false, gains: [0.0; BANDS] }
    }
}

impl EqSettings {
    /// Clamped to ±12 dB and rounded to 0.1 dB; NaN becomes 0.
    pub fn sanitized(mut self) -> Self {
        for g in &mut self.gains {
            *g = if g.is_finite() {
                ((g.clamp(-MAX_BAND_DB, MAX_BAND_DB)) * 10.0).round() / 10.0 + 0.0
            } else {
                0.0
            };
        }
        self
    }

    /// Enabled and not flat: the only case that needs filters at all.
    pub fn audible(&self) -> bool {
        self.enabled && self.gains.iter().any(|g| *g != 0.0)
    }
}

/// Presets, in Spotify's order (Spotify's list is Apple's iTunes/Music EQ list, same 22 names).
/// Values in dB for 60/150/400/1k/2.4k/15k.
///
/// Source: Apple's own preset values (10-band, 32 Hz..16 kHz), read from the Music app's preset
/// store (`~/Library/Preferences/com.apple.Music.eq.plist`, `EQPresets`: big-endian i16 centi-dB
/// per band) and cross-checked against eqMac's `AdvancedEqualizerDefaultPresets.swift`, which
/// ships the same curves doubled (20 of 21 identical). Apple's ten slider points, joined linearly
/// in log-frequency, are the target curve; each preset here is the six RBJ gains (our band
/// shapes, [`EQ_BANDS`]) whose composite response is closest to it, least squares over 241 log
/// points from 20 Hz to 20 kHz, then polished on the 0.1 dB grid. The comment is the remaining
/// RMS error, and the error of the previous table (Feishin/Strawberry resamples and the seven
/// presets authored by ear). Method and numbers: docs/bench/eq-presets.md.
pub const PRESETS: [(&str, [f64; BANDS]); 22] = [
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Acoustic", [5.8, 3.5, 0.8, 0.7, 5.0, 3.5]), // 1.34 dB rms (was 2.34)
    ("Bass Booster", [6.0, 3.6, 1.2, -0.2, 0.0, 0.0]), // 0.35 dB rms (was 0.51)
    ("Bass Reducer", [-6.0, -3.6, -1.2, 0.2, 0.0, 0.0]), // 0.35 dB rms (was 0.51)
    ("Classical", [5.1, 3.7, -0.3, -2.5, 2.2, 5.8]), // 1.22 dB rms (was 3.13)
    ("Dance", [5.1, 4.7, -0.8, 2.7, 6.0, 0.5]),   // 1.60 dB rms (was 4.49)
    ("Deep", [5.5, 1.4, 1.4, 3.1, -1.3, -7.2]),   // 1.36 dB rms (was 1.91)
    ("Electronic", [4.9, 1.8, -2.5, 1.7, 1.5, 7.5]), // 1.34 dB rms (was 1.81)
    ("Hip-Hop", [5.6, 2.7, 0.3, -1.4, 1.4, 4.5]), // 0.87 dB rms (was 1.18)
    ("Jazz", [4.3, 2.4, -0.1, -2.3, 1.7, 5.8]),   // 1.10 dB rms (was 2.37)
    ("Latin", [4.9, 0.4, -0.8, -1.7, -0.3, 6.9]), // 1.05 dB rms (was 1.59)
    ("Loudness", [6.6, 0.6, -1.8, 0.8, -2.9, 2.3]), // 1.48 dB rms (was 2.14)
    ("Lounge", [-3.1, -0.7, 3.3, 2.4, -1.2, 1.7]), // 0.60 dB rms (was 1.33)
    ("Piano", [3.2, 0.3, 3.2, 0.0, 5.2, 5.3]),    // 1.17 dB rms (was 1.83)
    ("Pop", [-1.7, -0.1, 3.1, 3.6, 0.4, -2.3]),   // 0.44 dB rms (was 1.42)
    ("R&B", [4.2, 6.6, -2.5, -2.1, 4.0, 5.7]),    // 1.55 dB rms (was 2.10)
    ("Rock", [5.5, 3.2, -0.1, -1.8, 2.5, 7.0]),   // 1.29 dB rms (was 2.24)
    ("Small Speakers", [6.0, 3.6, 1.1, 0.5, -2.8, -6.6]), // 1.25 dB rms (was 1.50)
    ("Spoken Word", [-3.2, -0.1, 1.9, 3.4, 5.3, 0.4]), // 0.90 dB rms (was 1.10)
    ("Treble Booster", [0.0, 0.0, -0.1, 0.2, 4.2, 8.6]), // 1.46 dB rms (was 1.77)
    ("Treble Reducer", [0.0, 0.0, 0.1, -0.2, -4.2, -8.6]), // 1.46 dB rms (was 1.77)
    ("Vocal Booster", [-2.2, -3.1, 3.6, 2.9, 2.2, -2.1]), // 0.63 dB rms (was 1.93)
];

pub fn preset(name: &str) -> Option<[f64; BANDS]> {
    PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, g)| *g)
}

/// The name of the preset these gains are, if any ("Custom" otherwise, which is the UI's call).
pub fn preset_name(gains: &[f64; BANDS]) -> Option<&'static str> {
    PRESETS
        .iter()
        .find(|(_, g)| g.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.05))
        .map(|(n, _)| *n)
}

// ── The mpv filter chain ────────────────────────────────────────────────────────────────────────

/// The mpv label of the Encore stage. One lavfi graph, so its internal filters are addressable by
/// `af-command enc <cmd> <arg> <instance>` without rebuilding anything.
pub const LABEL: &str = "enc";

/// Peak protection. `limit=1` is full scale so only genuine overs are touched; `level=disabled`
/// keeps alimiter from re-normalising; `latency=1` trims its 5 ms lookahead so gapless boundaries
/// stay sample-exact (see `af_chain`'s history in lib.rs).
const LIMITER: &str = "alimiter@lim=limit=1:level=disabled:latency=1";

/// Which FFmpeg filters the Encore stage is built from. Detected at runtime by the player (a
/// probe of the linked FFmpeg), so one binary does the right thing on either build.
///
/// stream: ios-eq. MPVKit's FFmpeg (the iOS build) is configured with `--disable-filters` and a
/// short whitelist: of the audio filters only aformat, amix, anull, aresample, areverse,
/// asetrate, atempo, atrim, equalizer, firequalizer, pan, superequalizer and volume exist. No
/// shelves, no dynamics at all (alimiter, acompressor, dynaudnorm, asoftclip, loudnorm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flavor {
    /// RBJ `lowshelf`/`equalizer`/`highshelf` plus the `alimiter` backstop (desktop FFmpeg).
    #[default]
    Full,
    /// `equalizer` and `volume` only. Each shelf is a pair of peaking filters fitted to it
    /// ([`LOW_SHELF_FIT`], [`HIGH_SHELF_FIT`]), run at [`CURVE_RATE`] so the fit holds for any
    /// source; peak safety is gain headroom ([`peak_gain_db`]) instead of a limiter.
    Portable,
}

/// One peaking filter standing in for part of a shelf: centre, Q, and its gain as a function
/// of the shelf's gain `g`: `k1 * g + k2 * g * |g| / 12`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakFit {
    pub freq: f64,
    pub q: f64,
    pub k1: f64,
    pub k2: f64,
}

impl PeakFit {
    pub fn gain(&self, shelf_db: f64) -> f64 {
        let g = self.k1 * shelf_db + self.k2 * shelf_db * shelf_db.abs() / MAX_BAND_DB;
        // The value the af string carries (`num`), so the maths is what FFmpeg runs.
        (g * 1000.0).round() / 1000.0 + 0.0
    }
}

/// stream: ios-eq. The 60 Hz, Q 0.7 low shelf as two peaking filters. Least squares (Nelder-Mead)
/// of the four parameters each against the RBJ shelf over 241 log points 20 Hz-20 kHz at shelf
/// gains of ±1..12 dB, 48 kHz. Worst single-gain error 0.16 dB RMS, 0.41 dB max (at ±12 dB).
/// Method and numbers: docs/streams/ios-eq.md.
pub const LOW_SHELF_FIT: [PeakFit; 2] = [
    PeakFit { freq: 21.6, q: 0.816, k1: 0.855, k2: 0.0131 },
    PeakFit { freq: 43.52, q: 1.092, k1: 0.4637, k2: -0.039 },
];

/// stream: ios-eq. The 15 kHz, Q 0.7 high shelf, likewise (48 kHz). Worst 0.11 dB RMS, 0.42 dB max.
pub const HIGH_SHELF_FIT: [PeakFit; 2] = [
    PeakFit { freq: 16608.8, q: 1.237, k1: 0.4414, k2: -0.0288 },
    PeakFit { freq: 19774.0, q: 1.051, k1: 0.8626, k2: 0.0002 },
];

/// The peaking filters a [`Flavor::Portable`] band is made of: (instance name, freq, q, gain).
fn portable_band(i: usize, g: f64) -> Vec<(String, f64, f64, f64)> {
    let band = EQ_BANDS[i];
    let fit = match band.kind {
        BandKind::Peaking => return vec![(format!("equalizer@b{i}"), band.freq, band.q, g)],
        BandKind::LowShelf => &LOW_SHELF_FIT,
        BandKind::HighShelf => &HIGH_SHELF_FIT,
    };
    fit.iter()
        .zip(["a", "b"])
        .map(|(p, s)| (format!("equalizer@b{i}{s}"), p.freq, p.q, p.gain(g)))
        .collect()
}

/// The biquads `flavor`'s EQ runs for these gains (zero-gain filters are the identity, left out).
pub fn biquads(gains: &[f64; BANDS], flavor: Flavor, rate: f64) -> Vec<Biquad> {
    match flavor {
        Flavor::Full => EQ_BANDS
            .iter()
            .zip(gains)
            .filter(|(_, g)| **g != 0.0)
            .map(|(b, g)| biquad(b.kind, b.freq, b.q, *g, rate))
            .collect(),
        Flavor::Portable => (0..BANDS)
            .flat_map(|i| portable_band(i, gains[i]))
            .filter(|(_, _, _, g)| *g != 0.0)
            .map(|(_, f, q, g)| biquad(BandKind::Peaking, f, q, g, rate))
            .collect(),
    }
}

/// `flavor`'s summed EQ response in dB at `freq`.
pub fn flavor_response_db(gains: &[f64; BANDS], flavor: Flavor, freq: f64, rate: f64) -> f64 {
    biquads(gains, flavor, rate).iter().map(|b| b.magnitude_db(freq, rate)).sum()
}

/// The most the EQ can lift *any* signal's peak, in dB: the L1 norm of its impulse response
/// (|y| <= sum|h| * max|x|, and a signal exists that reaches it). Above the curve's peak
/// ([`peak_db`]) by the phase smear. With no limiter, a gain of `-peak_gain_db` in front makes
/// clipping impossible rather than unlikely. 0 for a flat EQ.
pub fn peak_gain_db(gains: &[f64; BANDS], flavor: Flavor) -> f64 {
    let filters = biquads(gains, flavor, CURVE_RATE);
    if filters.is_empty() {
        return 0.0;
    }
    // 0.5 s: the slowest pole (21.6 Hz, Q 0.8, +12 dB) has decayed past -200 dB by then.
    let mut x: Vec<f64> = vec![0.0; 24_000];
    x[0] = 1.0;
    for f in &filters {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        for v in x.iter_mut() {
            let y = f.b0 * *v + f.b1 * x1 + f.b2 * x2 - f.a1 * y1 - f.a2 * y2;
            (x2, x1, y2, y1) = (x1, *v, y1, y);
            *v = y;
        }
    }
    20.0 * x.iter().map(|v| v.abs()).sum::<f64>().log10()
}

/// Everything Encore puts in front of the effects. `None` stages are absent from the graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    /// Volume-levelling gain in dB.
    pub level: Option<f64>,
    /// EQ preamp and band gains.
    pub eq: Option<(f64, [f64; BANDS])>,
    pub limiter: bool,
    /// Which filters the graph is made of.
    pub flavor: Flavor,
}

impl Stage {
    /// What the player wants for a track gain and an EQ setting (the desktop chain).
    ///
    /// The limiter rides along whenever anything can lift a peak: a positive levelling gain (as
    /// Limusic always did) or an active EQ (its auto preamp keeps the curve at 0 dB, but a boosted
    /// band can still push a dense mix's inter-band sum over).
    pub fn new(level: Option<f64>, eq: &EqSettings) -> Self {
        Self::with_flavor(level, eq, Flavor::Full)
    }

    /// [`Self::new`] for a given FFmpeg. [`Flavor::Portable`] has no limiter, so it never lifts
    /// a peak instead: see [`Self::settled`].
    pub fn with_flavor(level: Option<f64>, eq: &EqSettings, flavor: Flavor) -> Self {
        let eq = eq.audible().then(|| (auto_preamp(&eq.gains), eq.gains));
        let limiter = level.is_some_and(|g| g > 0.0) || eq.is_some();
        Stage { level, eq, limiter, flavor }.settled()
    }

    /// stream: ios-eq. A [`Flavor::Portable`] stage's peak safety, with no limiter to lean on:
    /// the whole stage's worst-case peak gain (levelling + preamp + the EQ's [`peak_gain_db`])
    /// is held at or under 0 dB, so a signal at full scale comes out at most at full scale.
    /// - EQ on: the preamp is the desktop's ([`auto_preamp`]) or `-(level + peak_gain_db)`,
    ///   whichever is lower. A track levelled down by more than the bound (most music: modern
    ///   masters sit well above -14 LUFS) plays exactly as loud as on the desktop.
    /// - EQ off: a positive levelling gain is dropped to 0 dB. Quiet tracks then stay as quiet
    ///   as mastered under Normal/Loud, which is Spotify's own rule when it can't limit.
    fn settled(mut self) -> Self {
        if self.flavor != Flavor::Portable {
            return self;
        }
        self.limiter = false;
        let level = self.level.unwrap_or(0.0);
        match &mut self.eq {
            Some((pre, gains)) => {
                let need = -(level + peak_gain_db(gains, Flavor::Portable));
                // Never louder than the desktop (its auto preamp), never above the bound.
                // Rounded down to the af string's precision: safe side.
                let pre_db = need.min(auto_preamp(gains)).min(0.0);
                *pre = ((pre_db * 1000.0).floor() / 1000.0) + 0.0;
            }
            None => self.level = self.level.map(|g| g.min(0.0)),
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.level.is_none() && self.eq.is_none() && !self.limiter
    }

    /// Same filters in the same order, so only parameters differ and `af-command` can retune it.
    pub fn same_shape(&self, other: &Stage) -> bool {
        self.level.is_some() == other.level.is_some()
            && self.eq.is_some() == other.eq.is_some()
            && self.limiter == other.limiter
            && self.flavor == other.flavor
    }

    /// `self` as it would have to look to keep `applied`'s shape: a stage `applied` has and
    /// `self` doesn't stays in, neutral (0 dB, flat EQ). Turning the EQ off mid-song then zeroes
    /// it live instead of rebuilding the graph (a rebuild is an audible tick). The next track
    /// change builds the real, smaller graph.
    pub fn kept_shape_of(&self, applied: &Stage) -> Stage {
        Stage {
            level: self.level.or(applied.level.map(|_| 0.0)),
            eq: self.eq.or(applied.eq.map(|_| (0.0, [0.0; BANDS]))),
            limiter: self.limiter || applied.limiter,
            flavor: self.flavor,
        }
        .settled()
    }

    /// The lavfi graph body (no label), e.g. `volume@lvl=volume=-3dB,alimiter@lim=...`.
    pub fn graph(&self) -> String {
        let mut parts = Vec::new();
        if let Some(g) = self.level {
            parts.push(format!("volume@lvl=volume={}dB", num(g)));
        }
        if let Some((pre, gains)) = &self.eq {
            if self.flavor == Flavor::Portable {
                // The shelf fits are 48 kHz fits. On iOS the output runs at 48 kHz anyway, so
                // this moves mpv's own resample into the graph rather than adding one.
                parts.push(format!("aresample={}", CURVE_RATE as u32));
            }
            parts.push(format!("volume@pre=volume={}dB", num(*pre)));
            match self.flavor {
                Flavor::Full => {
                    for (i, (band, g)) in EQ_BANDS.iter().zip(gains).enumerate() {
                        parts.push(format!(
                            "{}@b{i}=f={}:t=q:w={}:g={}",
                            full_filter(band.kind),
                            num(band.freq),
                            num(band.q),
                            num(*g)
                        ));
                    }
                }
                Flavor::Portable => {
                    for (i, g) in gains.iter().enumerate() {
                        for (name, f, q, g) in portable_band(i, *g) {
                            parts.push(format!(
                                "{name}=f={}:t=q:w={}:g={}",
                                num(f),
                                num(q),
                                num(g)
                            ));
                        }
                    }
                }
            }
        }
        if self.limiter {
            parts.push(LIMITER.to_owned());
        }
        parts.join(",")
    }

    /// The `af` entry, or "" when there is nothing to do. Length-quoted (`%n%`) so the graph's
    /// commas survive mpv's filter-list parser and a loadfile per-file option list alike.
    pub fn filter(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let graph = self.graph();
        format!("@{LABEL}:lavfi=graph=%{}%{graph}", graph.len())
    }

    /// `af-command` argument lists that turn `self` into `target` in place. Only valid when
    /// [`Self::same_shape`]; each entry is `[label, command, argument, instance]`.
    pub fn commands_to(&self, target: &Stage) -> Vec<[String; 4]> {
        let mut out = Vec::new();
        let cmd = |c: &str, a: String, t: String| [LABEL.to_owned(), c.to_owned(), a, t];
        if let (Some(a), Some(b)) = (self.level, target.level) {
            if a != b {
                out.push(cmd("volume", format!("{}dB", num(b)), "volume@lvl".into()));
            }
        }
        if let (Some((pa, ga)), Some((pb, gb))) = (&self.eq, &target.eq) {
            if pa != pb {
                out.push(cmd("volume", format!("{}dB", num(*pb)), "volume@pre".into()));
            }
            for i in 0..BANDS {
                if ga[i] == gb[i] {
                    continue;
                }
                match self.flavor {
                    Flavor::Full => out.push(cmd(
                        "g",
                        num(gb[i]),
                        format!("{}@b{i}", full_filter(EQ_BANDS[i].kind)),
                    )),
                    Flavor::Portable => {
                        for (name, _, _, g) in portable_band(i, gb[i]) {
                            out.push(cmd("g", num(g), name));
                        }
                    }
                }
            }
        }
        out
    }
}

fn full_filter(kind: BandKind) -> &'static str {
    match kind {
        BandKind::LowShelf => "lowshelf",
        BandKind::Peaking => "equalizer",
        BandKind::HighShelf => "highshelf",
    }
}

/// A full `af` value: the Encore stage, then whatever effects tail follows (reverb, stretch).
pub fn join_chain(stage: &Stage, tail: &str) -> String {
    let head = stage.filter();
    match (head.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_owned(),
        (false, true) => head,
        (false, false) => format!("{head},{tail}"),
    }
}

/// A float as FFmpeg reads it: no exponent, no trailing zeros, no "-0".
fn num(v: f64) -> String {
    let v = (v * 1000.0).round() / 1000.0 + 0.0;
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

// ── Volume levelling (Spotify's Quiet / Normal / Loud) ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Quiet,
    #[default]
    Normal,
    Loud,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "quiet" => Some(Level::Quiet),
            "normal" => Some(Level::Normal),
            "loud" => Some(Level::Loud),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Level::Quiet => "quiet",
            Level::Normal => "normal",
            Level::Loud => "loud",
        }
    }

    /// Integrated loudness target, LUFS (Spotify's published values).
    pub fn target_lufs(self) -> f64 {
        match self {
            Level::Quiet => -23.0,
            Level::Normal => -14.0,
            Level::Loud => -11.0,
        }
    }

    /// The most a quiet track is lifted. YouTube sends no peak data, so a boost is limiter work:
    /// Quiet and Normal stay gentle (Spotify only lifts there when the peaks allow it), Loud is
    /// the mode where Spotify itself leans on a limiter.
    pub fn max_boost_db(self) -> f64 {
        match self {
            Level::Quiet => 0.0,
            Level::Normal => 3.0,
            Level::Loud => 6.0,
        }
    }
}

/// Gain in dB that brings a track to `level`'s target, from YouTube's per-format `loudnessDb`.
///
/// `loudnessDb` is the track's loudness relative to -14 LUFS (YouTube's own response has
/// `perceptualLoudnessDb == loudnessDb - 14`), so the track sits at `loudnessDb - 14` LUFS.
/// `None` (no metadata, or already within 0.05 dB) means "no gain filter".
pub fn loudness_gain(loudness_db: Option<f64>, level: Level) -> Option<f64> {
    let lufs = loudness_db.filter(|l| l.is_finite())? - 14.0;
    let gain = (level.target_lufs() - lufs).clamp(-30.0, level.max_boost_db());
    (gain.abs() >= 0.05).then_some((gain * 100.0).round() / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biquad_matches_the_cookbook() {
        // Zero gain is the identity for every shape.
        for b in EQ_BANDS {
            let q = biquad(b.kind, b.freq, b.q, 0.0, 48_000.0);
            for f in [30.0, 500.0, 5000.0, 19_000.0] {
                assert!(q.magnitude_db(f, 48_000.0).abs() < 1e-9);
            }
        }
        // A peaking filter hits its gain exactly at its centre frequency.
        let p = biquad(BandKind::Peaking, 1000.0, 1.0, 6.0, 48_000.0);
        assert!((p.magnitude_db(1000.0, 48_000.0) - 6.0).abs() < 1e-9);
        assert!(p.magnitude_db(50.0, 48_000.0).abs() < 0.1);
        // Shelves: full gain far inside the shelf, half (in dB) at the corner, ~0 far outside.
        let lo = biquad(BandKind::LowShelf, 60.0, 0.7, -9.0, 48_000.0);
        assert!((lo.magnitude_db(10.0, 48_000.0) + 9.0).abs() < 0.2);
        assert!((lo.magnitude_db(60.0, 48_000.0) + 4.5).abs() < 0.05);
        assert!(lo.magnitude_db(5000.0, 48_000.0).abs() < 0.05);
        let hi = biquad(BandKind::HighShelf, 15_000.0, 0.7, 12.0, 48_000.0);
        assert!((hi.magnitude_db(15_000.0, 48_000.0) - 6.0).abs() < 0.05);
        assert!(hi.magnitude_db(200.0, 48_000.0).abs() < 0.05);
        // A known coefficient set (peaking, 1 kHz, Q 1, +6 dB, 48 kHz), from the cookbook by hand.
        let a = 10f64.powf(6.0 / 40.0);
        let w = std::f64::consts::TAU * 1000.0 / 48_000.0;
        let alpha = w.sin() / 2.0;
        let a0 = 1.0 + alpha / a;
        assert!((p.b0 - (1.0 + alpha * a) / a0).abs() < 1e-12);
        assert!((p.a2 - (1.0 - alpha / a) / a0).abs() < 1e-12);
    }

    #[test]
    fn preamp_keeps_the_peak_at_zero() {
        assert_eq!(auto_preamp(&[0.0; BANDS]), 0.0);
        assert_eq!(auto_preamp(&[-6.0; BANDS]), 0.0, "cuts never need a preamp");
        let one = auto_preamp(&[0.0, 0.0, 0.0, 6.0, 0.0, 0.0]);
        assert!((one + 6.0).abs() < 0.05, "{one}");
        // Overlapping boosts sum past either band; the preamp follows the sum, capped at -12.
        let all = [12.0; BANDS];
        assert!(peak_db(&all) > 12.0);
        assert_eq!(auto_preamp(&all), -12.0);
        let g = [3.0, 4.0, 0.0, 0.0, 0.0, 0.0];
        assert!(auto_preamp(&g) < -4.0, "{}", auto_preamp(&g));
    }

    #[test]
    fn preset_table() {
        assert_eq!(PRESETS.len(), 22);
        let names: Vec<_> = PRESETS.iter().map(|p| p.0).collect();
        for want in [
            "Flat",
            "Acoustic",
            "Bass Booster",
            "Bass Reducer",
            "Classical",
            "Dance",
            "Deep",
            "Electronic",
            "Hip-Hop",
            "Jazz",
            "Latin",
            "Loudness",
            "Lounge",
            "Piano",
            "Pop",
            "R&B",
            "Rock",
            "Small Speakers",
            "Spoken Word",
            "Treble Booster",
            "Treble Reducer",
            "Vocal Booster",
        ] {
            assert!(names.contains(&want), "missing {want}");
        }
        let mut sorted = names.clone();
        sorted[1..].sort();
        assert_eq!(names, sorted, "Flat first, the rest alphabetical like Spotify");
        for (name, gains) in PRESETS {
            assert!(gains.iter().all(|g| g.abs() <= MAX_BAND_DB), "{name}");
            assert_eq!(EqSettings { enabled: true, gains }.sanitized().gains, gains, "{name}");
        }
        // Every preset but Flat is distinct and recognisable from its gains.
        for (name, gains) in PRESETS {
            assert_eq!(preset_name(&gains), Some(name));
        }
        assert_eq!(preset("bass booster"), Some(PRESETS[2].1));
        assert_eq!(preset_name(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), None);
    }

    /// Apple's own ten-band values (32 Hz..16 kHz) the table is fitted to (see [`PRESETS`]).
    const APPLE_PRESETS: [(&str, [f64; 10]); 21] = [
        ("Acoustic", [5.0, 4.9, 3.95, 1.05, 2.15, 1.75, 3.5, 4.1, 3.55, 2.15]),
        ("Bass Booster", [5.5, 4.25, 3.5, 2.5, 1.25, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ("Bass Reducer", [-5.5, -4.25, -3.5, -2.5, -1.25, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ("Classical", [4.75, 3.75, 3.0, 2.5, -1.5, -1.5, 0.0, 2.25, 3.25, 3.75]),
        ("Dance", [3.57, 6.55, 4.99, 0.0, 1.92, 3.65, 5.15, 4.54, 3.59, 0.0]),
        ("Deep", [4.95, 3.55, 1.75, 1.0, 2.85, 2.5, 1.45, -2.15, -3.55, -4.6]),
        ("Electronic", [4.25, 3.8, 1.2, 0.0, -2.15, 2.25, 0.85, 1.25, 3.95, 4.8]),
        ("Hip-Hop", [5.0, 4.25, 1.5, 3.0, -1.0, -1.0, 1.5, -0.5, 2.0, 3.0]),
        ("Jazz", [4.0, 3.0, 1.5, 2.25, -1.5, -1.5, 0.0, 1.5, 3.0, 3.75]),
        ("Latin", [4.5, 3.0, 0.0, 0.0, -1.5, -1.5, -1.5, 0.0, 3.0, 4.5]),
        ("Loudness", [6.0, 4.0, 0.0, 0.0, -2.0, 0.0, -1.0, -5.0, 5.0, 1.0]),
        ("Lounge", [-3.0, -1.5, -0.5, 1.5, 4.0, 2.5, 0.0, -1.5, 2.0, 1.0]),
        ("Piano", [3.0, 2.0, 0.0, 2.5, 3.0, 1.5, 3.5, 4.5, 3.0, 3.5]),
        ("Pop", [-1.5, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0, -1.5]),
        ("R&B", [2.62, 6.92, 5.65, 1.33, -2.19, -1.5, 2.32, 2.65, 3.0, 3.75]),
        ("Rock", [5.0, 4.0, 3.0, 1.5, -0.5, -1.0, 0.5, 2.5, 3.5, 4.5]),
        ("Small Speakers", [5.5, 4.25, 3.5, 2.5, 1.25, 0.0, -1.25, -2.5, -3.5, -4.25]),
        ("Spoken Word", [-3.46, -0.47, 0.0, 0.69, 3.46, 4.61, 4.84, 4.28, 2.54, 0.0]),
        ("Treble Booster", [0.0, 0.0, 0.0, 0.0, 0.0, 1.25, 2.5, 3.5, 4.25, 5.5]),
        ("Treble Reducer", [0.0, 0.0, 0.0, 0.0, 0.0, -1.25, -2.5, -3.5, -4.25, -5.5]),
        ("Vocal Booster", [-1.5, -3.0, -3.0, 1.5, 3.75, 3.75, 3.0, 1.5, 0.0, -1.5]),
    ];

    /// RMS dB distance between our six-band curve and Apple's slider points joined linearly in
    /// log-frequency, over 241 log points from 20 Hz to 20 kHz.
    fn apple_error(gains: &[f64; BANDS], apple: &[f64; 10]) -> f64 {
        const AF: [f64; 10] = [32., 64., 125., 250., 500., 1e3, 2e3, 4e3, 8e3, 16e3];
        let target = |f: f64| {
            let x = f.ln();
            if x <= AF[0].ln() {
                return apple[0];
            }
            for i in 0..9 {
                let (a, b) = (AF[i].ln(), AF[i + 1].ln());
                if x <= b {
                    return apple[i] + (apple[i + 1] - apple[i]) * (x - a) / (b - a);
                }
            }
            apple[9]
        };
        let (lo, hi) = (20f64.ln(), 20_000f64.ln());
        let sq: f64 = (0..=240)
            .map(|i| (lo + (hi - lo) * i as f64 / 240.0).exp())
            .map(|f| (response_db(gains, f, CURVE_RATE) - target(f)).powi(2))
            .sum();
        (sq / 241.0).sqrt()
    }

    #[test]
    fn presets_follow_apple_curves() {
        for (name, apple) in APPLE_PRESETS {
            let gains = preset(name).unwrap_or_else(|| panic!("{name}"));
            let err = apple_error(&gains, &apple);
            // The best six-band fit of the hardest curve (Dance) is 1.60 dB; the old table was
            // up to 4.49 dB off.
            assert!(err < 1.65, "{name}: {err:.2} dB rms");
        }
        // Mirrors stay mirrors.
        let (b, r) = (preset("Bass Booster").unwrap(), preset("Bass Reducer").unwrap());
        assert!(b.iter().zip(r).all(|(x, y)| (x + y).abs() < 1e-9));
    }

    #[test]
    fn sanitize_clamps_and_rounds() {
        let e = EqSettings { enabled: true, gains: [20.0, -13.0, f64::NAN, 1.234, -0.04, 0.0] }
            .sanitized();
        assert_eq!(e.gains, [12.0, -12.0, 0.0, 1.2, 0.0, 0.0]);
        assert!(e.gains[4].is_sign_positive());
        assert!(!EqSettings { enabled: false, gains: [3.0; BANDS] }.audible());
        assert!(!EqSettings { enabled: true, gains: [0.0; BANDS] }.audible());
    }

    #[test]
    fn filter_strings() {
        let off = Stage::new(None, &EqSettings::default());
        assert!(off.is_empty());
        assert_eq!(off.filter(), "");
        assert_eq!(join_chain(&off, "rubberband=x"), "rubberband=x");

        let cut = Stage::new(Some(-4.5), &EqSettings::default());
        assert_eq!(cut.graph(), "volume@lvl=volume=-4.5dB");
        assert_eq!(cut.filter(), "@enc:lavfi=graph=%24%volume@lvl=volume=-4.5dB");

        let boost = Stage::new(Some(2.0), &EqSettings::default());
        assert_eq!(
            boost.graph(),
            "volume@lvl=volume=2dB,alimiter@lim=limit=1:level=disabled:latency=1"
        );

        let eq = EqSettings { enabled: true, gains: [3.0, 0.0, -2.0, 0.0, 0.0, 1.5] };
        let s = Stage::new(None, &eq);
        let pre = auto_preamp(&eq.gains);
        assert!(pre < 0.0);
        assert_eq!(
            s.graph(),
            format!(
                "volume@pre=volume={}dB,lowshelf@b0=f=60:t=q:w=0.7:g=3,\
                 equalizer@b1=f=150:t=q:w=1:g=0,equalizer@b2=f=400:t=q:w=1:g=-2,\
                 equalizer@b3=f=1000:t=q:w=1:g=0,equalizer@b4=f=2400:t=q:w=1:g=0,\
                 highshelf@b5=f=15000:t=q:w=0.7:g=1.5,\
                 alimiter@lim=limit=1:level=disabled:latency=1",
                num(pre)
            )
        );
        let f = s.filter();
        let n: usize = f.split('%').nth(1).unwrap().parse().unwrap();
        assert_eq!(n, f.rsplit_once('%').unwrap().1.len(), "length quote matches");
        assert_eq!(join_chain(&s, "rubberband=y"), format!("{f},rubberband=y"));
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(1e-7), "0");
        assert_eq!(num(0.70), "0.7");
    }

    #[test]
    fn live_updates_keep_the_shape() {
        let on = EqSettings { enabled: true, gains: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
        let a = Stage::new(Some(-3.0), &on);
        let b = Stage::new(Some(-5.0), &EqSettings { gains: [2.0, 0.0, 0.0, 4.0, 0.0, 0.0], ..on });
        assert!(a.same_shape(&b));
        let cmds = a.commands_to(&b);
        let flat: Vec<String> = cmds.iter().map(|c| c.join(" ")).collect();
        assert!(flat.contains(&"enc volume -5dB volume@lvl".to_owned()), "{flat:?}");
        assert!(flat.contains(&"enc g 4 equalizer@b3".to_owned()), "{flat:?}");
        assert!(flat.iter().any(|c| c.ends_with("volume@pre")), "preamp follows the curve");
        assert_eq!(flat.len(), 3);

        // Switching the EQ off keeps a flat EQ in place rather than rebuilding.
        let off = Stage::new(Some(-3.0), &EqSettings::default());
        assert!(!off.same_shape(&a));
        let kept = off.kept_shape_of(&a);
        assert!(kept.same_shape(&a));
        assert_eq!(kept.eq, Some((0.0, [0.0; BANDS])));
        assert!(kept.limiter);
        // Levelling switched off mid-song: 0 dB, same graph.
        let none = Stage::new(None, &on).kept_shape_of(&a);
        assert_eq!(none.level, Some(0.0));
        // Nothing to keep: unchanged.
        assert_eq!(off.kept_shape_of(&off), off);
        assert!(a.commands_to(&a).is_empty());
    }

    /// RMS dB difference between the Portable and the desktop (Full) EQ curves, over the same
    /// 241 log points 20 Hz-20 kHz as [`peak_db`]. The desktop curve is the reference.
    fn portable_error(gains: &[f64; BANDS], rate: f64) -> (f64, f64) {
        let (lo, hi) = (20f64.ln(), 20_000f64.ln());
        let errs: Vec<f64> = (0..=240)
            .map(|i| (lo + (hi - lo) * i as f64 / 240.0).exp())
            .map(|f| {
                flavor_response_db(gains, Flavor::Portable, f, rate)
                    - flavor_response_db(gains, Flavor::Full, f, rate)
            })
            .collect();
        let rms = (errs.iter().map(|e| e * e).sum::<f64>() / errs.len() as f64).sqrt();
        (rms, errs.iter().fold(0.0f64, |m, e| m.max(e.abs())))
    }

    /// Deterministic random curves: each band uniform on the app's 0.1 dB grid within ±12 dB.
    fn random_curves(n: usize) -> Vec<[f64; BANDS]> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 241) as f64 - 120.0) / 10.0
        };
        let mut out: Vec<[f64; BANDS]> = (0..n).map(|_| std::array::from_fn(|_| next())).collect();
        // The corners: every band at either extreme.
        for m in 0..(1 << BANDS) {
            out.push(std::array::from_fn(|i| if m >> i & 1 == 1 { 12.0 } else { -12.0 }));
        }
        out
    }

    /// stream: ios-eq. The iOS EQ (shelves made of peaking filters) against the desktop's RBJ
    /// shelves, on every preset and on random curves. Target: <= 0.5 dB RMS each. Run with
    /// `ENCORE_EQ_BENCH=1 cargo test -p player portable_eq -- --nocapture` for the table.
    #[test]
    fn portable_eq_tracks_the_desktop_curve() {
        let show = std::env::var_os("ENCORE_EQ_BENCH").is_some();
        let mut worst = 0.0f64;
        for (name, gains) in &PRESETS[1..] {
            let (rms, max) = portable_error(gains, CURVE_RATE);
            let (rms44, max44) = portable_error(gains, 44_100.0);
            let pre_full = auto_preamp(gains);
            let peak_full = peak_gain_db(gains, Flavor::Full);
            let peak_port = peak_gain_db(gains, Flavor::Portable);
            if show {
                println!(
                    "| {name} | {rms:.3} | {max:.3} | {rms44:.3} | {max44:.3} | {:.2} | {pre_full:.2} | {peak_full:.2} | {peak_port:.2} |",
                    peak_db(gains)
                );
            }
            assert!(rms <= 0.5, "{name}: {rms:.3} dB rms");
            worst = worst.max(rms);
        }
        let curves = random_curves(5000);
        let mut errs: Vec<(f64, f64)> =
            curves.iter().map(|g| portable_error(g, CURVE_RATE)).collect();
        errs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let n = errs.len();
        let mean = errs.iter().map(|e| e.0).sum::<f64>() / n as f64;
        let (p95, max_rms) = (errs[n * 95 / 100].0, errs[n - 1].0);
        let max_abs = errs.iter().fold(0.0f64, |m, e| m.max(e.1));
        let mut e44: Vec<f64> = curves.iter().map(|g| portable_error(g, 44_100.0).0).collect();
        e44.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if show {
            println!("presets: worst {worst:.3} dB rms");
            println!(
                "random ({n}): mean {mean:.3}, p95 {p95:.3}, max {max_rms:.3} dB rms; max abs {max_abs:.3} dB"
            );
            println!(
                "random at 44.1 kHz without the resample: mean {:.3}, max {:.3} dB rms",
                e44.iter().sum::<f64>() / n as f64,
                e44[n - 1]
            );
        }
        assert!(max_rms <= 0.5, "random curve at {max_rms:.3} dB rms");
    }

    #[test]
    fn peak_gain_bounds_every_signal() {
        assert_eq!(peak_gain_db(&[0.0; BANDS], Flavor::Portable), 0.0);
        for (_, gains) in &PRESETS[1..] {
            for flavor in [Flavor::Full, Flavor::Portable] {
                // The L1 bound is never below the curve's own peak.
                let peak = peak_gain_db(gains, flavor);
                let curve = (0..=240)
                    .map(|i| 20.0 * 1000f64.powf(i as f64 / 240.0))
                    .map(|f| flavor_response_db(gains, flavor, f, CURVE_RATE))
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(peak >= curve - 1e-6, "{gains:?} {flavor:?}: {peak} < {curve}");
            }
        }
        // The worst-case signal (the sign-reversed impulse response) reaches the bound.
        let gains = [6.0, 0.0, 0.0, 0.0, 0.0, 6.0];
        let filters = biquads(&gains, Flavor::Portable, CURVE_RATE);
        let run = |mut x: Vec<f64>| {
            for f in &filters {
                let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
                for v in x.iter_mut() {
                    let y = f.b0 * *v + f.b1 * x1 + f.b2 * x2 - f.a1 * y1 - f.a2 * y2;
                    (x2, x1, y2, y1) = (x1, *v, y1, y);
                    *v = y;
                }
            }
            x
        };
        let mut imp = vec![0.0; 24_000];
        imp[0] = 1.0;
        let h = run(imp);
        let worst: Vec<f64> = h.iter().rev().map(|v| v.signum()).collect();
        let y = run(worst);
        let got = 20.0 * y.iter().fold(0.0f64, |m, v| m.max(v.abs())).log10();
        assert!((got - peak_gain_db(&gains, Flavor::Portable)).abs() < 0.01, "{got}");
    }

    fn filter_run(filters: &[Biquad], mut x: Vec<f64>) -> Vec<f64> {
        for f in filters {
            let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
            for v in x.iter_mut() {
                let y = f.b0 * *v + f.b1 * x1 + f.b2 * x2 - f.a1 * y1 - f.a2 * y2;
                (x2, x1, y2, y1) = (x1, *v, y1, y);
                *v = y;
            }
        }
        x
    }

    /// stream: ios-eq. Peak overshoot on loud (full-scale) signals, every preset, levelling off:
    /// the desktop chain with its limiter taken out (what iOS would do with the desktop's preamp
    /// alone) against the Portable stage. The Portable output must never pass full scale.
    /// `ENCORE_EQ_BENCH=1` prints the table.
    #[test]
    fn portable_stage_cannot_clip_loud_signals() {
        let show = std::env::var_os("ENCORE_EQ_BENCH").is_some();
        let n = 24_000;
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        };
        let tau = std::f64::consts::TAU;
        let white: Vec<f64> = (0..n).map(|_| rnd()).collect();
        // A brickwalled master: noise driven 12 dB into a hard clip.
        let wall: Vec<f64> =
            (0..n).map(|_| ((rnd() + rnd() + rnd()) * 4.0).clamp(-1.0, 1.0)).collect();
        let signals: Vec<(&str, Vec<f64>)> = vec![
            ("square 50 Hz", (0..n).map(|i| if (i / 480) % 2 == 0 { 1.0 } else { -1.0 }).collect()),
            ("square 4.8 kHz", (0..n).map(|i| if (i / 5) % 2 == 0 { 1.0 } else { -1.0 }).collect()),
            ("white noise", white),
            ("brickwalled noise", wall),
            (
                "60 Hz + 15 kHz",
                (0..n)
                    .map(|i| {
                        let t = i as f64 / CURVE_RATE;
                        0.5 * (tau * 60.0 * t).sin() + 0.5 * (tau * 15_000.0 * t).sin()
                    })
                    .collect(),
            ),
        ];
        let db = |v: &[f64]| 20.0 * v.iter().fold(0.0f64, |m, x| m.max(x.abs())).log10();
        let mut worst_full = f64::NEG_INFINITY;
        let mut worst_port = f64::NEG_INFINITY;
        for (sig, x) in &signals {
            let (mut wf, mut wp) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for (_, gains) in &PRESETS[1..] {
                let full = filter_run(&biquads(gains, Flavor::Full, CURVE_RATE), x.clone());
                let full_db = db(&full) + auto_preamp(gains);
                let stage = Stage::with_flavor(
                    None,
                    &EqSettings { enabled: true, gains: *gains },
                    Flavor::Portable,
                );
                let port = filter_run(&biquads(gains, Flavor::Portable, CURVE_RATE), x.clone());
                let port_db = db(&port) + stage.eq.unwrap().0;
                wf = wf.max(full_db);
                wp = wp.max(port_db);
                assert!(port_db <= 1e-9, "{sig}, {gains:?}: Portable peak {port_db:.2} dBFS");
            }
            if show {
                println!("| {sig} | {:.2} | {wf:+.2} | {wp:+.2} |", db(x));
            }
            worst_full = worst_full.max(wf);
            worst_port = worst_port.max(wp);
        }
        let mut wf = f64::NEG_INFINITY;
        let mut wp = f64::NEG_INFINITY;
        for (_, gains) in &PRESETS[1..] {
            for flavor in [Flavor::Full, Flavor::Portable] {
                let filters = biquads(gains, flavor, CURVE_RATE);
                let mut imp = vec![0.0; n];
                imp[0] = 1.0;
                let worst: Vec<f64> =
                    filter_run(&filters, imp).iter().rev().map(|v| v.signum()).collect();
                let y = db(&filter_run(&filters, worst));
                match flavor {
                    Flavor::Full => wf = wf.max(y + auto_preamp(gains)),
                    Flavor::Portable => {
                        let st = Stage::with_flavor(
                            None,
                            &EqSettings { enabled: true, gains: *gains },
                            flavor,
                        );
                        let out = y + st.eq.unwrap().0;
                        assert!(out <= 0.01, "{gains:?}: worst-case signal at {out:.2} dBFS");
                        wp = wp.max(out);
                    }
                }
            }
        }
        if show {
            println!("| worst-case (reversed impulse response) | 0.00 | {wf:+.2} | {wp:+.2} |");
            println!("overall: desktop preamp without limiter {worst_full:+.2} dBFS, Portable {worst_port:+.2} dBFS");
        }
    }

    #[test]
    fn portable_stage_never_lifts_a_peak() {
        let eq = EqSettings { enabled: true, gains: [3.0, 0.0, -2.0, 0.0, 0.0, 1.5] };
        let s = Stage::with_flavor(None, &eq, Flavor::Portable);
        assert!(!s.limiter);
        let (pre, _) = s.eq.unwrap();
        assert!(pre <= -peak_gain_db(&eq.gains, Flavor::Portable), "{pre}");
        assert!(pre < auto_preamp(&eq.gains), "L1 headroom is more than the curve's peak");
        let g = s.graph();
        assert!(g.starts_with("aresample=48000,volume@pre=volume="), "{g}");
        for want in [
            "equalizer@b0a=f=21.6:t=q:w=0.816:g=",
            "equalizer@b0b=f=43.52:t=q:w=1.092:g=",
            "equalizer@b1=f=150:t=q:w=1:g=0",
            "equalizer@b2=f=400:t=q:w=1:g=-2",
            "equalizer@b5a=f=16608.8:t=q:w=1.237:g=",
            "equalizer@b5b=f=19774:t=q:w=1.051:g=",
        ] {
            assert!(g.contains(want), "{want} missing: {g}");
        }
        assert!(!g.contains("shelf") && !g.contains("alimiter"), "{g}");
        // Levelled down far enough, the preamp is the desktop's.
        let quietened = Stage::with_flavor(Some(-9.0), &eq, Flavor::Portable);
        assert_eq!(quietened.eq.unwrap().0, auto_preamp(&eq.gains));
        // A boost with nothing to absorb it is dropped; a cut is kept.
        let off = EqSettings::default();
        assert_eq!(Stage::with_flavor(Some(4.0), &off, Flavor::Portable).level, Some(0.0));
        assert_eq!(Stage::with_flavor(Some(-4.0), &off, Flavor::Portable).level, Some(-4.0));
        let boosted = Stage::with_flavor(Some(4.0), &eq, Flavor::Portable);
        let (pre, gains) = boosted.eq.unwrap();
        assert!(4.0 + pre + peak_gain_db(&gains, Flavor::Portable) <= 1e-9);
    }

    #[test]
    fn portable_live_updates_are_commands() {
        let on = EqSettings { enabled: true, gains: [2.0, 0.0, 0.0, 0.0, 0.0, 0.0] };
        let a = Stage::with_flavor(Some(-3.0), &on, Flavor::Portable);
        let b = Stage::with_flavor(
            Some(-5.0),
            &EqSettings { gains: [-4.0, 0.0, 0.0, 4.0, 0.0, 0.0], ..on },
            Flavor::Portable,
        );
        assert!(a.same_shape(&b));
        assert!(!a.same_shape(&Stage::new(Some(-3.0), &on)), "flavors never share a graph");
        let flat: Vec<String> = a.commands_to(&b).iter().map(|c| c.join(" ")).collect();
        assert!(flat.contains(&"enc volume -5dB volume@lvl".to_owned()), "{flat:?}");
        assert!(flat.contains(&"enc g 4 equalizer@b3".to_owned()), "{flat:?}");
        let b0a = LOW_SHELF_FIT[0].gain(-4.0);
        assert!(flat.contains(&format!("enc g {} equalizer@b0a", num(b0a))), "{flat:?}");
        assert!(flat.iter().any(|c| c.ends_with("equalizer@b0b")), "{flat:?}");
        // EQ switched off mid-song on a boosted track: kept in place, flat, and still safe.
        let boosted = Stage::with_flavor(Some(2.0), &on, Flavor::Portable);
        let kept = Stage::with_flavor(Some(2.0), &EqSettings::default(), Flavor::Portable)
            .kept_shape_of(&boosted);
        assert!(kept.same_shape(&boosted));
        let (pre, gains) = kept.eq.unwrap();
        assert_eq!(gains, [0.0; BANDS]);
        assert!(kept.level.unwrap() + pre <= 0.0, "{kept:?}");
    }

    #[test]
    fn lufs_gain_math() {
        // loudnessDb 0 = a -14 LUFS track: Normal leaves it alone.
        assert_eq!(loudness_gain(Some(0.0), Level::Normal), None);
        // A modern master at -6 LUFS (loudnessDb 8).
        assert_eq!(loudness_gain(Some(8.0), Level::Normal), Some(-8.0));
        assert_eq!(loudness_gain(Some(8.0), Level::Loud), Some(-5.0));
        assert_eq!(loudness_gain(Some(8.0), Level::Quiet), Some(-17.0));
        // A quiet older master at -20 LUFS: lifted, within each level's boost budget.
        assert_eq!(loudness_gain(Some(-6.0), Level::Normal), Some(3.0));
        assert_eq!(loudness_gain(Some(-6.0), Level::Loud), Some(6.0));
        assert_eq!(loudness_gain(Some(-6.0), Level::Quiet), Some(-3.0));
        assert_eq!(loudness_gain(Some(-12.0), Level::Quiet), None, "Quiet never boosts");
        // Missing or garbage metadata: no filter.
        assert_eq!(loudness_gain(None, Level::Loud), None);
        assert_eq!(loudness_gain(Some(f64::NAN), Level::Loud), None);
        // Absurd values clamp.
        assert_eq!(loudness_gain(Some(40.0), Level::Normal), Some(-30.0));
        assert_eq!(Level::parse("loud"), Some(Level::Loud));
        assert_eq!(Level::parse("LOUD"), None);
        assert_eq!(Level::default().key(), "normal");
    }
}
