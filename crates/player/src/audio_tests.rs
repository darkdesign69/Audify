//! stream: audio. Real libmpv checks for the Encore audio chain: no effects involved, so these
//! run on a stock Homebrew mpv (the reverb/Rubber Band tests next door need an FFmpeg with them).
//! Output goes to mpv's `pcm` writer, untimed, so each test takes well under a second.
use super::*;
use std::{collections::HashMap, path::Path, time::Instant};

const RATE: usize = 48_000;

fn wait_for(label: &str, timeout: f64, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out: {label}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A 16-bit stereo WAV of a 997 Hz tone at -12 dBFS (997, not 1000: a 1 kHz tone at 48 kHz repeats every 48 samples, and FFmpeg's WAV probe then takes it for S/PDIF and refuses the file).
fn tone_wav(path: &Path, seconds: f64) -> usize {
    let frames = (seconds * RATE as f64).round() as u32;
    let bytes = frames * 4;
    let mut data = Vec::with_capacity(bytes as usize + 44);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(bytes + 36).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");
    data.extend_from_slice(&(RATE as u32).to_le_bytes());
    data.extend_from_slice(&(RATE as u32 * 4).to_le_bytes());
    data.extend_from_slice(b"\x04\0\x10\0data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for frame in 0..frames {
        let t = frame as f64 / RATE as f64;
        let v = ((std::f64::consts::TAU * 997.0 * t).sin() * 0.25 * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
    frames as usize
}

fn pcm(mpv: &Mpv, output: &Path) {
    mpv.set_property("ao", "pcm").unwrap();
    mpv.set_property("ao-pcm-file", output.to_str().unwrap()).unwrap();
    mpv.set_property("ao-pcm-waveheader", false).unwrap();
    mpv.set_property("audio-format", "s16").unwrap();
    mpv.set_property("audio-samplerate", RATE as i64).unwrap();
    mpv.set_property("audio-channels", "stereo").unwrap();
    mpv.set_property("pause", true).unwrap();
}

/// Left channel, as floats.
fn samples(path: &Path) -> Vec<f64> {
    std::fs::read(path)
        .unwrap()
        .chunks_exact(4)
        .map(|s| i16::from_le_bytes([s[0], s[1]]) as f64 / 32768.0)
        .collect()
}

/// Peak level in dBFS of `n` frames from `at`.
fn level(s: &[f64], at: usize, n: usize) -> f64 {
    let peak = s[at..(at + n).min(s.len())].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    20.0 * peak.max(1e-9).log10()
}

const TONE_DB: f64 = -12.04; // 0.25 of full scale

/// Wait for the end, then unload so the pcm writer flushes what it holds.
fn wait_eof(p: &Player) {
    wait_for("EOF", 10.0, || p.mpv().get_property::<bool>("eof-reached").unwrap_or(false));
    p.stop().unwrap();
    wait_for("unload", 2.0, || p.mpv().get_property::<String>("path").is_err());
}

/// LIMUSIC-MAP open item: gapless at normal speed with no effects. Two tracks, the second
/// appended with its own levelling gain, must come out back to back with not one sample missing
/// or inserted, and the gain must change on the boundary sample, not after the app hears about
/// the advance (the old click, bug 4).
#[test]
fn gapless_boundary_is_sample_exact_and_switches_gain_on_the_sample() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    let frames_a = tone_wav(&a, 1.0);
    let frames_b = tone_wav(&b, 1.0);
    let mut p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let mut events = p.take_events().unwrap();
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    p.load(a.to_str().unwrap(), &HashMap::new(), None, None, false).unwrap();
    p.enqueue_with_gain(b.to_str().unwrap(), Some(-12.0)).unwrap();
    p.play().unwrap();
    wait_for("first track end", 10.0, || matches!(events.try_recv(), Ok(PlayerEvent::TrackEnded)));
    // What state.rs does on the advance: retune to the new track's gain. It is already in the
    // graph, so this must neither rebuild nor change anything.
    let before = p.filter_chain();
    p.set_gain(Some(-12.0)).unwrap();
    assert_eq!(p.filter_chain(), before, "the advance rebuilt the chain");
    wait_eof(&p);
    let s = samples(&out);
    assert_eq!(s.len(), frames_a + frames_b, "gap or overlap at the gapless boundary");
    // The tone is continuous across the boundary (same phase), so any step is the gain.
    let pre = level(&s, frames_a - 480, 480);
    let post = level(&s, frames_a, 480);
    assert!((pre - TONE_DB).abs() < 0.1, "track 1 at {pre}");
    assert!((post - (TONE_DB - 12.0)).abs() < 0.1, "track 2's first 10 ms at {post}");
}

/// An EQ drag changes the running graph through `af-command`: the level moves mid-song, and
/// mpv's `af` (which only an `af set` rewrites) stays as it was, so no rebuild happened.
///
/// The source runs at real time (`arealtime`): the pcm writer is untimed, so a file would be
/// rendered to the end before a command could land.
#[test]
fn eq_retunes_live_without_a_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    // On before load: a structural change, applied with the file.
    let mut eq = EqSettings { enabled: true, gains: [0.0, 0.0, 0.0, -1.0, 0.0, 0.0] };
    p.set_eq(eq, true).unwrap();
    let src = "av://lavfi:sine=f=997:sample_rate=48000:d=2.5,arealtime";
    p.load(src, &HashMap::new(), Some(-3.0), None, false).unwrap();
    p.play().unwrap();
    wait_for("playing", 5.0, || p.mpv().get_property::<f64>("time-pos").unwrap_or(0.0) > 0.6);
    let built = p.filter_chain();
    assert!(built.contains("equalizer@b3") && built.contains("volume@lvl"), "{built}");
    // Switching off mid-song zeroes the EQ in place instead of rebuilding...
    p.set_eq(EqSettings { enabled: false, ..eq }, true).unwrap();
    assert_eq!(p.filter_chain(), built, "switching off rebuilt the graph");
    // ...and a drag on the way back is still a command: 1 kHz cut by 9 dB.
    eq.gains[3] = -9.0;
    p.set_eq(eq, false).unwrap();
    assert_eq!(p.filter_chain(), built, "a drag rebuilt the graph");
    wait_eof(&p);
    let s = samples(&out);
    let before = level(&s, 4800, 4800);
    // mpv filters ahead of what is written, so give the change time to reach the output.
    let after = level(&s, s.len() - 4800, 2400);
    assert!(((before - after) - 8.0).abs() < 0.3, "-1 dB to -9 dB moved {before} -> {after}");
}

/// The EQ maths and FFmpeg agree: a boosted band comes out boosted, minus the auto preamp.
#[test]
fn eq_boost_matches_the_curve_and_the_preamp() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    tone_wav(&a, 0.5);
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    let gains = [0.0, 0.0, 6.0, 6.0, 0.0, 0.0];
    p.set_eq(EqSettings { enabled: true, gains }, true).unwrap();
    p.load(a.to_str().unwrap(), &HashMap::new(), None, None, false).unwrap();
    p.play().unwrap();
    wait_eof(&p);
    let s = samples(&out);
    let got = level(&s, s.len() / 2, 4800);
    let want = TONE_DB + dsp::response_db(&gains, 997.0, 48_000.0) + dsp::auto_preamp(&gains);
    assert!((got - want).abs() < 0.3, "got {got}, curve says {want}");
}

/// Sleep timer, end of track: stop exactly at the end of the playing file. mpv then moves on to
/// the next entry *paused* at its start (the app's queue follows through the normal track end),
/// and play carries on from there. Real-time null output: pausing is about time.
#[test]
fn stop_after_current_pauses_at_the_end_then_continues() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    tone_wav(&a, 0.4);
    tone_wav(&b, 0.4);
    let mut p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let mut events = p.take_events().unwrap();
    p.mpv().set_property("ao", "null").unwrap();
    p.load(a.to_str().unwrap(), &HashMap::new(), None, None, false).unwrap();
    p.enqueue(b.to_str().unwrap()).unwrap();
    p.set_stop_after_current(true);
    p.set_fade_gain(-30.0);
    p.play().unwrap();
    let mut ended = 0;
    wait_for("stopped on the next track", 5.0, || {
        while let Ok(e) = events.try_recv() {
            ended += matches!(e, PlayerEvent::TrackEnded) as usize;
        }
        ended == 1 && p.mpv().get_property::<i64>("playlist-pos").unwrap_or(0) == 1
    });
    assert!(!p.stop_after_current());
    assert!(p.take_held_at_end(), "the queue is told the timer stopped here");
    assert!(!p.take_held_at_end(), "and only once");
    std::thread::sleep(Duration::from_millis(250));
    assert!(p.is_paused(), "the next track started by itself");
    let at = p.mpv().get_property::<f64>("time-pos").unwrap();
    assert!(at < 0.05, "the next track moved while paused: {at}");
    assert_eq!(p.mpv().get_property::<f64>("volume-gain").unwrap(), 0.0, "fade not reset");
    assert_eq!(p.mpv().get_property::<String>("keep-open").unwrap(), "no");
    assert!(p.mpv().get_property::<bool>("gapless-audio").unwrap_or(true));
    p.play().unwrap();
    wait_for("next track plays", 5.0, || {
        while let Ok(e) = events.try_recv() {
            ended += matches!(e, PlayerEvent::TrackEnded) as usize;
        }
        ended == 2
    });
}

