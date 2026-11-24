// Dev mock of the downloads backend (stream: downloads). A fake queue "downloads" 3 tracks at a
// time at ~1 MB/s each, emitting the same events Rust does, so the rings, the accent arrows, the
// Downloaded chip and offline greying can be checked in a browser. Late Night Drive starts out
// downloaded.
import type {
	DownloadCollection,
	DownloadCollectionStatus,
	DownloadNetwork,
	SongItem
} from '$lib/api';

type Track = { song: SongItem; state: 'queued' | 'downloading' | 'done'; done: number; total: number };

export function downloadHandlers(
	emit: (event: string, payload: unknown) => void,
	listFor: (c: DownloadCollection) => SongItem[]
) {
	const tracks = new Map<string, Track>();
	const cols = new Map<string, DownloadCollection & { addedAt: number; members: string[] }>();
	const net: DownloadNetwork = {
		online: true,
		metered: false,
		offline: false,
		offlineMode: false,
		allowMetered: false,
		paused: false
	};
	let quality: 'HIGH' | 'LOW' = 'HIGH';

	const status = (): DownloadCollectionStatus[] =>
		[...cols.values()].map((c) => {
			const ts = c.members.map((id) => tracks.get(id)!).filter(Boolean);
			const done = ts.filter((t) => t.state === 'done');
			return {
				id: c.id,
				kind: c.kind,
				title: c.title,
				subtitle: c.subtitle,
				thumbnail: c.thumbnail,
				addedAt: c.addedAt,
				total: ts.length,
				done: done.length,
				failed: 0,
				bytes: done.reduce((n, t) => n + t.total, 0)
			};
		});
	const summary = () => {
		const all = [...tracks.values()];
		const done = all.filter((t) => t.state === 'done');
		return {
			collections: status(),
			count: done.length,
			bytes: done.reduce((n, t) => n + t.total, 0),
			queued: all.length - done.length,
			failed: 0
		};
	};
	const changed = (added: string[] = [], removed: string[] = []) =>
		emit('downloads:changed', { ...summary(), added, removed });

	function add(c: DownloadCollection, songs: SongItem[], instant = false) {
		const ids = [...new Set(songs.map((s) => s.video_id))];
		cols.set(c.id, { ...c, addedAt: Date.now() / 1000, members: ids });
		for (const s of songs) {
			if (tracks.has(s.video_id)) continue;
			const total = 3_000_000 + ((s.video_id.charCodeAt(0) * 7919) % 4_000_000);
			tracks.set(s.video_id, { song: s, state: instant ? 'done' : 'queued', done: instant ? total : 0, total });
		}
	}
	function prune(): string[] {
		const held = new Set([...cols.values()].flatMap((c) => c.members));
		const removed: string[] = [];
		for (const [id, t] of tracks) if (!held.has(id)) {
			if (t.state === 'done') removed.push(id);
			tracks.delete(id);
		}
		return removed;
	}

	// The fake queue.
	setInterval(() => {
		if (net.offline || (net.metered && !net.allowMetered)) return;
		const running = [...tracks.values()].filter((t) => t.state === 'downloading');
		for (const t of [...tracks.values()].filter((t) => t.state === 'queued').slice(0, 3 - running.length)) {
			t.state = 'downloading';
			running.push(t);
		}
		const finished: string[] = [];
		for (const t of running) {
			t.done = Math.min(t.total, t.done + 330_000);
			if (t.done >= t.total) {
				t.state = 'done';
				finished.push(t.song.video_id);
			}
		}
		const live = [...tracks.values()].filter((t) => t.state === 'downloading');
		if (live.length || finished.length)
			emit('downloads:progress', live.map((t) => ({ videoId: t.song.video_id, done: t.done, total: t.total })));
		if (finished.length) changed(finished);
	}, 330);

	const snapshot = () => ({
		...summary(),
		done: [...tracks.values()].filter((t) => t.state === 'done').map((t) => t.song.video_id),
		active: [...tracks.values()]
			.filter((t) => t.state === 'downloading')
			.map((t) => ({ videoId: t.song.video_id, done: t.done, total: t.total })),
		network: { ...net },
		dir: '/Users/you/Library/Application Support/app.spotless.desktop/Downloads',
		art: {},
		quality
	});
	const setNet = () => {
		net.offline = net.offlineMode || !net.online;
		net.paused = net.offline || (net.metered && !net.allowMetered);
		emit('downloads:network', { ...net });
	};

	// Seed: one playlist already downloaded.
	queueMicrotask(() => {
		const seed: DownloadCollection = { id: 'LOCALPLAYLIST:1', kind: 'playlist', title: 'Late Night Drive' };
		add(seed, listFor(seed), true);
	});
	// Poke from the console: __ENCORE_DEVMOCK__.downloads.goOffline(true)
	const control = {
		goOffline(off: boolean) {
			net.online = !off;
			setNet();
		},
		metered(on: boolean) {
			net.metered = on;
			setNet();
		}
	};

	const handlers: Record<string, (a: Record<string, any>) => unknown> = {
		downloads_snapshot: snapshot,
		downloads_add: (a) => {
			const c = a.collection as DownloadCollection;
			add(c, a.songs?.length && (c.kind === 'episode' || c.kind === 'song') ? a.songs : listFor(c));
			changed();
		},
		downloads_remove: (a) => {
			cols.delete(a.id);
			changed([], prune());
		},
		downloads_remove_all: () => {
			cols.clear();
			changed([], prune());
		},
		downloads_retry: () => changed(),
		downloads_sync: () => undefined,
		downloads_set_settings: (a) => {
			const p = a.patch ?? {};
			if (p.offlineMode !== undefined) net.offlineMode = p.offlineMode;
			if (p.allowMetered !== undefined) net.allowMetered = p.allowMetered;
			if (p.quality) quality = p.quality;
			setNet();
			return snapshot();
		},
		downloads_set_dir: (a) => ({ ...snapshot(), dir: a.path }),
		downloads_songs: () => [...tracks.values()].filter((t) => t.state === 'done').map((t) => t.song)
	};
	return { handlers, control };
}
