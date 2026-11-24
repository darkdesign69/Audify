// UI-only preferences that have no Rust setting (stream p-settings). Persisted to localStorage
// under `encore:prefs`; every window reads the same key. Rust-backed settings stay in Rust
// (`get_settings`/`set_setting`, `audio_settings`, `smart_settings`, `podcast_settings`, lib).
import { browser } from '$app/environment';

type Prefs = {
	/** Settings › Your Library › "Show Local Files": the Local files section on the Library page. */
	showLocalFiles: boolean;
	/** Settings › Display: open the Now Playing view when something starts playing. */
	nowPlayingOnPlay: boolean;
	/** The crossfade length the switch returns to (Rust stores 0 = off). */
	crossfadeSecs: number;
	/** The first-run "Bring your music from Spotify" step was shown (or the library wasn't empty). */
	onboarded: boolean;
};

const KEY = 'encore:prefs';
const DEFAULTS: Prefs = { showLocalFiles: true, nowPlayingOnPlay: true, crossfadeSecs: 5, onboarded: false };

function load(): Prefs {
	if (!browser) return { ...DEFAULTS };
	try {
		return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? '{}') };
	} catch {
		return { ...DEFAULTS };
	}
}

export const uiPrefs = $state<Prefs>(load());

export function setPref<K extends keyof Prefs>(key: K, value: Prefs[K]) {
	uiPrefs[key] = value;
	if (!browser) return;
	try {
		localStorage.setItem(KEY, JSON.stringify(uiPrefs));
	} catch {
		/* storage full / disabled: the value still holds for this session */
	}
}
