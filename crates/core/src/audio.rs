//! Encore audio stream: the app side of the equalizer, volume levelling (Spotify's Quiet / Normal /
//! Loud), the output device picker, the crossfade length and the sleep timer. The DSP and the mpv
//! work live in `crates/player` (`dsp.rs`, `sleep.rs`, `lib.rs`); this file persists settings,
//! holds the command bodies (wrapped by the desktop's Tauri commands) and runs two small background loops (device watch, sleep ticker).
//!
//! Settings keys (SQLite `settings`): `eq_enabled`, `eq_gains` (JSON array of 6 dB values),
//! `volume_level` (`quiet|normal|loud`), `audio_device` (mpv device name, absent = system
//! default). `normalize_volume`, `crossfade` and `crossfade_secs` are Limusic's existing keys.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use player::dsp::{self, EQ_BANDS, MAX_BAND_DB, PRESETS};
use player::sleep::{Action, SleepMode, SleepTimer, FADE_SECS};
use player::{AudioDevice, EqSettings, Level, Player};
use serde::Serialize;

use crate::db::Db;
use crate::host::HostExt;
use crate::state::AppState;


const K_EQ_ENABLED: &str = "eq_enabled";
const K_EQ_GAINS: &str = "eq_gains";
const K_LEVEL: &str = "volume_level";
const K_DEVICE: &str = "audio_device";

// ── Persistence ─────────────────────────────────────────────────────────────────────────────────

pub fn saved_eq(db: &Db) -> EqSettings {
    let enabled = db.get_setting(K_EQ_ENABLED).as_deref() == Some("true");
    let gains = db
        .get_setting(K_EQ_GAINS)
        .and_then(|s| serde_json::from_str::<[f64; dsp::BANDS]>(&s).ok())
        .unwrap_or([0.0; dsp::BANDS]);
    EqSettings { enabled, gains }.sanitized()
}

fn save_eq(db: &Db, eq: &EqSettings) {
    db.set_setting(K_EQ_ENABLED, if eq.enabled { "true" } else { "false" });
    db.set_setting(K_EQ_GAINS, &serde_json::to_string(&eq.gains).unwrap_or_default());
}

pub fn saved_level(db: &Db) -> Level {
    db.get_setting(K_LEVEL).as_deref().and_then(Level::parse).unwrap_or_default()
}

fn normalize_on(db: &Db) -> bool {
    db.get_setting("normalize_volume").as_deref() != Some("false")
}

/// The saved output device, `None` for the system default.
pub fn saved_device(db: &Db) -> Option<String> {
    db.get_setting(K_DEVICE).filter(|d| !d.is_empty() && d != "auto")
}

/// The levelling gain for one track at the user's chosen level. `state::AppState::track_gain`
/// calls this (and owns the on/off switch).
pub fn track_loudness_gain(db: &Db, loudness_db: Option<f64>) -> Option<f64> {
    dsp::loudness_gain(loudness_db, saved_level(db))
}

/// Startup: put the saved EQ and device on the player before anything plays.
pub fn apply_saved(player: &Player, db: &Db) {
    if let Err(e) = player.set_eq(saved_eq(db), true) {
        tracing::warn!(error = %e, "couldn't apply the saved equalizer");
    }
    let want = effective_device(saved_device(db), &player.audio_devices());
    if let Err(e) = player.set_audio_device(&want) {
        tracing::warn!(error = %e, device = want, "couldn't select the saved audio device");
    }
}

/// The device to use: the saved one while it is connected, else the system default. The choice
/// itself stays saved, so headphones that come back are picked up again.
fn effective_device(saved: Option<String>, devices: &[AudioDevice]) -> String {
    match saved {
        Some(d) if devices.iter().any(|x| x.name == d) => d,
        _ => "auto".to_owned(),
    }
}

// ── State the UI reads ──────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EqState {
    pub enabled: bool,
    pub gains: [f64; dsp::BANDS],
    /// The preset these gains are, or `None` ("Custom").
    pub preset: Option<&'static str>,
    /// The automatic pre-gain the player applies (dB, <= 0), for display.
    pub preamp: f64,
}

