// The playing song's lyrics, shared by the Now Playing preview card, the lyrics view and the
// fullscreen player: one fetch per track (Limusic's `lyricsFor` dedupes in-flight requests), the
// source picker and the timing nudge write back here, and `nowLyrics` (the mini player and other
// Limusic views) is kept in step.
import * as api from '$lib/api';
import { playback } from '$lib/player.svelte';
import { durationSecs, lyricsFor, setNowLyrics } from '$lib/nowlyrics.svelte';

export type LyricsStatus = 'idle' | 'loading' | 'ready' | 'error';

export const lyr = $state({
	/** videoId the state below belongs to. */
	id: '',
	lyrics: null as api.Lyrics | null,
	status: 'idle' as LyricsStatus
});

let listening = false;

function apply(id: string, l: api.Lyrics | null) {
	if (lyr.id !== id) return;
	lyr.lyrics = l;
	lyr.status = 'ready';
	setNowLyrics(id, l);
}

function fetchFor(now: api.NowPlaying) {
	const id = now.videoId;
	lyr.id = id;
	lyr.lyrics = null;
	lyr.status = 'loading';
	lyricsFor(now)
		.then((l) => apply(id, l))
		.catch(() => {
			if (lyr.id === id) lyr.status = 'error';
		});
}

/** Call from an `$effect` in every consumer: fetches once per track. */
export function ensureLyrics(): void {
	const now = playback.now;
	if (!now) {
		lyr.id = '';
		lyr.lyrics = null;
		lyr.status = 'idle';
		return;
	}
	if (now.videoId === lyr.id && lyr.status !== 'idle') return;
	if (!listening) {
		listening = true;
		// The cache refreshed the lyrics on screen behind us: swap them in silently.
		api
			.onLyricsUpdated((id) => {
				const n = playback.now;
				if (!n || n.videoId !== id || lyr.id !== id) return;
				lyricsFor(n)
					.then((l) => apply(id, l))
					.catch(() => {});
			})
			.catch(() => {
				listening = false;
			});
	}
	fetchFor(now);
}

export function retryLyrics(): void {
	const now = playback.now;
	if (now) fetchFor(now);
}

/** What the source picker / timing nudge ask the providers about. */
export function lyricsTrack(): api.LyricsTrack | null {
	const now = playback.now;
	if (!now) return null;
	return {
		videoId: now.videoId,
		title: now.title,
		artists: now.artists,
		album: now.album ?? undefined,
		duration:
			durationSecs(now.duration) ?? (playback.duration > 0 ? playback.duration : undefined)
	};
}

/** Keep `source`'s lyrics for this song (null = back to automatic). */
export async function pickLyricsSource(source: string | null): Promise<void> {
	const track = lyricsTrack();
	if (!track) return;
	const id = track.videoId;
	lyr.status = 'loading';
	try {
		const l = await api.chooseLyricsSource({ ...track, source });
		apply(id, l);
	} catch {
		if (lyr.id === id) lyr.status = 'error';
	}
}

/** Nudge the timing by `deltaMs` (positive = lyrics later). Applied locally at once. */
export async function nudgeLyricsOffset(deltaMs: number): Promise<void> {
	const l = lyr.lyrics;
	const id = lyr.id;
	if (!l || !id) return;
	const next = Math.max(-15000, Math.min(15000, (l.offset_ms ?? 0) + deltaMs));
	lyr.lyrics = { ...l, offset_ms: next, pinned: true };
	setNowLyrics(id, lyr.lyrics);
	await api.setLyricsOffset(id, next).catch(() => {});
}

/** Index of the last synced line whose cue has passed at `posMs` (already offset-corrected). */
export function activeLine(lines: api.LyricLine[], posMs: number): number {
	let i = -1;
	for (let j = 0; j < lines.length; j++) {
		const t = lines[j].time_ms;
		if (t === undefined) continue;
		if (t > posMs) break;
		i = j;
	}
	return i;
}

/** "Lyrics provided by LRCLIB" from the provider's attribution string. */
export function attribution(l: api.Lyrics): string {
	const src = l.source.replace(/^Source:\s*/i, '').trim();
	return `Lyrics provided by ${src || l.provider}`;
}
