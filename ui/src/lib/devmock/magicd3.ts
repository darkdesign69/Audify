// DEV ONLY (lane magic-d3): Liked Songs chips and Enhance for the browser preview. Mirrors
// smart/tags.rs (library chips) and smartshuffle.rs › Enhance on the fixture pool.
/* eslint-disable @typescript-eslint/no-explicit-any */
import type { SongItem } from '$lib/api';
import { db, player, poolSlice } from './state';

const BY_ARTIST: [RegExp, string[]][] = [
	[/daft punk/i, ['Electronic', 'Dance', 'House']],
	[/radiohead/i, ['Rock', 'Alternative']],
	[/justice|cassius|stardust|modjo/i, ['Electronic', 'Dance']]
];
const FALLBACK = ['Pop', 'Hip-hop', 'R&B', 'Indie', 'Chill'];

function trackTags(s: SongItem, i: number): string[] {
	const hit = BY_ARTIST.find(([re]) => re.test(s.artists));
	const out = hit ? [...hit[1]] : [FALLBACK[i % FALLBACK.length]];
	if (i % 3 === 0) out.push('Chill');
	if (i % 4 === 1) out.push('Energetic');
	return out;
}

function likedChips() {
	const songs = db.liked.map((e) => e.song);
	const by = new Map<string, string[]>();
	songs.forEach((s, i) => {
		for (const t of trackTags(s, i)) by.set(t, [...(by.get(t) ?? []), s.video_id]);
	});
	const chips = [...by]
		.filter(([, ids]) => ids.length >= 5 && ids.length * 10 <= songs.length * 9)
		.sort((a, b) => b[1].length - a[1].length)
		.slice(0, 8)
		.map(([label, videoIds]) => ({
			label,
			kind: ['Chill', 'Sad', 'Happy', 'Energetic', 'Romantic', 'Party'].includes(label) ? 'mood' : 'genre',
			videoIds
		}));
	return { chips, done: songs.length, total: songs.length };
}

export function magicD3Handlers(emit: (event: string, payload: unknown) => void, emitQueue: () => void) {
	const state = new Map<string, any>();
	const get = (id: string) => state.get(id) ?? { playlistId: id, on: false, recs: [], at: 0 };
	const put = (e: any) => {
		state.set(e.playlistId, e);
		emit('enhance-changed', e);
		return e;
	};
	const dropFromQueue = (ids: Set<string>) => {
		player.items = player.items.filter((s, i) => i <= player.current || !ids.has(s.video_id));
		emitQueue();
	};
	return {
		smart_tags_liked: () => likedChips(),
		enhance_state: (a: any) => get(a.playlistId),
		enhance_set: (a: any) => {
			const prev = get(a.playlistId);
			dropFromQueue(new Set(prev.recs.map((s: SongItem) => s.video_id)));
			if (!a.on) return put({ playlistId: a.playlistId, on: false, recs: [], at: 0 });
			const n = Math.max(1, Math.min(50, Math.floor((a.len ?? 0) / 2)));
			const recs = poolSlice(60, n).map((s) => ({ ...s, video_id: `${s.video_id}~enh` }));
			return put({ playlistId: a.playlistId, on: true, recs, at: Math.floor(Date.now() / 1000) });
		},
		enhance_remove: (a: any) => {
			const e = get(a.playlistId);
			dropFromQueue(new Set([a.videoId]));
			return put({ ...e, recs: e.recs.filter((s: SongItem) => s.video_id !== a.videoId) });
		},
		enhance_keep: (a: any) => {
			const e = get(a.playlistId);
			return put({ ...e, recs: e.recs.filter((s: SongItem) => s.video_id !== a.videoId) });
		}
	};
}