impl From<EqSettings> for EqState {
    fn from(eq: EqSettings) -> Self {
        EqState {
            enabled: eq.enabled,
            gains: eq.gains,
            preset: dsp::preset_name(&eq.gains),
            preamp: dsp::auto_preamp(&eq.gains),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBandInfo {
    pub kind: dsp::BandKind,
    pub freq: f64,
    pub q: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPresetInfo {
    pub name: &'static str,
    pub gains: [f64; dsp::BANDS],
}

/// Static facts about the EQ, for the graph and the preset menu.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EqInfo {
    pub bands: Vec<EqBandInfo>,
    pub presets: Vec<EqPresetInfo>,
    pub max_db: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DevicesState {
    pub devices: Vec<AudioDevice>,
    /// What the user picked (`auto` = system default).
    pub selected: String,
    /// What mpv is using now: `selected`, or `auto` while that device is unplugged.
    pub active: String,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SleepState {
    pub mode: Option<SleepMode>,
    /// Seconds left on a timed mode, at the moment this was made (`None` for end of track/off).
    pub remaining: Option<f64>,
    /// The fade lasts this long before the stop.
    pub fade_secs: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub eq: EqState,
    pub normalize: bool,
    pub level: Level,
    /// 0 = off (gapless).
    pub crossfade_secs: f64,
    pub device: String,
    pub sleep: SleepState,
}

/// Managed alongside `AppState`. Only the sleep timer lives here; everything else is in mpv
/// and the settings table.
#[derive(Default)]
pub struct AudioCtl {
    sleep: Mutex<SleepTimer>,
    /// stream: perf — wakes the sleep ticker when a timer is set; disarmed, it sleeps for good
    /// instead of waking five times a second for nothing.
    armed: tokio::sync::Notify,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

impl AudioCtl {
    pub fn sleep_state(&self) -> SleepState {
        let t = *self.sleep.lock().unwrap();
        SleepState { mode: t.mode, remaining: t.remaining(now()), fade_secs: FADE_SECS }
    }
}

fn devices_state(player: &Player, db: &Db) -> DevicesState {
    DevicesState {
        devices: player.audio_devices(),
        selected: saved_device(db).unwrap_or_else(|| "auto".to_owned()),
        active: player.audio_device(),
    }
}

// ── Background loops ────────────────────────────────────────────────────────────────────────────

/// Called once from setup, after `AppState` is managed.
pub fn start(state: Arc<AppState>) {
    spawn_device_watch(state.clone());
    spawn_sleep_ticker(state, ctl());
}

/// The sleep timer's state, one per process.
pub fn ctl() -> Arc<AudioCtl> {
    static CTL: std::sync::OnceLock<Arc<AudioCtl>> = std::sync::OnceLock::new();
    CTL.get_or_init(Default::default).clone()
}

/// mpv refreshes `audio-device-list` on CoreAudio hotplug; poll it (cheap, no device I/O) and
/// follow the saved choice: fall back to the default when it disappears, return to it when it
/// comes back, and tell the UI either way.
fn spawn_device_watch(state: Arc<AppState>) {
    crate::rt::spawn(async move {
        let mut last: Option<DevicesState> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let player = &state.player;
            let devices = player.audio_devices();
            let want = effective_device(saved_device(&state.db), &devices);
            if player.audio_device() != want {
                tracing::info!(device = want, "audio device changed, following");
                if let Err(e) = player.set_audio_device(&want) {
                    tracing::warn!(error = %e, "couldn't switch the audio device");
                }
            }
            let now = devices_state(player, &state.db);
            let changed = last.as_ref().is_none_or(|l| {
                l.devices != now.devices || l.active != now.active || l.selected != now.selected
            });
            if changed && last.is_some() {
                let _ = state.host.emit("audio-devices", &now);
            }
            last = Some(now);
        }
    });
}

/// Five ticks a second: smooth enough for a 10 s fade, nothing for the CPU.
fn spawn_sleep_ticker(state: Arc<AppState>, ctl: Arc<AudioCtl>) {
    crate::rt::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        let mut fading = false;
        loop {
            tick.tick().await;
            let timer = *ctl.sleep.lock().unwrap();
            if !timer.armed() {
                if fading {
                    state.player.set_fade_gain(0.0);
                    fading = false;
                }
                ctl.armed.notified().await;
                tick.reset();
                continue;
            }
            let player = &state.player;
            // End of track: the player stops itself on the exact sample and clears its flag.
            if timer.mode == Some(SleepMode::EndOfTrack) && !player.stop_after_current() {
                finish(&state, &ctl);
                fading = false;
                continue;
            }
            let left = if timer.mode == Some(SleepMode::EndOfTrack) && !player.is_paused() {
                player.track_left()
            } else {
                None
            };
            match timer.tick(now(), left) {
                Action::Wait => {
                    if fading {
                        player.set_fade_gain(0.0);
                        fading = false;
                    }
                }
                Action::Fade(db) => {
                    player.set_fade_gain(db);
                    fading = true;
                }
                Action::Stop => {
                    tracing::info!("sleep timer: pausing");
                    let _ = player.pause();
                    // Paused before the gain comes back, so nothing is heard at full level.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    player.set_fade_gain(0.0);
                    fading = false;
                    finish(&state, &ctl);
                }
            }
        }
    });
}

fn finish(state: &AppState, ctl: &AudioCtl) {
    ctl.sleep.lock().unwrap().cancel();
    let _ = state.host.emit("sleep-timer", ctl.sleep_state());
}

// ── Commands ────────────────────────────────────────────────────────────────────────────────────

/// Everything the Playback / Audio settings and the player bar need, in one call.
pub async fn audio_settings(state: &AppState) -> Result<AudioSettings, String> {
    let db = &state.db;
    Ok(AudioSettings {
        eq: state.player.eq().into(),
        normalize: normalize_on(db),
        level: saved_level(db),
        crossfade_secs: crate::state::saved_crossfade(db).unwrap_or(0.0),
        device: saved_device(db).unwrap_or_else(|| "auto".to_owned()),
        sleep: ctl().sleep_state(),
    })
}

pub fn audio_eq_info() -> EqInfo {
    EqInfo {
        bands: EQ_BANDS.iter().map(|b| EqBandInfo { kind: b.kind, freq: b.freq, q: b.q }).collect(),
        presets: PRESETS.iter().map(|(name, gains)| EqPresetInfo { name, gains: *gains }).collect(),
        max_db: MAX_BAND_DB,
    }
}

/// Set the equalizer. While a band is being dragged send `commit: false` (live, not saved, at
/// most once per animation frame); on release, a preset pick or the switch send `commit: true`.
pub async fn set_eq(
    state: &AppState,
    enabled: bool,
    gains: [f64; dsp::BANDS],
    commit: bool,
) -> Result<EqState, String> {
    let eq = EqSettings { enabled, gains }.sanitized();
    state.player.set_eq(eq, commit).map_err(|e| e.to_string())?;
    if commit {
        save_eq(&state.db, &eq);
    }
    Ok(eq.into())
}

/// Pick a preset by name (case-insensitive). Turns the EQ on, like Spotify.
pub async fn set_eq_preset(state: &AppState, name: String) -> Result<EqState, String> {
    let gains = dsp::preset(&name).ok_or_else(|| format!("No preset named {name}"))?;
    let eq = EqSettings { enabled: true, gains };
    state.player.set_eq(eq, true).map_err(|e| e.to_string())?;
    save_eq(&state.db, &eq);
    Ok(eq.into())
}

/// Spotify's "Normalize volume" switch (Limusic's `normalize_volume` key).
pub async fn set_normalize_volume(state: &AppState, on: bool) -> Result<(), String> {
    state.db.set_setting("normalize_volume", if on { "true" } else { "false" });
    state.reapply_gain().await;
    Ok(())
}

/// Spotify's "Volume level": `quiet` (-23 LUFS), `normal` (-14), `loud` (-11, limiter).
/// Retunes the playing track at once (a live gain change, no rebuild).
pub async fn set_volume_level(state: &AppState, level: String) -> Result<(), String> {
    let level = Level::parse(&level).ok_or_else(|| format!("Unknown volume level {level}"))?;
    state.db.set_setting(K_LEVEL, level.key());
    state.reapply_gain().await;
    Ok(())
}

/// Crossfade length, 0 (off, gapless) to 12 s. Applies from the next track change.
pub async fn set_crossfade_secs(state: &AppState, secs: f64) -> Result<f64, String> {
    let secs = if secs.is_finite() { secs.clamp(0.0, 12.0).round() } else { 0.0 };
    if secs < 1.0 {
        state.db.set_setting("crossfade", "false");
    } else {
        state.db.set_setting("crossfade", "true");
        state.db.set_setting("crossfade_secs", &secs.to_string());
    }
    state.apply_crossfade().await;
    Ok(if secs < 1.0 { 0.0 } else { secs })
}

pub async fn audio_devices(state: &AppState) -> Result<DevicesState, String> {
    Ok(devices_state(&state.player, &state.db))
}

/// Pick the output (`auto` = follow the system default). Saved; applied now if connected.
pub async fn set_audio_device(
    state: &AppState,
    name: String,
) -> Result<DevicesState, String> {
    let devices = state.player.audio_devices();
    if name != "auto" && !devices.iter().any(|d| d.name == name) {
        return Err("That audio device isn't connected".to_owned());
    }
    state.db.set_setting(K_DEVICE, if name == "auto" { "" } else { &name });
    state.player.set_audio_device(&name).map_err(|e| e.to_string())?;
    let now = devices_state(&state.player, &state.db);
    let _ = state.host.emit("audio-devices", &now);
    Ok(now)
}

/// Start (or replace) the sleep timer: `minutes` for a timed stop (Spotify offers 5, 10, 15, 30,
/// 45, 60), or `endOfTrack` to stop when the playing track or episode ends.
pub async fn set_sleep_timer(
    state: &AppState,
    minutes: Option<f64>,
    end_of_track: bool,
) -> Result<SleepState, String> {
    let ctl = ctl();
    {
        let mut t = ctl.sleep.lock().unwrap();
        if end_of_track {
            t.set_end_of_track();
        } else {
            let m = minutes.filter(|m| m.is_finite() && *m > 0.0).ok_or("Pick a duration")?;
            t.set_minutes(now(), m.min(24.0 * 60.0));
        }
    }
    state.player.set_stop_after_current(end_of_track);
    state.player.set_fade_gain(0.0);
    ctl.armed.notify_one();
    let s = ctl.sleep_state();
    let _ = state.host.emit("sleep-timer", &s);
    Ok(s)
}

pub async fn cancel_sleep_timer(
    state: &AppState,
) -> Result<SleepState, String> {
    let ctl = ctl();
    ctl.sleep.lock().unwrap().cancel();
    state.player.set_stop_after_current(false);
    state.player.set_fade_gain(0.0);
    let s = ctl.sleep_state();
    let _ = state.host.emit("sleep-timer", &s);
    Ok(s)
}

pub async fn sleep_timer() -> Result<SleepState, String> {
    Ok(ctl().sleep_state())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_falls_back_while_unplugged() {
        let list = |names: &[&str]| -> Vec<AudioDevice> {
            names
                .iter()
                .map(|n| AudioDevice { name: n.to_string(), description: n.to_string() })
                .collect()
        };
        let devices = list(&["auto", "coreaudio/AirPods"]);
        assert_eq!(effective_device(None, &devices), "auto");
        assert_eq!(
            effective_device(Some("coreaudio/AirPods".into()), &devices),
            "coreaudio/AirPods"
        );
        assert_eq!(effective_device(Some("coreaudio/AirPods".into()), &list(&["auto"])), "auto");
    }
}