/// The sleep timer's fade gain sits on top of the volume and comes off cleanly.
#[test]
fn fade_gain_scales_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    tone_wav(&a, 0.3);
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    p.set_fade_gain(-20.0);
    p.load(a.to_str().unwrap(), &HashMap::new(), None, None, false).unwrap();
    p.play().unwrap();
    wait_eof(&p);
    let s = samples(&out);
    assert!((level(&s, 4800, 4800) - (TONE_DB - 20.0)).abs() < 0.3);
    p.set_fade_gain(f64::NAN);
    assert_eq!(p.mpv().get_property::<f64>("volume-gain").unwrap(), 0.0);
}

#[test]
fn output_devices_are_listed_and_selectable() {
    let dir = tempfile::tempdir().unwrap();
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    let devices = p.audio_devices();
    assert!(devices.iter().any(|d| d.name == "auto"), "{devices:?}");
    assert!(devices.iter().all(|d| !d.description.is_empty()));
    let pick = devices.last().unwrap().name.clone();
    p.set_audio_device(&pick).unwrap();
    assert_eq!(p.audio_device(), pick);
    p.set_audio_device("auto").unwrap();
    assert_eq!(p.audio_device(), "auto");
}

/// Tempo and the Encore stage share one chain: speed keeps the EQ and the gain, and an EQ drag
/// at 1.25x is still a live command, not a rebuild of the Rubber Band graph.
#[test]
fn speed_and_eq_share_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    tone_wav(&a, 3.0);
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    p.mpv().set_property("ao", "null").unwrap();
    let mut eq = EqSettings { enabled: true, gains: [2.0, 0.0, 0.0, 0.0, 0.0, -2.0] };
    p.set_eq(eq, true).unwrap();
    p.load(a.to_str().unwrap(), &HashMap::new(), Some(-4.0), None, false).unwrap();
    p.play().unwrap();
    wait_for("warm", 5.0, || warm(p.mpv()));
    p.set_playback_params(1.25, 0, Reverb::default()).unwrap();
    let chain = p.filter_chain();
    assert!(chain.contains("equalizer@b1") && chain.contains("volume=-4dB"), "{chain}");
    assert!(chain.contains("rubberband"), "{chain}");
    assert!(chain.find("@enc").unwrap() < chain.find("rubberband").unwrap(), "EQ before tempo");
    eq.gains[2] = 5.0;
    p.set_eq(eq, false).unwrap();
    p.set_gain(Some(-1.0)).unwrap();
    assert_eq!(p.filter_chain(), chain, "a drag or a retune at 1.25x rebuilt the graph");
    p.set_playback_params(1.0, 0, Reverb::default()).unwrap();
    let back = p.filter_chain();
    assert!(!back.contains("rubberband") && back.contains("g=5"), "{back}");
    p.stop().unwrap();
}

