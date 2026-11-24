// The local library store (Stream B). Reactive Svelte 5 wrapper over Rust `library.rs`: liked
// songs, saved albums/playlists/artists, local playlists, folders, pins, recents, hidden songs and
// the private session. No account needed for any of it; SQLite is the source of truth.
//
// Every mutation is optimistic (the state changes at once, the command follows, a failure rolls it
// back) and resolves to an `Undo`: call it to apply the inverse (what an "Undo" toast does).
//
// Usage: `import { lib } from '$lib/library.svelte'`; `lib.isLiked(id)`, `await lib.like(song)`.
// `initLibrary()` is called once per window by `initApp` (player.svelte.ts).
import { browser } from '$app/environment';
import { SvelteSet } from 'svelte/reactivity';
import * as api from './api';
import type {
	BrowseItem,
	Folder,
	FolderSnapshot,
	HiddenSong,
	LibPlaylist,
	LibraryScope,
	LikedEntry,
	Pin,
	PinKind,
	PlaylistEntry,
	PrivateSession,
	RecentContext,
	RecentTrack,
	SavedItem,
	SavedKind,
	SongItem
} from './api';

/** Applies the inverse of the mutation that returned it. */
export type Undo = () => Promise<void>;
const noop: Undo = async () => {};

/** The webview's old library blob (Limusic kept saves, pins and recents in localStorage). */
const PERSONAL_KEY = 'limusic:personal';
/** A copy of it taken before anything rewrites PERSONAL_KEY, kept until Rust has imported it. */
const LEGACY_KEY = 'encore:library-legacy';

// Copy the legacy blob aside at module load, before player.svelte.ts's `savePersonal` (which no
// longer writes the library fields) can overwrite the original.
if (browser) {
	try {
		const raw = localStorage.getItem(PERSONAL_KEY);
		if (raw && !localStorage.getItem(LEGACY_KEY)) {
			const o = JSON.parse(raw) as { saved?: unknown[]; pins?: unknown[]; recent?: object };
			const has =
				(o?.saved?.length ?? 0) > 0 ||
				(o?.pins?.length ?? 0) > 0 ||
				Object.keys(o?.recent ?? {}).length > 0;
			if (has) localStorage.setItem(LEGACY_KEY, raw);
		}
	} catch {
		// Unreadable blob: nothing to carry over.
	}
}

const kindOfId = (id: string): PinKind =>
	id === api.LIKED_ID
		? 'liked'
		: id.startsWith('MPRE')
			? 'album'
			: id.startsWith('UC')
				? 'artist'
				: id.startsWith('FOLDER:')
					? 'folder'
					: id.startsWith('MPSP')
						? 'podcast'
						: 'playlist';

const now = () => Math.floor(Date.now() / 1000);

class LibraryStore {
	/** The first snapshot has loaded. */
	ready = $state(false);
	/** Liked video ids: O(1) `isLiked`. */
	readonly liked = new SvelteSet<string>();
	/** Video ids hidden everywhere. */
	readonly hidden = new SvelteSet<string>();
	/** The user's own playlists, most recently changed first. */
	playlists = $state<LibPlaylist[]>([]);
	folders = $state<Folder[]>([]);
	/** At most four, in order. */
	pins = $state<Pin[]>([]);
	/** Saved albums, YouTube playlists and followed artists, newest first. */
	saved = $state<SavedItem[]>([]);
	/** item id → unix seconds it was last played from (the Recents sort). */
	lastPlayed = $state<Record<string, number>>({});
	/** stream: magic-d4 — item id → unix seconds it was last added to or edited (a like, for
	 *  Liked Songs). Recents sorts by the later of this and `lastPlayed`, like Spotify. */
	lastTouched = $state<Record<string, number>>({});
	/** Recently played contexts with their cards, newest first (loaded with the snapshot). */
	recent = $state<RecentContext[]>([]);
	privateSession = $state<PrivateSession>({ enabled: false });

	savedIds = $derived(new Set(this.saved.map((s) => s.id)));
	pinIds = $derived(this.pins.map((p) => p.itemId));
	likedCount = $derived(this.liked.size);

	// ── reads ────────────────────────────────────────────────────────────────────────────────

	/** stream: magic-d4 — the Recents sort key: last played from, added to or edited (secs). */
	recentAt = (id: string) => Math.max(this.lastPlayed[id] ?? 0, this.lastTouched[id] ?? 0);

