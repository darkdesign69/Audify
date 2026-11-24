// DEV ONLY (lane magic-d2): drops on the queue panel and Undo of Clear queue for the browser
// preview. Mirrors state.rs `splice_queued_at` / `restore_queued_rows` on the mock player.
/* eslint-disable @typescript-eslint/no-explicit-any */
import type { SongItem } from '$lib/api';
import { player } from './state';

const manual = (s: SongItem | undefined) => !!s && (s.queued || s.queued_end);

export function magicD2Handlers(emitQueue: () => void) {
	return {
		add_to_queue_at: (a: any) => {
			const items = (a.items ?? []) as SongItem[];
			if (!items.length) return;
			const ids = new Set(items.map((s) => s.video_id));
			const cur = player.current;
			let at = a.at as number;
			// A copy waiting in the manual block moves rather than doubles.
			const goes = (s: SongItem, i: number) => i > cur && manual(s) && ids.has(s.video_id);
			at -= player.items.slice(0, at).filter(goes).length;
			player.items = player.items.filter((s, i) => !goes(s, i));
			const lo = Math.min(cur + 1, player.items.length);
			let hi = lo;
			while (manual(player.items[hi])) hi++;
			at = Math.min(Math.max(at, lo), hi);
			const next = at === lo || !!(player.items[at - 1]?.queued && !player.items[at - 1]?.queued_end);
			player.items.splice(
				at,
				0,
				...items.map((s) => ({ ...structuredClone(s), queued: next, queued_end: !next, queued_from: a.from, autoplay: false }))
			);
			emitQueue();
		},
		restore_queued: (a: any) => {
			const waiting = new Set(player.items.filter((s, i) => i > player.current && manual(s)).map((s) => s.video_id));
			const back = ((a.items ?? []) as SongItem[]).filter((s) => !waiting.has(s.video_id));
			if (!back.length || !player.items.length) return 0;
			player.items.splice(player.current + 1, 0, ...back.map((s) => structuredClone(s)));
			emitQueue();
			return back.length;
		}
	};
}
