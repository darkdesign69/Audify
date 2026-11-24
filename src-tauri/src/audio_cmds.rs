//! Tauri commands for the audio settings (EQ, volume level, crossfade, output device, sleep timer).
//! The bodies live in `encore_core::audio`, shared with iOS.

use std::sync::Arc;

use encore_core::audio::{self, AudioSettings, DevicesState, EqInfo, EqState, SleepState};
use encore_core::state::AppState;
use player::dsp;
use tauri::State;

type St<'a> = State<'a, Arc<AppState>>;

#[tauri::command]
pub async fn audio_settings(state: St<'_>) -> Result<AudioSettings, String> {
    audio::audio_settings(&state).await
}

#[tauri::command]
pub fn audio_eq_info() -> EqInfo {
    audio::audio_eq_info()
}

#[tauri::command]
pub async fn set_eq(
    state: St<'_>,
    enabled: bool,
    gains: [f64; dsp::BANDS],
    commit: bool,
) -> Result<EqState, String> {
    audio::set_eq(&state, enabled, gains, commit).await
}

#[tauri::command]
pub async fn set_eq_preset(state: St<'_>, name: String) -> Result<EqState, String> {
    audio::set_eq_preset(&state, name).await
}

#[tauri::command]
pub async fn set_normalize_volume(state: St<'_>, on: bool) -> Result<(), String> {
    audio::set_normalize_volume(&state, on).await
}

#[tauri::command]
pub async fn set_volume_level(state: St<'_>, level: String) -> Result<(), String> {
    audio::set_volume_level(&state, level).await
}

#[tauri::command]
pub async fn set_crossfade_secs(state: St<'_>, secs: f64) -> Result<f64, String> {
    audio::set_crossfade_secs(&state, secs).await
}

#[tauri::command]
pub async fn audio_devices(state: St<'_>) -> Result<DevicesState, String> {
    audio::audio_devices(&state).await
}

#[tauri::command]
pub async fn set_audio_device(state: St<'_>, name: String) -> Result<DevicesState, String> {
    audio::set_audio_device(&state, name).await
}

#[tauri::command]
pub async fn set_sleep_timer(
    state: St<'_>,
    minutes: Option<f64>,
    end_of_track: bool,
) -> Result<SleepState, String> {
    audio::set_sleep_timer(&state, minutes, end_of_track).await
}

#[tauri::command]
pub async fn cancel_sleep_timer(state: St<'_>) -> Result<SleepState, String> {
    audio::cancel_sleep_timer(&state).await
}

#[tauri::command]
pub async fn sleep_timer() -> Result<SleepState, String> {
    audio::sleep_timer().await
}