/// Crossfade: the incoming deck is loaded with its own track's gain and the session EQ, and a
/// live EQ change reaches both decks.
#[test]
fn crossfade_deck_carries_its_gain_and_the_eq() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    tone_wav(&a, 3.0);
    tone_wav(&b, 3.0);
    let p = Player::new(dir.path().to_str().unwrap()).unwrap();
    p.mpv().set_property("ao", "null").unwrap();
    p.set_crossfade(Some(12.0));
    assert_eq!(p.decks.crossfade_ms.load(Ordering::Relaxed), 12_000, "Spotify's 12 s maximum");
    p.set_crossfade(Some(1.0));
    let mut eq = EqSettings { enabled: true, gains: [0.0, 3.0, 0.0, 0.0, 0.0, 0.0] };
    p.set_eq(eq, true).unwrap();
    p.load(a.to_str().unwrap(), &HashMap::new(), Some(-2.0), None, false).unwrap();
    p.play().unwrap();
    p.enqueue_with_gain(b.to_str().unwrap(), Some(-7.5)).unwrap();
    let idle = p.decks.mpv(1).unwrap().clone();
    wait_for("preloaded", 5.0, || p.decks.preloaded.load(Ordering::Acquire));
    let chain = idle.get_property::<String>("af").unwrap();
    assert!(chain.contains("volume=-7.5dB") && chain.contains("equalizer@b1"), "{chain}");
    eq.gains[1] = -3.0;
    p.set_eq(eq, true).unwrap();
    let applied = p.decks.applied.lock().unwrap().clone();
    for (deck, a) in applied.iter().enumerate() {
        let a = a.as_ref().unwrap_or_else(|| panic!("deck {deck} untracked"));
        assert_eq!(a.stage.eq.unwrap().1[1], -3.0, "deck {deck} missed the EQ change");
    }
    p.stop().unwrap();
}

