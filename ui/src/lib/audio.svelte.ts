// Spotless audio stream: reactive state for the equalizer, volume levelling, crossfade, output
// device and sleep timer. State and actions only; the UI stream builds the screens:
//   Settings > Playback: crossfade slider (0–12 s), "Normalize volume" + Volume level,
//   Settings > Audio quality / Equalizer: EqGraph.svelte + preset menu + switch,
//   player bar "Connect to a device" icon: `audio.devices`,
//   now-playing "..." menu > Sleep timer: SLEEP_OPTIONS, `audio.sleepLeft` for the countdown.
// Call `initAudio()` once from the root layout.
import * as api from '$lib/api';
import { EQ_BANDS, clampGain } from '$lib/eq-curve';

/** The sleep timer menu, in Spotify's order. `minutes: null` is "End of track". */
export const SLEEP_OPTIONS: { label: string; minutes: number | null }[] = [
	{ label: '5 minutes', minutes: 5 },
	{ label: '10 minutes', minutes: 10 },
	{ label: '15 minutes', minutes: 15 },
	{ label: '30 minutes', minutes: 30 },
	{ label: '45 minutes', minutes: 45 },
	{ label: '1 hour', minutes: 60 },
	{ label: 'End of track', minutes: null }
];

export const audio = $state({
	ready: false,
	eq: { enabled: false, gains: EQ_BANDS.map(() => 0), preset: 'Flat', preamp: 0 } as api.EqState,
	/** Bands and the preset list, from Rust. */
	info: null as api.EqInfo | null,
	normalize: true,
	level: 'normal' as api.VolumeLevel,
	/** 0 = off (gapless, the default). */
	crossfadeSecs: 0,
	devices: { devices: [], selected: 'auto', active: 'auto' } as api.DevicesState,
	sleep: { mode: null, remaining: null, fadeSecs: 10 } as api.SleepState,
	/** Seconds left on a timed sleep timer, ticking once a second; null otherwise. */
	sleepLeft: null as number | null
});

let started = false;
let countdown: ReturnType<typeof setInterval> | undefined;

function applySleep(s: api.SleepState) {
	audio.sleep = s;
	clearInterval(countdown);
	countdown = undefined;
	const mode = s.mode;
	if (mode?.kind === 'at') {
		const tick = () => (audio.sleepLeft = Math.max(0, Math.round(mode.endsAt - Date.now() / 1000)));
		tick();
		countdown = setInterval(tick, 1000);
	} else {
		audio.sleepLeft = null;
	}
}

export async function initAudio() {
	if (started) return;
	started = true;
	const [s, info, devices] = await Promise.all([api.audioSettings(), api.audioEqInfo(), api.audioDevices()]);
	audio.eq = s.eq;
	audio.info = info;
	audio.normalize = s.normalize;
	audio.level = s.level;
	audio.crossfadeSecs = s.crossfadeSecs;
	audio.devices = devices;
	applySleep(s.sleep);
	audio.ready = true;
	await api.onAudioDevices((d) => (audio.devices = d));
	await api.onSleepTimer(applySleep);
}

// ── Equalizer ──

let dragFrame = 0;
let dragGains: number[] | null = null;

/** While a band is dragged: update the curve now, send to the player at most once a frame. */
export function dragEq(gains: number[]) {
	audio.eq.gains = gains.map(clampGain);
	audio.eq.preset = null;
	dragGains = audio.eq.gains;
	if (dragFrame) return;
	dragFrame = requestAnimationFrame(async () => {
		dragFrame = 0;
		const g = dragGains;
		dragGains = null;
		if (g) await api.setEq(audio.eq.enabled, g, false).catch(() => {});
	});
}

/** On release (or any discrete edit): apply, save, and let Rust say which preset this is. */
export async function commitEq(gains: number[] = audio.eq.gains) {
	if (dragFrame) cancelAnimationFrame(dragFrame);
	dragFrame = 0;
	dragGains = null;
	audio.eq = await api.setEq(audio.eq.enabled, gains.map(clampGain), true);
}

export async function setEqEnabled(on: boolean) {
	audio.eq.enabled = on;
	audio.eq = await api.setEq(on, audio.eq.gains, true);
}

export async function pickEqPreset(name: string) {
	audio.eq = await api.setEqPreset(name);
}

export const resetEq = () => commitEq(EQ_BANDS.map(() => 0));

// ── Levelling, crossfade ──

export async function setNormalize(on: boolean) {
	audio.normalize = on;
	await api.setNormalizeVolume(on);
}

export async function setLevel(level: api.VolumeLevel) {
	audio.level = level;
	await api.setVolumeLevel(level);
}

/** 0 turns crossfade off (gapless). Applies from the next track change. */
export async function setCrossfade(secs: number) {
	audio.crossfadeSecs = await api.setCrossfadeSecs(secs);
}

// ── Output device ──

export async function refreshDevices() {
	audio.devices = await api.audioDevices();
}

export async function pickDevice(name: string) {
	audio.devices = await api.setAudioDevice(name);
}

// ── Sleep timer ──

/** `minutes: null` = end of track (or episode). */
export async function startSleep(minutes: number | null) {
	applySleep(await api.setSleepTimer(minutes, minutes === null));
}

export async function cancelSleep() {
	applySleep(await api.cancelSleepTimer());
}

/** "12:05" for the countdown chip. */
export function formatSleepLeft(secs: number): string {
	const h = Math.floor(secs / 3600);
	const m = Math.floor((secs % 3600) / 60);
	const s = Math.floor(secs % 60);
	const mm = h ? String(m).padStart(2, '0') : String(m);
	return `${h ? `${h}:` : ''}${mm}:${String(s).padStart(2, '0')}`;
}
