import type { SongItem } from '$lib/api';
import { playback } from '$lib/player.svelte';

/** The playing track as a SongItem (the queue row when it matches, else built from `now`). */
export function nowPlayingSong(): SongItem | null {
	const n = playback.now;
	if (!n) return null;
	const cur = playback.queue.items[playback.queue.currentIndex];
	if (cur?.video_id === n.videoId) return cur;
	return {
		video_id: n.videoId,
		title: n.title,
		artists: n.artists,
		artist_id: n.artistId,
		artist_runs: n.artistRuns,
		thumbnail: n.thumbnail,
		is_episode: n.isEpisode || undefined
	};
}