// ── stream: ios-eq. The portable chain (MPVKit's filters), run on the desktop's mpv ──────────

/// A 16-bit stereo WAV at `rate` of `signal(t)` (clamped to full scale).
fn wav_at(path: &Path, rate: u32, seconds: f64, signal: impl Fn(f64) -> f64) -> usize {
    let frames = (seconds * rate as f64).round() as u32;
    let bytes = frames * 4;
    let mut data = Vec::with_capacity(bytes as usize + 44);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(bytes + 36).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 4).to_le_bytes());
    data.extend_from_slice(b"\x04\0\x10\0data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for frame in 0..frames {
        let v = (signal(frame as f64 / rate as f64).clamp(-1.0, 1.0) * 32767.0) as i16;
        data.extend_from_slice(&v.to_le_bytes());
        data.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
    frames as usize
}

fn portable_player(dir: &Path) -> Player {
    let p = Player::new(dir.to_str().unwrap()).unwrap();
    assert!(p.force_dsp_flavor(dsp::Flavor::Portable));
    p
}

/// Rock, the preset with both shelves well up.
fn rock() -> EqSettings {
    EqSettings { enabled: true, gains: dsp::preset("Rock").unwrap() }
}

/// Gapless with the EQ on, through the portable chain: not a sample gained or lost at the join
/// (its `aresample` included), the level is the portable curve's, and no shelf or limiter in it.
#[test]
fn portable_chain_is_gapless_and_follows_its_curve() {
    let dir = tempfile::tempdir().unwrap();
    for rate in [48_000u32, 44_100] {
        let (a, b) =
            (dir.path().join(format!("a{rate}.wav")), dir.path().join(format!("b{rate}.wav")));
        let tone = |t: f64| (std::f64::consts::TAU * 997.0 * t).sin() * 0.25;
        let fa = wav_at(&a, rate, 1.0, tone);
        let fb = wav_at(&b, rate, 1.0, tone);
        let mut p = portable_player(dir.path());
        let mut events = p.take_events().unwrap();
        let out = dir.path().join(format!("out{rate}.pcm"));
        pcm(p.mpv(), &out);
        p.mpv().set_property("keep-open", true).unwrap();
        p.set_eq(rock(), true).unwrap();
        p.load(a.to_str().unwrap(), &HashMap::new(), Some(-3.0), None, false).unwrap();
        p.enqueue_with_gain(b.to_str().unwrap(), Some(-9.0)).unwrap();
        p.play().unwrap();
        wait_for("first track end", 10.0, || {
            matches!(events.try_recv(), Ok(PlayerEvent::TrackEnded))
        });
        let chain = p.filter_chain();
        p.set_gain(Some(-9.0)).unwrap();
        assert_eq!(p.filter_chain(), chain, "the advance rebuilt the chain");
        assert!(chain.contains("equalizer@b0a") && chain.contains("equalizer@b5b"), "{chain}");
        assert!(!chain.contains("shelf") && !chain.contains("alimiter"), "{chain}");
        wait_eof(&p);
        let s = samples(&out);
        let want = ((fa + fb) as f64 * RATE as f64 / rate as f64).round() as usize;
        assert!(s.len().abs_diff(want) <= 1, "{rate} Hz: {} frames, want {want}", s.len());
        let stage = |g| dsp::Stage::with_flavor(Some(g), &rock(), dsp::Flavor::Portable);
        let curve = dsp::flavor_response_db(&rock().gains, dsp::Flavor::Portable, 997.0, 48_000.0);
        for (at, g) in [(s.len() / 4, -3.0), (s.len() * 3 / 4, -9.0)] {
            let st = stage(g);
            let want = TONE_DB + g + st.eq.unwrap().0 + curve;
            let got = level(&s, at, 4800);
            assert!((got - want).abs() < 0.3, "{rate} Hz at {at}: {got}, curve says {want}");
        }
    }
}