	isLiked = (videoId: string) => this.liked.has(videoId);
	isHidden = (videoId: string) => this.hidden.has(videoId);
	isSaved = (id: string) => this.savedIds.has(id);
	isSynced = (id: string) => this.saved.some((s) => s.id === id && s.synced);
	isPinned = (id: string) => this.pinIds.includes(id);
	savedOf = (kind: SavedKind) => this.saved.filter((s) => s.kind === kind);
	playlist = (id: string) => this.playlists.find((p) => p.id === id);
	folder = (id: string) => this.folders.find((f) => f.id === id);
	/** Folders directly under `parentId` (null = root), in order. */
	childFolders = (parentId: string | null) =>
		this.folders
			.filter((f) => (f.parentId ?? null) === parentId)
			.sort((a, b) => a.position - b.position);
	/** The folder an item (playlist) sits in, if any. */
	folderOf = (itemId: string) => this.folders.find((f) => f.items.includes(itemId));

	// ── loading ──────────────────────────────────────────────────────────────────────────────

	async reload(scope: LibraryScope = 'all') {
		if (scope === 'all') {
			const s = await api.libSnapshot();
			replace(this.liked, s.likedIds);
			replace(this.hidden, s.hiddenSongIds);
			this.playlists = s.playlists;
			this.folders = s.folders;
			this.pins = s.pins;
			this.saved = s.saved;
			this.lastPlayed = s.lastPlayed;
			this.lastTouched = s.lastTouched ?? {};
			this.privateSession = s.privateSession;
			this.recent = await api.libRecentContexts(100).catch(() => this.recent);
			this.ready = true;
			return;
		}
		switch (scope) {
			case 'liked':
				return replace(this.liked, await api.libLikedIds());
			case 'hidden':
				return replace(
					this.hidden,
					(await api.libHiddenSongs()).filter((h) => !h.contextId).map((h) => h.videoId)
				);
			case 'saved':
				this.saved = await api.libSaved();
				return;
			case 'playlists':
				this.playlists = await api.libPlaylists();
				return;
			case 'folders':
				this.folders = await api.libFolders();
				this.playlists = await api.libPlaylists(); // folderId lives on the playlist rows too
				return;
			case 'pins':
				this.pins = await api.libPins();
				return;
			case 'played':
				this.lastPlayed = await api.libLastPlayed();
				this.recent = await api.libRecentContexts(100);
				return;
			case 'private':
				this.privateSession = await api.libPrivateSession();
				return;
		}
	}

	// ── likes ────────────────────────────────────────────────────────────────────────────────

	/** Like one or more songs. Undo unlikes the ones this call newly liked. */
	async like(songs: SongItem | SongItem[]): Promise<Undo> {
		const list = (Array.isArray(songs) ? songs : [songs]).filter((s) => s.video_id);
		const fresh = list.filter((s) => !this.liked.has(s.video_id));
		for (const s of fresh) this.liked.add(s.video_id);
		if (fresh.length) this.touch(api.LIKED_ID);
		try {
			await api.libLike(list);
		} catch (e) {
			for (const s of fresh) this.liked.delete(s.video_id);
			throw e;
		}
		const ids = fresh.map((s) => s.video_id);
		return ids.length ? async () => void (await this.unlike(ids)) : noop;
	}

	/** Unlike. Undo puts them back with their original dates. */
	async unlike(videoIds: string | string[]): Promise<Undo> {
		const ids = (Array.isArray(videoIds) ? videoIds : [videoIds]).filter((id) =>
			this.liked.has(id)
		);
		for (const id of ids) this.liked.delete(id);
		let removed: LikedEntry[];
		try {
			removed = await api.libUnlike(ids);
		} catch (e) {
			for (const id of ids) this.liked.add(id);
			throw e;
		}
		return removed.length ? () => this.restoreLikes(removed) : noop;
	}

	async restoreLikes(entries: LikedEntry[]) {
		for (const e of entries) this.liked.add(e.song.video_id);
		await api.libRestoreLikes(entries);
	}

	/** Answers the new state and its undo. */
	async toggleLike(song: SongItem): Promise<{ liked: boolean; undo: Undo }> {
		if (this.isLiked(song.video_id))
			return { liked: false, undo: await this.unlike(song.video_id) };
		return { liked: true, undo: await this.like(song) };
	}

	likedSongs = () => api.libLikedSongs();

