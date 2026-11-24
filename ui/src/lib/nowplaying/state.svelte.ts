// UI state of the playback views (stream p-nowplaying). The shell owns `shell.lyricsOpen` (the
// takeover of the main view) and `shell.rightView`; this adds which picture the takeover shows
// (Spotify's toolbar: Artwork · Video · Artist image · Lyrics) and the fullscreen lyrics toggle.
import { playback } from '$lib/player.svelte';
import { shell } from '$lib/shell/layout.svelte';

export type ExpandedMode = 'lyrics' | 'artwork' | 'artist' | 'video';

export const npui = $state({
	mode: 'lyrics' as ExpandedMode,
	/** Fullscreen player shows the lyrics over the picture. */
	fullscreenLyrics: false
});

/** Open the takeover on `mode` (the lyrics preview card's "Show lyrics", the toolbar). */
export function openExpanded(mode: ExpandedMode = 'lyrics') {
	npui.mode = mode;
	shell.lyricsOpen = true;
}

export function closeExpanded() {
	shell.lyricsOpen = false;
}

/**
 * The playback position in seconds, run forward every frame between mpv's ~4 Hz ticks while
 * `fine()` says something needs it (word-synced lyrics); otherwise just the tick. Construct during
 * component init.
 */
export class PlayClock {
	secs = $state(0);
	constructor(fine: () => boolean) {
		$effect(() => {
			const pos = playback.position;
			if (playback.paused || !fine()) {
				this.secs = pos;
				return;
			}
			const base = pos;
			const at = performance.now();
			const speed = playback.speed || 1;
			this.secs = pos;
			const tick = () => {
				this.secs = base + ((performance.now() - at) / 1000) * speed;
				frame = requestAnimationFrame(tick);
			};
			let frame = requestAnimationFrame(tick);
			return () => cancelAnimationFrame(frame);
		});
	}
}

/** "3:05" / "1:02:03". */
export function clock(secs: number): string {
	if (!secs || secs < 0 || !Number.isFinite(secs)) return '0:00';
	const t = Math.floor(secs);
	const h = Math.floor(t / 3600);
	const m = Math.floor((t % 3600) / 60);
	const s = t % 60;
	return `${h ? `${h}:${String(m).padStart(2, '0')}` : m}:${String(s).padStart(2, '0')}`;
}