/// A drag retunes the portable graph live (both halves of a shelf move) without a rebuild.
#[test]
fn portable_eq_retunes_live_without_a_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let p = portable_player(dir.path());
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    let mut eq = EqSettings { enabled: true, gains: [0.0, 0.0, 0.0, -1.0, 0.0, 0.0] };
    p.set_eq(eq, true).unwrap();
    let src = "av://lavfi:sine=f=997:sample_rate=48000:d=2.5,arealtime";
    p.load(src, &HashMap::new(), Some(-12.0), None, false).unwrap();
    p.play().unwrap();
    wait_for("playing", 5.0, || p.mpv().get_property::<f64>("time-pos").unwrap_or(0.0) > 0.6);
    let built = p.filter_chain();
    assert!(built.contains("equalizer@b0a") && built.contains("volume@lvl"), "{built}");
    eq.gains[3] = -9.0;
    eq.gains[0] = 6.0;
    let t0 = Instant::now();
    p.set_eq(eq, false).unwrap();
    eprintln!("portable drag: set_eq took {:?}", t0.elapsed());
    let t0 = Instant::now();
    let _ = dsp::Stage::with_flavor(Some(-12.0), &eq, dsp::Flavor::Portable);
    eprintln!("portable stage maths: {:?}", t0.elapsed());
    assert_eq!(p.filter_chain(), built, "a drag rebuilt the graph");
    wait_eof(&p);
    let s = samples(&out);
    let before = level(&s, 4800, 4800);
    let after = level(&s, s.len() - 4800, 2400);
    let st = |gains| {
        dsp::Stage::with_flavor(
            Some(-12.0),
            &EqSettings { enabled: true, gains },
            dsp::Flavor::Portable,
        )
    };
    let moved = |g: [f64; 6]| {
        st(g).eq.unwrap().0 + dsp::flavor_response_db(&g, dsp::Flavor::Portable, 997.0, 48_000.0)
    };
    let want = moved([0.0, 0.0, 0.0, -1.0, 0.0, 0.0]) - moved(eq.gains);
    assert!(((before - after) - want).abs() < 0.3, "{before} -> {after}, want a {want} dB drop");
}