	// ── saved albums / playlists / artists ───────────────────────────────────────────────────

	/** Save a card. Undo unsaves it (if this call saved it). */
	async save(card: BrowseItem, synced = false): Promise<Undo> {
		if (this.isSaved(card.id)) {
			if (synced) await this.markSynced([card.id]);
			return noop;
		}
		const item: SavedItem = {
			id: card.id,
			kind: card.kind as SavedKind,
			title: card.title,
			subtitle: card.subtitle,
			thumbnail: card.thumbnail,
			card: { ...card },
			synced,
			addedAt: now()
		};
		this.saved = [item, ...this.saved];
		try {
			await api.libSave(card, synced);
		} catch (e) {
			this.saved = this.saved.filter((s) => s !== item);
			throw e;
		}
		return async () => void (await this.unsave(card.id));
	}

	/** Unsave. Undo restores the row exactly (date, synced flag). */
	async unsave(id: string): Promise<Undo> {
		const before = this.saved;
		if (!before.some((s) => s.id === id)) return noop;
		this.saved = before.filter((s) => s.id !== id);
		let removed: SavedItem | null;
		try {
			removed = await api.libUnsave(id);
		} catch (e) {
			this.saved = before;
			throw e;
		}
		return removed ? () => this.restoreSaved(removed) : noop;
	}

	async restoreSaved(item: SavedItem) {
		this.saved = [item, ...this.saved.filter((s) => s.id !== item.id)].sort(
			(a, b) => b.addedAt - a.addedAt
		);
		await api.libRestoreSaved(item);
	}

	async markSynced(ids: string[], synced = true) {
		const set = new Set(ids);
		this.saved = this.saved.map((s) => (set.has(s.id) ? { ...s, synced } : s));
		await api.libMarkSynced(ids, synced);
	}

	// ── playlists ────────────────────────────────────────────────────────────────────────────

	async createPlaylist(name?: string, description?: string, folderId?: string) {
		const p = await api.libPlaylistCreate(name, description, folderId);
		this.playlists = [p, ...this.playlists];
		if (folderId) this.folders = await api.libFolders();
		return p;
	}

	/** Rename / re-describe. Undo puts the old values back. */
	async updatePlaylist(id: string, patch: { name?: string; description?: string }): Promise<Undo> {
		const old = this.playlist(id);
		const p = await api.libPlaylistUpdate(id, patch.name, patch.description);
		this.playlists = this.playlists.map((x) => (x.id === id ? p : x));
		this.touch(id);
		if (!old) return noop;
		return async () => {
			const back = await api.libPlaylistUpdate(id, old.name, old.description);
			this.playlists = this.playlists.map((x) => (x.id === id ? back : x));
		};
	}

	/** Soft delete. Undo restores it (and Settings › Recover playlists, for 30 days). */
	async deletePlaylist(id: string): Promise<Undo> {
		const before = this.playlists;
		this.playlists = before.filter((p) => p.id !== id);
		try {
			await api.libPlaylistDelete(id);
		} catch (e) {
			this.playlists = before;
			throw e;
		}
		return () => this.restorePlaylist(id);
	}

	async restorePlaylist(id: string) {
		const p = await api.libPlaylistRestore(id);
		this.playlists = [p, ...this.playlists.filter((x) => x.id !== id)].sort(
			(a, b) => b.updatedAt - a.updatedAt
		);
		this.touch(id);
	}

	/** stream: magic-d4 — deleted playlists still recoverable (Settings › Recover playlists). */
	deletedPlaylists = () => api.libDeletedPlaylists();

	entries = (id: string) => api.libPlaylistEntries(id);
	/** Which of these the playlist already holds: ask before adding, to warn. */
	duplicates = (id: string, videoIds: string[]) => api.libPlaylistDuplicates(id, videoIds);

	/**
	 * Add songs. Without `allowDuplicates`, ones already there are skipped and reported in
	 * `duplicates`. Undo removes exactly the rows added.
	 */
	async addToPlaylist(
		id: string,
		songs: SongItem[],
		opts: { allowDuplicates?: boolean; position?: number } = {}
	): Promise<{ added: PlaylistEntry[]; duplicates: string[]; undo: Undo }> {
		const r = await api.libPlaylistAdd(id, songs, opts.allowDuplicates ?? false, opts.position);
		this.bumpCount(id, r.added.length);
		const rows = r.added.map((e) => e.rowId);
		return {
			...r,
			undo: rows.length ? async () => void (await this.removeFromPlaylist(id, rows)) : noop
		};
	}

