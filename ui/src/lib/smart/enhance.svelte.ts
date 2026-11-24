/**
 * Lane magic-d3: Enhance (#13). Spotify's button on your own playlists: a recommended song after
 * every 2 of yours, each with a sparkle and add/remove buttons; off takes them all out. The
 * suggestions live in Rust per playlist (smartshuffle.rs › Enhance), which also keeps the queue in
 * step when that playlist is playing.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { SongItem } from '$lib/api';
import { LIKED_ID } from '$lib/api';
import { addToPlaylist, likeSongs } from '$lib/kit/actions';
import { toast } from '$lib/kit/toast';
export { ENHANCE_EVERY, enhanceEligible, enhanceSlots, interleave } from './chips';

export interface EnhanceState {
	playlistId: string;
	on: boolean;
	recs: SongItem[];
	at: number;
}

export const enhance = $state<{ byId: Record<string, EnhanceState>; busy: string | null }>({ byId: {}, busy: null });

function apply(e: EnhanceState | null | undefined) {
	if (!e?.playlistId) return;
	enhance.byId[e.playlistId] = { ...e, recs: e.recs ?? [] };
}

let subscribed = false;
function subscribe() {
	if (subscribed) return;
	subscribed = true;
	void listen<EnhanceState>('enhance-changed', (e) => apply(e.payload)).catch(() => {});
}

export async function loadEnhance(playlistId: string) {
	subscribe();
	try {
		apply(await invoke<EnhanceState>('enhance_state', { playlistId }));
	} catch {
		/* older backend: no Enhance */
	}
}

export const enhanced = (playlistId: string) => !!enhance.byId[playlistId]?.on;
export const enhancedRecs = (playlistId: string) => (enhance.byId[playlistId]?.on ? enhance.byId[playlistId].recs : []);

/** The Enhance button. `len`: the playlist's song count (how many suggestions it gets). */
export async function toggleEnhance(playlistId: string, len: number) {
	if (enhance.busy) return;
	subscribe();
	const on = !enhanced(playlistId);
	enhance.busy = playlistId;
	try {
		apply(await invoke<EnhanceState>('enhance_set', { playlistId, on, len }));
		if (on) toast('Suggestions mixed into this playlist');
	} catch {
		toast.error(on ? "Couldn't find suggestions for this playlist. Try again later." : "Couldn't turn off Enhance");
	} finally {
		enhance.busy = null;
	}
}

/** "Add to this playlist" on a suggestion: it becomes one of the playlist's songs. */
export async function addEnhanced(playlistId: string, title: string, song: SongItem) {
	if (playlistId === LIKED_ID) await likeSongs([song]);
	else await addToPlaylist({ kind: 'playlist', id: playlistId, title }, [song]);
	try {
		apply(await invoke<EnhanceState>('enhance_keep', { playlistId, videoId: song.video_id }));
	} catch {
		/* the next load catches up */
	}
}

/** "Remove recommendation": gone from the page and the queue, and a "not for me" signal. */
export async function removeEnhanced(playlistId: string, song: SongItem) {
	try {
		apply(await invoke<EnhanceState>('enhance_remove', { playlistId, videoId: song.video_id }));
		toast("We won't suggest that one again");
	} catch {
		toast.error("Couldn't remove that suggestion");
	}
}
