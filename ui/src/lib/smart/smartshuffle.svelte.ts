/**
 * Smart Shuffle (stream smart2): Spotify's third shuffle state. The shuffle buttons cycle
 * off → shuffle → smart shuffle → off; smart shuffle only exists in your own playlists and Liked
 * Songs (elsewhere the cycle is off ↔ shuffle). Suggested rows carry a sparkle and add/remove
 * buttons (queue panel, playlist page).
 */
import * as api from '$lib/api';
import { toast } from '$lib/kit/toast';
import { addToPlaylist, likeSongs, playback } from '$lib/kit/actions';
import type { SongItem } from '$lib/api';
import { nextMode, smartEligible, type ShuffleMode } from './shufflemode';
export { nextMode, shuffleLabel, smartEligible, type ShuffleMode } from './shufflemode';

export const smartShuffle = $state<api.SmartShuffleState & { busy: boolean }>({
	on: false,
	sourceId: null,
	recIds: [],
	busy: false
});

let started = false;
/** Subscribe once (the shell / player bar calls this). */
export function initSmartShuffle() {
	if (started) return;
	started = true;
	const apply = (s: api.SmartShuffleState) => {
		smartShuffle.on = s.on;
		smartShuffle.sourceId = s.sourceId ?? null;
		smartShuffle.recIds = s.recIds ?? [];
	};
	void api.smartShuffleState().then(apply).catch(() => {});
	void api.onSmartShuffleChanged(apply);
}

export function shuffleMode(shuffleOn: boolean, sourceId: string | null | undefined): ShuffleMode {
	if (smartShuffle.on && shuffleOn && smartShuffle.sourceId === sourceId) return 'smart';
	return shuffleOn ? 'shuffle' : 'off';
}

export const isSuggestion = (videoId: string) => smartShuffle.recIds.includes(videoId);

/** One press of the shuffle button for what is playing. */
export async function cycleShuffle(shuffleOn: boolean, sourceId: string | null | undefined) {
	if (smartShuffle.busy) return;
	const mode = shuffleMode(shuffleOn, sourceId);
	const next = nextMode(mode, smartEligible(sourceId));
	smartShuffle.busy = true;
	try {
		if (next === 'smart') {
			await api.smartShuffleSet(true);
			toast('Smart Shuffle is on');
		} else if (mode === 'smart') {
			await api.smartShuffleSet(false);
		} else {
			await api.toggleShuffle();
		}
	} catch {
		toast.error("Couldn't turn on Smart Shuffle");
	} finally {
		smartShuffle.busy = false;
	}
}

/** "Add to this playlist" on a suggestion: into the playing playlist (a like for Liked Songs);
 *  it stops being a suggestion. */
export async function addSuggestion(song: SongItem) {
	const id = playback.queue.sourceId;
	if (!id) return;
	if (id === api.LIKED_ID) await likeSongs([song]);
	else await addToPlaylist({ kind: 'playlist', id, title: playback.queue.sourceName ?? '' }, [song]);
	await api.smartShuffleKeep(song.video_id);
}

/** "Remove recommendation": out of the queue, and a "not for me" taste signal. */
export async function removeSuggestion(song: SongItem) {
	await api.smartShuffleRemove(song.video_id);
	toast("We won't suggest that one again");
}