	/** Remove rows. Undo re-inserts them at their old positions with their old dates. */
	async removeFromPlaylist(
		id: string,
		rowIds: number[]
	): Promise<{ removed: PlaylistEntry[]; undo: Undo }> {
		const removed = await api.libPlaylistRemove(id, rowIds);
		this.bumpCount(id, -removed.length);
		return {
			removed,
			undo: removed.length
				? async () => {
						await api.libPlaylistInsertEntries(id, removed);
						this.bumpCount(id, removed.length);
					}
				: noop
		};
	}

	/** Move rows before `before` (undefined = the end). Undo restores the previous order. */
	async moveInPlaylist(id: string, rowIds: number[], before?: number): Promise<Undo> {
		const prev = await api.libPlaylistMove(id, rowIds, before);
		this.touch(id);
		return async () => void (await api.libPlaylistSetOrder(id, prev));
	}

	/** Save a whole order (e.g. a sort made permanent). Undo restores the previous one. */
	async setPlaylistOrder(id: string, rowIds: number[]): Promise<Undo> {
		const prev = await api.libPlaylistSetOrder(id, rowIds);
		this.touch(id);
		return async () => void (await api.libPlaylistSetOrder(id, prev));
	}

	private bumpCount(id: string, delta: number) {
		if (!delta) return;
		this.touch(id);
		this.playlists = this.playlists.map((p) =>
			p.id === id ? { ...p, count: Math.max(0, p.count + delta), updatedAt: now() } : p
		);
	}

	// ── folders ──────────────────────────────────────────────────────────────────────────────

	async createFolder(name?: string, parentId?: string): Promise<Folder> {
		const f = await api.libFolderCreate(name, parentId);
		this.folders = [...this.folders, f];
		return f;
	}

	async renameFolder(id: string, name: string): Promise<Undo> {
		const old = await api.libFolderRename(id, name);
		this.folders = this.folders.map((f) => (f.id === id ? { ...f, name } : f));
		return async () => {
			await api.libFolderRename(id, old);
			this.folders = this.folders.map((f) => (f.id === id ? { ...f, name: old } : f));
		};
	}

	/** Move a folder under `parentId` (null = root) at `index`. Rejects with 'folder_cycle'. */
	async moveFolder(id: string, parentId: string | null, index?: number): Promise<Undo> {
		const prev = await api.libFolderMove(id, parentId, index);
		this.folders = await api.libFolders();
		return async () => {
			await api.libFolderMove(id, prev.folderId ?? null, prev.index);
			this.folders = await api.libFolders();
		};
	}

	/** Delete a folder; its contents move up a level. Undo rebuilds it. */
	async deleteFolder(id: string): Promise<Undo> {
		const snap: FolderSnapshot = await api.libFolderDelete(id);
		await this.reload('folders');
		this.pins = this.pins.filter((p) => p.itemId !== id);
		return async () => {
			await api.libFolderRestore(snap);
			await this.reload('folders');
		};
	}

	/** Put a playlist in a folder (null = root) at `index`. Undo moves it back. */
	async setItemFolder(itemId: string, folderId: string | null, index?: number): Promise<Undo> {
		const prev = await api.libFolderSetItem(itemId, folderId, index);
		await this.reload('folders');
		return async () => {
			await api.libFolderSetItem(itemId, prev?.folderId ?? null, prev?.index);
			await this.reload('folders');
		};
	}

	// ── pins ─────────────────────────────────────────────────────────────────────────────────

	/** Pin (max four). Rejects with 'pins_full'. Undo unpins. */
	async pin(itemId: string, kind?: PinKind, index?: number): Promise<Undo> {
		const was = this.isPinned(itemId);
		if (!was && this.pins.length >= api.MAX_PINS) throw new Error('pins_full');
		this.pins = await api.libPin(itemId, kind ?? kindOfId(itemId), index);
		return was ? noop : async () => void (await this.unpin(itemId));
	}

	/** Unpin. Undo re-pins at the same spot. */
	async unpin(itemId: string): Promise<Undo> {
		const pin = this.pins.find((p) => p.itemId === itemId);
		if (!pin) return noop;
		this.pins = this.pins.filter((p) => p.itemId !== itemId);
		const at = await api.libUnpin(itemId);
		return async () => {
			this.pins = await api.libPin(itemId, pin.kind, at ?? undefined);
		};
	}

