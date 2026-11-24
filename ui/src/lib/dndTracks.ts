// Lane magic-d2: the parts of a tracks drop that fetch or call the core (`dnd.ts` stays free of
// runtime imports so its self-checks run under plain node).
import { invoke } from '@tauri-apps/api/core';
import * as api from './api';
import type { SongItem } from './api';
import type { TrackDrop } from './dnd';

/** The songs behind a drop: a card is expanded to its album or playlist (one fetch, like the card
 *  menu's Add to queue). `from` names the album/playlist for the queue panel's block heading. */
export async function resolveTrackDrop(d: TrackDrop): Promise<{ songs: SongItem[]; from?: string }> {
	if ('songs' in d) return { songs: d.songs };
	const { item } = d;
	if (item.kind === 'album') return { songs: (await api.getAlbum(item.id)).items, from: item.title };
	if (item.kind === 'playlist') return { songs: (await api.getPlaylist(item.id)).items, from: item.title };
	return {
		songs: [{ video_id: item.id, title: item.title, artists: item.subtitle ?? '', thumbnail: item.thumbnail }]
	};
}

/** Insert dropped songs into the queue at the drop line (`at` = the queue index the line sits in
 *  front of; the core clamps it into "Next in queue"). */
export const addToQueueAt = (items: SongItem[], at: number, from?: string) =>
	invoke<void>('add_to_queue_at', { items, at, from });

/** Undo of Clear queue: the snapshotted hand-queued rows come back after the playing track. */
export const restoreQueued = (items: SongItem[]) => invoke<number>('restore_queued', { items });

