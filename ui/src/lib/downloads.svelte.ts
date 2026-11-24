// Downloads state for the UI (stream: downloads). Everything follows Rust events:
// `downloads:changed` (a track finished / a collection was added, removed or synced),
// `downloads:progress` (bytes, 3×/s while running), `downloads:network` (online / metered /
// offline mode).
//
//   import { downloads, isDownloaded, collectionState, toggleDownload } from '$lib/downloads.svelte';
//   isDownloaded(song.video_id)          // the small accent arrow in a track row
//   collectionState(id)                  // the action-bar button: none | waiting | downloading | done | failed
//   retryDownloads()                     // every failed song back in the queue
//   downloads.network.offline            // grey everything that isn't downloaded
import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import * as api from './api';
import type {
	DownloadCollection,
	DownloadCollectionStatus,
	DownloadNetwork,
	DownloadsSummary,
	SongItem
} from './api';
import { setArtResolver } from './thumb';
import { reloadSaved } from './podcasts.svelte';

export const downloads = $state({
	ready: false,
	/** Downloaded tracks and their size on disk. */
	count: 0,
	bytes: 0,
	queued: 0,
	failed: 0,
	dir: '',
	quality: 'HIGH' as 'HIGH' | 'LOW',
	network: {
		online: true,
		metered: false,
		offline: false,
		offlineMode: false,
		allowMetered: false,
		paused: false
	} as DownloadNetwork,
	/** Thumbnail URL → local artwork, used while offline. */
	art: {} as Record<string, string>
});

/** Every downloaded videoId. */
export const doneIds = new SvelteSet<string>();
/** Downloaded collections by id. */
export const collections = new SvelteMap<string, DownloadCollectionStatus>();
/** Bytes of the tracks downloading right now. */
export const progress = new SvelteMap<string, { done: number; total: number }>();

export const isDownloaded = (videoId: string | null | undefined) => !!videoId && doneIds.has(videoId);
export const isCollectionDownloaded = (id: string) => collections.has(id);

/** `failed`: nothing left in flight and some songs didn't download, so the button offers Retry. */
export type CollectionState = 'none' | 'waiting' | 'downloading' | 'done' | 'failed';

/** The button state for a collection, and how far along it is (0–1). Same rules as iOS's
 *  `DownloadStore.state`. */
export function collectionState(id: string): { state: CollectionState; fraction: number } {
	const c = collections.get(id);
	if (!c) return { state: 'none', fraction: 0 };
	if (c.total > 0 && c.done >= c.total) return { state: 'done', fraction: 1 };
	if (c.failed > 0 && c.done + c.failed >= c.total) return { state: 'failed', fraction: c.done / c.total };
	// Count bytes in flight so the ring creeps rather than jumps per track. Which running track
	// belongs to which collection isn't sent, so a share of the in-flight progress is added.
	let partial = 0;
	for (const p of progress.values()) if (p.total > 0) partial += p.done / p.total;
	const fraction = c.total ? Math.min(0.99, (c.done + Math.min(partial, c.total - c.done)) / c.total) : 0;
	if (downloads.network.paused || c.total === 0) return { state: 'waiting', fraction };
	return { state: 'downloading', fraction };
}

function applySummary(s: DownloadsSummary) {
	downloads.count = s.count;
	downloads.bytes = s.bytes;
	downloads.queued = s.queued;
	downloads.failed = s.failed;
	const seen = new Set<string>();
	for (const c of s.collections) {
		seen.add(c.id);
		collections.set(c.id, c);
	}
	for (const id of [...collections.keys()]) if (!seen.has(id) && !pendingAdds.has(id)) collections.delete(id);
}

const pendingAdds = new Set<string>();

/** Start (or remove, after Spotify's confirmation) a collection's download. */
export async function toggleDownload(
	target: DownloadCollection,
	opts: { songs?: SongItem[]; confirm?: (title: string) => Promise<boolean> } = {}
): Promise<'added' | 'removed' | 'kept'> {
	if (collections.has(target.id)) {
		if (opts.confirm && !(await opts.confirm(target.title))) return 'kept';
		collections.delete(target.id);
		await api.downloadsRemove(target.id);
		return 'removed';
	}
	// Optimistic: the ring shows at once, the backend fills the real counts in.
	pendingAdds.add(target.id);
	collections.set(target.id, { ...target, total: 0, done: 0, failed: 0, bytes: 0, addedAt: Date.now() / 1000 });
	try {
		await api.downloadsAdd(
			$state.snapshot(target) as DownloadCollection,
			opts.songs ? ($state.snapshot(opts.songs) as SongItem[]) : undefined
		);
	} catch (e) {
		collections.delete(target.id);
		throw e;
	} finally {
		pendingAdds.delete(target.id);
	}
	// The core saved a downloaded episode to Your Episodes (Spotify); show it there.
	if (target.kind === 'episode') reloadSaved().catch(() => {});
	return 'added';
}

/** Every failed song goes back in the queue (`downloads_retry`). The backend's
 *  `downloads:changed` event brings the new counts. */
export async function retryDownloads() {
	await api.downloadsRetry();
}

export async function setOfflineMode(on: boolean) {
	downloads.network.offlineMode = on;
	downloads.network.offline = on || !downloads.network.online;
	apply(await api.downloadsSetSettings({ offlineMode: on }));
}
export async function setAllowMetered(on: boolean) {
	downloads.network.allowMetered = on;
	apply(await api.downloadsSetSettings({ allowMetered: on }));
}
export async function setDownloadQuality(q: 'HIGH' | 'LOW') {
	downloads.quality = q;
	apply(await api.downloadsSetSettings({ quality: q }));
}
export async function moveDownloads(path: string) {
	apply(await api.downloadsSetDir(path));
}
export async function removeAllDownloads() {
	collections.clear();
	doneIds.clear();
	await api.downloadsRemoveAll();
}

function apply(s: api.DownloadsSnapshot) {
	applySummary(s);
	doneIds.clear();
	for (const id of s.done) doneIds.add(id);
	progress.clear();
	for (const p of s.active) progress.set(p.videoId, { done: p.done, total: p.total });
	downloads.network = s.network;
	downloads.dir = s.dir;
	downloads.art = s.art;
	downloads.quality = s.quality;
	downloads.ready = true;
}

let started = false;
let artTimer: ReturnType<typeof setTimeout> | undefined;

/** Load the state and subscribe to the events. Idempotent; called from `initApp`. */
export async function initDownloads() {
	if (started) return;
	started = true;
	// While offline, artwork comes from the files saved next to the downloads.
	setArtResolver((url) => (downloads.network.offline ? downloads.art[url] : undefined));
	await api.onDownloadsChanged((s) => {
		applySummary(s);
		for (const id of s.added) doneIds.add(id);
		for (const id of s.removed) doneIds.delete(id);
		for (const id of s.added) progress.delete(id);
		// New artwork arrives with finished tracks; refresh the map now and then, not per track.
		clearTimeout(artTimer);
		artTimer = setTimeout(() => {
			api.downloadsSnapshot()
				.then((snap) => (downloads.art = snap.art))
				.catch(() => {});
		}, 3000);
	});
	await api.onDownloadsProgress((list) => {
		const live = new Set(list.map((p) => p.videoId));
		for (const id of [...progress.keys()]) if (!live.has(id)) progress.delete(id);
		for (const p of list) progress.set(p.videoId, { done: p.done, total: p.total });
	});
	await api.onDownloadsNetwork((n) => (downloads.network = n));
	try {
		apply(await api.downloadsSnapshot());
	} catch {
		/* older backend: no downloads */
	}
}