	/** The sidebar's pin toggle. Sync answer (optimistic), the write follows. */
	togglePin(itemId: string, kind?: PinKind): 'pinned' | 'unpinned' | 'full' {
		if (this.isPinned(itemId)) {
			this.unpin(itemId).catch(() => this.reload('pins'));
			return 'unpinned';
		}
		if (this.pins.length >= api.MAX_PINS) return 'full';
		const k = kind ?? kindOfId(itemId);
		this.pins = [...this.pins, { itemId, kind: k, position: this.pins.length }];
		api.libPin(itemId, k)
			.then((p) => (this.pins = p))
			.catch(() => this.reload('pins'));
		return 'pinned';
	}

	async reorderPins(itemIds: string[]): Promise<Undo> {
		const prev = this.pinIds.slice();
		this.pins = await api.libPinsReorder(itemIds);
		return async () => void (this.pins = await api.libPinsReorder(prev));
	}

	// ── recents ──────────────────────────────────────────────────────────────────────────────

	/** "Played from this" (a playlist, album, artist, Liked Songs). Fire and forget. */
	notePlayed(card: BrowseItem) {
		const at = now();
		this.lastPlayed = { ...this.lastPlayed, [card.id]: at };
		const entry: RecentContext = { itemId: card.id, kind: card.kind, card, playedAt: at };
		this.recent = [entry, ...this.recent.filter((r) => r.itemId !== card.id)].slice(0, 100);
		api.libNotePlayed(card.id, card.kind, card).catch(() => {});
	}

	/** stream: magic-d4 — "added to / edited now": Recents moves the item to the top at once (the
	 *  core stamps it too and echoes `library-changed` `touched` to every window). */
	touch(id: string, at = now()) {
		if ((this.lastTouched[id] ?? 0) >= at) return;
		this.lastTouched = { ...this.lastTouched, [id]: at };
	}

	recentContexts = (limit = 50) => api.libRecentContexts(limit);
	/** Recently played songs (local listens), each once, newest first. */
	recentTracks = (limit = 50): Promise<RecentTrack[]> => api.libRecentTracks(limit);

	// ── hidden songs (hidden artists: player.svelte.ts `blockArtist`) ────────────────────────

	async hideSong(song: SongItem, contextId?: string): Promise<Undo> {
		if (!contextId) this.hidden.add(song.video_id);
		const added = await api.libHideSong(song.video_id, song, contextId);
		return added ? () => this.unhideSong(song.video_id, contextId).then(() => {}) : noop;
	}

	async unhideSong(videoId: string, contextId?: string): Promise<HiddenSong | null> {
		if (!contextId) this.hidden.delete(videoId);
		return api.libUnhideSong(videoId, contextId);
	}

	hiddenSongs = (contextId?: string) => api.libHiddenSongs(contextId);

	// ── private session ──────────────────────────────────────────────────────────────────────

	async setPrivateSession(enabled: boolean) {
		this.privateSession = await api.libSetPrivateSession(enabled);
	}

	// ── export ───────────────────────────────────────────────────────────────────────────────

	exportLibrary = (path?: string) => api.libExport(path);
}

function replace(set: SvelteSet<string>, ids: string[]) {
	const next = new Set(ids);
	for (const id of set) if (!next.has(id)) set.delete(id);
	for (const id of ids) set.add(id);
}

export const lib = new LibraryStore();

let started = false;

/**
 * Load the library, import the old localStorage library once, and follow `library-changed` from
 * every window. Idempotent. Answers the migration report when one ran this time.
 */
export async function initLibrary(): Promise<api.MigrationReport | null> {
	if (started || !browser) return null;
	started = true;
	let report: api.MigrationReport | null = null;
	try {
		const raw = localStorage.getItem(LEGACY_KEY);
		if (raw) {
			report = await api.libMigrateLegacy(JSON.parse(raw));
			localStorage.removeItem(LEGACY_KEY);
		}
	} catch {
		// Kept for the next launch: the blob stays under LEGACY_KEY until Rust takes it.
	}
	await api.onLibraryChanged((scope) => lib.reload(scope).catch(() => {}));
	await api.onLibraryTouched((id, at) => lib.touch(id, at)); // stream: magic-d4
	await lib.reload('all').catch(() => {});
	return report?.already ? null : report;
}