/// Loud input, boosted EQ, no limiter: the portable stage's headroom keeps every sample at or
/// under full scale. Float output, so an over shows up instead of being clipped by the writer.
#[test]
fn portable_chain_never_clips_a_full_scale_signal() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("square.wav");
    // 50 Hz full-scale square: every bass boost's worst friend.
    wav_at(&a, 48_000, 1.0, |t| if (t * 100.0).floor() as i64 % 2 == 0 { 1.0 } else { -1.0 });
    for name in ["Bass Booster", "Rock", "Electronic", "Loudness"] {
        let p = portable_player(dir.path());
        let out = dir.path().join("loud.pcm");
        pcm(p.mpv(), &out);
        p.mpv().set_property("audio-format", "float").unwrap();
        p.mpv().set_property("keep-open", true).unwrap();
        p.set_eq(EqSettings { enabled: true, gains: dsp::preset(name).unwrap() }, true).unwrap();
        // Normal levelling would lift this "quiet" track by 3 dB on the desktop (with its
        // limiter); here the boost has to be absorbed.
        p.load(a.to_str().unwrap(), &HashMap::new(), Some(3.0), None, false).unwrap();
        p.play().unwrap();
        wait_eof(&p);
        let peak = std::fs::read(&out)
            .unwrap()
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs())
            .fold(0.0f32, f32::max);
        let db = 20.0 * (peak as f64).log10();
        eprintln!("{name}: peak {db:.2} dBFS");
        assert!(peak > 0.1 && peak <= 1.0, "{name}: peak {db:.2} dBFS");
    }
}

/// No Rubber Band (MPVKit): speed is mpv's scaletempo2, and a gapless join at 1.25x neither
/// drops nor repeats audio: the output is (a + b) / 1.25 long and the tone never dips.
#[test]
fn scaletempo2_speed_is_gapless_without_rubberband() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
    let fa = tone_wav(&a, 1.0);
    let fb = tone_wav(&b, 1.0);
    let mut p = portable_player(dir.path());
    let mut events = p.take_events().unwrap();
    let out = dir.path().join("out.pcm");
    pcm(p.mpv(), &out);
    p.mpv().set_property("keep-open", true).unwrap();
    NO_RUBBERBAND.set(true);
    assert!(!p.can_shift_pitch());
    p.set_speed(1.25).unwrap();
    assert!(matches!(p.set_pitch(2), Err(Error::NoPitchFilter)));
    p.set_eq(rock(), true).unwrap();
    // The seam is per thread and every chain is built on this one: keep it on to the end.
    p.load(a.to_str().unwrap(), &HashMap::new(), None, None, false).unwrap();
    p.enqueue(b.to_str().unwrap()).unwrap();
    p.play().unwrap();
    wait_for("first track end", 10.0, || matches!(events.try_recv(), Ok(PlayerEvent::TrackEnded)));
    assert!(!p.filter_chain().contains("rubberband"), "{}", p.filter_chain());
    wait_eof(&p);
    NO_RUBBERBAND.set(false);
    let s = samples(&out);
    let want = ((fa + fb) as f64 / 1.25).round() as usize;
    eprintln!("scaletempo2 1.25x gapless: {} frames, want {want}", s.len());
    assert!(s.len().abs_diff(want) <= 2, "{} frames, want {want}", s.len());
    // Every 5 ms window from 20 ms in to 20 ms before the end holds the tone (no gap, no dip).
    let curve = dsp::flavor_response_db(&rock().gains, dsp::Flavor::Portable, 997.0, 48_000.0);
    let st = dsp::Stage::with_flavor(None, &rock(), dsp::Flavor::Portable);
    let want_db = TONE_DB + curve + st.eq.unwrap().0;
    let worst = (960..s.len() - 960 - 240)
        .step_by(240)
        .map(|at| level(&s, at, 240))
        .fold(f64::INFINITY, f64::min);
    assert!(worst > want_db - 1.0, "dip to {worst} dB (tone at {want_db})");
}
