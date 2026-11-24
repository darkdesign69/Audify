// DEV ONLY (stream onboard): the Spotify import for the browser preview, so every step of the
// onboarding and the import dialog can be looked at. Mirrors the shape of src-tauri import.rs:
// read → start (matching, `import-progress` events) → review (rows, picks) → create → done.
/* eslint-disable @typescript-eslint/no-explicit-any */
import type { ImportPreview, ImportRow, ImportSnapshot, SongItem } from '$lib/api';
import { pool } from './state';

type Emit = (event: string, payload: unknown) => void;

const LISTS = [
	{ kind: 'liked', name: 'Liked Songs', count: 212 },
	{ kind: 'playlist', name: 'Running 2026', count: 20 },
	{ kind: 'playlist', name: 'Chill Evenings', count: 29 },
	{ kind: 'playlist', name: 'Throwbacks', count: 38 },
	{ kind: 'playlist', name: 'Dinner Jazz', count: 56 }
] as const;

export function importHandlers(emit: Emit): Record<string, (a: any) => unknown> {
	let preview: ImportPreview | null = null;
	let snap: ImportSnapshot | null = null;
	let rows: ImportRow[] = [];
	let timer: ReturnType<typeof setInterval> | null = null;
	const cover = (i: number) => pool[i % pool.length]?.thumbnail ?? null;
	const send = () => snap && emit('import-progress', structuredClone(snap));
	const counts = () => {
		if (!snap) return;
		snap.matched = rows.filter((r) => r.tier === 'matched').length;
		snap.check = rows.filter((r) => r.tier === 'check').length;
		snap.missing = rows.filter((r) => r.tier === 'missing').length;
	};

	function readPreview(single: boolean): ImportPreview {
		const lists = single
			? [{ kind: 'playlist' as const, name: 'Road Trip Classics', owner: 'alex', cover: cover(3), count: 48, skipped: 0, truncated: false }]
			: LISTS.map((l, i) => ({ ...l, owner: null, cover: l.kind === 'liked' ? null : cover(i * 5), skipped: 0, truncated: false }));
		return single ? { lists } : { lists, albums: 12, artists: 31, history: { plays: 18402, songs: 3211, from: 1730000000, to: 1760000000 } };
	}

	function buildRows(total: number) {
		rows = Array.from({ length: total }, (_, i) => {
			const s: SongItem = structuredClone(pool[i % pool.length]);
			const key = `sp${i}`;
			if (i % 25 === 3) {
				const odd = { ...s, video_id: s.video_id + '-x', title: s.title + ' (Live)' };
				return { key, title: s.title, artists: s.artists, album: s.album, durationMs: 215000, tier: 'check', pick: odd, candidates: [odd, s, structuredClone(pool[(i + 7) % pool.length])] };
			}
			if (i % 40 === 7) return { key, title: `Unreleased Demo ${i}`, artists: 'Nobody Known', durationMs: 151000, tier: 'missing', pick: null, candidates: [] };
			return { key, title: s.title, artists: s.artists, album: s.album, durationMs: 200000, tier: 'matched', pick: s, candidates: [] };
		}) as ImportRow[];
	}

	return {
		import_read: (a) => {
			if (a.link && !/spotify/.test(a.link)) throw 'not_spotify';
			preview = readPreview(!!a.link && /playlist|album/.test(a.link));
			return structuredClone(preview);
		},
		import_read_file: () => (preview = readPreview(false)),
		import_start: (a) => {
			const lists = (a.lists as number[]).map((i) => preview!.lists[i]);
			const total = lists.reduce((n, l) => n + l.count, 0);
			buildRows(total);
			snap = {
				phase: 'matching', total, done: 0, matched: 0, check: 0, missing: 0,
				lists: lists.map((l) => ({ kind: l.kind, name: l.name, count: l.count, cover: l.cover })),
				recent: [], step: [0, 0], results: []
			};
			timer && clearInterval(timer);
			timer = setInterval(() => {
				if (!snap) return;
				snap.done = Math.min(snap.total, snap.done + Math.ceil(snap.total / 40));
				const r = rows[snap.done - 1];
				snap.recent = r ? [{ title: r.title, artists: r.artists, tier: r.tier, thumbnail: r.pick?.thumbnail }] : [];
				if (snap.done >= snap.total) {
					clearInterval(timer!);
					counts();
					snap.phase = 'review';
				}
				send();
			}, 200);
			return structuredClone(snap);
		},
		import_status: () => (snap ? structuredClone(snap) : null),
		import_rows: (a) => rows.filter((r) => r.tier === a.tier).map((r) => (a.tier === 'matched' ? { ...r, candidates: [] } : r)),
		import_pick: (a) => {
			const r = rows.find((x) => x.key === a.key);
			if (!snap || !r) throw 'gone';
			r.pick = a.song;
			r.tier = a.song ? 'matched' : 'missing';
			counts();
			return structuredClone(snap);
		},
		import_create: () => {
			if (!snap) throw 'gone';
			snap.phase = 'creating';
			snap.step = [0, snap.lists.length];
			send();
			const s = snap;
			const tick = setInterval(() => {
				s.step = [s.step[0] + 1, s.step[1]];
				if (s.step[0] >= s.step[1]) {
					clearInterval(tick);
					s.phase = 'done';
					s.results = s.lists.map((l, i) => ({
						kind: l.kind, name: l.name, id: l.kind === 'liked' ? 'VLLM' : `LOCALPLAYLIST:${i + 1}`, local: true,
						added: l.count - (i === 1 ? 2 : 0), missing: i === 1 ? 2 : 0, removed: 0, updated: i === 2
					}));
				}
				send();
			}, 400);
			return null;
		},
		import_cancel: () => {
			timer && clearInterval(timer);
			if (snap && snap.phase === 'matching') snap.phase = 'cancelled';
			else snap = null;
			send();
			return null;
		}
	};
}
