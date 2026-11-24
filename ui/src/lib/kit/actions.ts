// ─────────────────────────────────────────────────────────────────────────────────────────────
// The kit's ONE adapter onto app data and actions. Every kit component, menu definition and the
// library sidebar call these functions and nothing else in player.svelte.ts / api.ts for writes.
//
// Library writes go to the local library store `lib` ($lib/library.svelte, Stream B): every
// mutation is optimistic and returns an Undo, which the toasts here offer. Playback/queue goes
// through `$lib/player.svelte`. YouTube-account playlists fall back to the api.ts commands.
// ─────────────────────────────────────────────────────────────────────────────────────────────
import { goto } from '$app/navigation';
import { page } from '$app/state';
import * as api from '$lib/api';
import type { BrowseItem, SongItem } from '$lib/api';
import { displayRuns } from '$lib/artistruns';
import {
	addSongsToPlaylist,
	addToLibrary,
	blockArtist,
	enqueue,
	forgetPlaylist,
	inLibrary,
	library,
	loadLibrary,
	openNewPlaylist,
	ownedByUser,
	personal,
	playback,
	playFrom,
	playSingleFrom,
	removeFromLibrary,
	startRadio as playerStartRadio,
	ui
} from '$lib/player.svelte';
import { matchFresh, restoreMoves } from '$lib/restoreorder';
import type { RadioKind } from '$lib/trackapi';
import { copyText } from '$lib/clipboard';
import { lib } from '$lib/library.svelte';
import { toast, toastUndo } from './toast';

export { playback };

// ── Songs: likes ────────────────────────────────────────────────────────────────────────────

/** Is this song in Liked Songs. Reactive. */
export const isLiked = (song: SongItem): boolean => lib.isLiked(song.video_id);

/** Toggle like with Spotify's toasts ("Added to Liked Songs" / "Removed from Liked Songs" + Undo). */
export async function toggleLike(song: SongItem): Promise<void> {
	try {
		const { liked, undo } = await lib.toggleLike(song);
		// Spotify: "Added to Liked Songs · Change", one click from liking to filing it in playlists.
		if (liked) toastUndo('Added to Liked Songs', () => openSavedInPicker(song), 'Change', {
			secondary: { label: 'Undo', onClick: () => void Promise.resolve(undo()).catch((e) => toast.fail(e, "Couldn't undo that")) }
		});
		else toastUndo('Removed from Liked Songs', undo);
	} catch (e) {
		toast.fail(e, "Couldn't update Liked Songs");
	}
}

/** The "Saved in" picker (the accent check's), opened from the like toast's "Change": anchored
 *  where the toast sat, bottom centre above the player bar. */
export async function openSavedInPicker(song: SongItem) {
	const { openMenu } = await import('./menus/state.svelte');
	const x = Math.round(window.innerWidth / 2);
	const y = Math.max(120, window.innerHeight - 140);
	openMenu({ x, y }, [{ type: 'saved-in', song }], { side: 'top', align: 'center', width: 338 });
}

export async function likeSongs(songs: SongItem[]): Promise<void> {
	const todo = songs.filter((s) => !isLiked(s));
	if (!todo.length) return;
	try {
		toastUndo('Added to Liked Songs', await lib.like(todo));
	} catch (e) {
		toast.fail(e, "Couldn't update Liked Songs");
	}
}

export async function unlikeSongs(songs: SongItem[]): Promise<void> {
	const todo = songs.filter((s) => isLiked(s)).map((s) => s.video_id);
	if (!todo.length) return;
	try {
		toastUndo('Removed from Liked Songs', await lib.unlike(todo));
	} catch (e) {
		toast.fail(e, "Couldn't update Liked Songs");
	}
}

// ── Songs: play & queue ─────────────────────────────────────────────────────────────────────

/** Play one song on its own (a search result, a Home card): Spotify treats the list it sat in as
 *  "what you might have meant", not a queue, so only Autoplay follows it. */
export function playSingle(song: SongItem, from?: { search?: string }) {
	// ── lane magic-d1 ── a pick on the search page plays "from search" with its query, so the
	// right panel can say so (Spotify's "Playing from search"). Read here so the search page's
	// own call sites stay as they are.
	const search =
		from?.search ??
		(page.url?.pathname?.startsWith('/search') ? (page.url.searchParams.get('q') ?? undefined) : undefined);
	return playSingleFrom(song, { search });
}

/** Play `songs` as a context starting at `start` (double-click on a row). */
export function playTracks(
	songs: SongItem[],
	start: number,
	context?: PlayTracksContext,
	shuffle?: boolean,
	continuation?: string
) {
	if (!songs.length) return;
	if (!context?.id) return playSingle(songs[start]);
	const source: BrowseItem = {
		kind: context.kind ?? 'playlist',
		id: context.id,
		title: context.title ?? '',
		subtitle: context.subtitle,
		thumbnail: context.thumbnail
	};
	return playFrom(source, songs, start, context.sourceId ?? context.id, shuffle, continuation);
}

/** What a list plays as (Recents/Home context card). `sourceId` overrides the queue's source id
 *  (an album page plays its `OLAK…` playlist so autoplay continues with the album radio). */
export type PlayTracksContext = {
	id?: string;
	title?: string;
	kind?: BrowseItem['kind'];
	subtitle?: string;
	thumbnail?: string;
	sourceId?: string;
};

export const playNext = (songs: SongItem[], from?: string) => enqueue(songs, true, from);
export const addToQueue = (songs: SongItem[], from?: string) => enqueue(songs, false, from);

export async function removeFromQueue(index: number) {
	try {
		await api.removeFromQueue(index);
	} catch (e) {
		toast.fail(e, "Couldn't remove that from the queue");
	}
}

/** Start a radio at once, replacing the queue (the radio page's Play, Home radio cards). */
export const startRadio = playerStartRadio;

// ── lane magic-d1 ── "Go to … radio" opens the radio as a page (MAGIC-GAPS #1) ─────────────────
// Spotify opens "<Song> Radio" as a playlist you can browse; nothing plays until you press Play.

export function radioHref(kind: RadioKind, id: string, name?: string, thumb?: string | null): string {
	const q = new URLSearchParams();
	if (name) q.set('name', name);
	if (thumb) q.set('thumb', thumb);
	const qs = q.toString();
	return `/radio/${kind}/${encodeURIComponent(id)}${qs ? `?${qs}` : ''}`;
}

export const goRadio = (kind: RadioKind, id: string, name?: string, thumb?: string | null) =>
	goto(radioHref(kind, id, name, thumb));

/** "Go to song radio": the radio page, the music keeps playing. */
export const songRadio = (song: SongItem) => goRadio('song', song.video_id, song.title, song.thumbnail);

// ── Navigation helpers ───────────────────────────────────────────────────────────────────────

export const artistHref = (id: string) => `/artist/${encodeURIComponent(id)}`;
export const albumHref = (id: string) => `/album/${encodeURIComponent(id)}`;
export const playlistHref = (id: string) => `/playlist/${encodeURIComponent(id)}`;
export function hrefFor(item: Pick<BrowseItem, 'kind' | 'id'>): string {
	if (item.kind === 'artist') return artistHref(item.id);
	if (item.kind === 'album') return albumHref(item.id);
	if (item.kind === 'playlist') return playlistHref(item.id);
	return '/';
}
export const goArtist = (id: string) => goto(artistHref(id));
export const goAlbum = (id: string) => goto(albumHref(id));
export const goPlaylist = (id: string) => goto(playlistHref(id));

/** Artists a song credits, with ids where known (for "Go to artist ▸"). */
export function songArtists(song: SongItem): { id?: string; name: string }[] {
	return displayRuns(song.artist_runs, song.artists, song.artist_id).map((r) => ({
		id: r.id ?? undefined,
		name: r.text
	}));
}

// ── Share ────────────────────────────────────────────────────────────────────────────────────

export function shareUrl(item: { kind: BrowseItem['kind'] | 'song'; id: string }): string {
	switch (item.kind) {
		case 'song':
			return `https://music.youtube.com/watch?v=${item.id}`;
		case 'artist':
			return `https://music.youtube.com/channel/${item.id}`;
		case 'album':
			return `https://music.youtube.com/browse/${item.id}`;
		default:
			return `https://music.youtube.com/playlist?list=${item.id.replace(/^VL/, '')}`;
	}
}

export async function copyLink(item: { kind: BrowseItem['kind'] | 'song'; id: string }) {
	try {
		await copyText(shareUrl(item));
		toast('Link copied to clipboard');
	} catch (e) {
		toast.fail(e, "Couldn't copy the link");
	}
}

// ── Playlists ────────────────────────────────────────────────────────────────────────────────

/** The user's own (local) playlists as cards, most recently changed first. Reactive. */
export function ownPlaylists(): BrowseItem[] {
	return lib.playlists.map((p) => ({
		kind: 'playlist' as const,
		id: p.id,
		title: p.name,
		subtitle: 'Playlist',
		thumbnail: p.cover ?? p.thumbnail ?? undefined
	}));
}

/** Own playlists + saved YouTube playlists. Reactive. */
export function allPlaylists(): BrowseItem[] {
	return [...ownPlaylists(), ...lib.savedOf('playlist').map((s) => s.card)];
}

/** Playlists the user can add songs to ("Add to playlist ▸"), most recent first. Reactive. */
export function editablePlaylists(): BrowseItem[] {
	const yt = library.items.filter(
		(p) => p.kind === 'playlist' && p.id !== api.LIKED_MUSIC_ID && !api.isLocalPlaylist(p.id) && ownedByUser(p)
	);
	return [...ownPlaylists(), ...yt];
}

export const isOwnPlaylist = (id: string): boolean => {
	if (api.isLocalPlaylist(id)) return true;
	const p = library.items.find((i) => i.id === id);
	return !!p && ownedByUser(p);
};

/** Add songs; on duplicates asks "Already added" (Add anyway / Don't add) like Spotify. */
export async function addToPlaylist(target: BrowseItem, songs: SongItem[]) {
	if (!songs.length) return;
	if (!api.isLocalPlaylist(target.id)) return addSongsToPlaylist(target, songs);
	try {
		const r = await lib.addToPlaylist(target.id, songs);
		if (r.duplicates.length) {
			const { confirmDialog } = await import('./confirm.svelte');
			const anyway = await confirmDialog({
				title: 'Already added',
				body:
					r.duplicates.length === 1 && songs.length === 1
						? `This is already in your ${target.title} playlist.`
						: `Some of these are already in your ${target.title} playlist.`,
				confirmLabel: 'Add anyway',
				cancelLabel: r.added.length ? 'Skip duplicates' : "Don't add"
			});
			if (anyway) {
				const dupes = songs.filter((s) => r.duplicates.includes(s.video_id));
				const r2 = await lib.addToPlaylist(target.id, dupes, { allowDuplicates: true });
				const undo1 = r.undo;
				toastUndo(`Added to ${target.title}`, async () => {
					await r2.undo();
					await undo1();
				});
				return;
			}
		}
		if (r.added.length) toastUndo(`Added to ${target.title}`, r.undo);
	} catch (e) {
		toast.fail(e, `Couldn't add to ${target.title}`);
	}
}

/** Spotify's Create ▸ Playlist: makes "My Playlist #N" at once, adds `songs`, opens it. */
export async function createPlaylist(
	songs: SongItem[] = [],
	open = true,
	folderId?: string
): Promise<BrowseItem | null> {
	try {
		const p = await lib.createPlaylist(undefined, undefined, folderId);
		const card: BrowseItem = { kind: 'playlist', id: p.id, title: p.name };
		if (songs.length) {
			const r = await lib.addToPlaylist(p.id, songs, { allowDuplicates: true });
			if (r.added.length) toast(`Added to ${p.name}`);
		}
		if (open) goto(playlistHref(p.id));
		return card;
	} catch (e) {
		toast.fail(e, "Couldn't create the playlist");
		return null;
	}
}

export async function createFolder(parentId?: string) {
	try {
		return await lib.createFolder(undefined, parentId);
	} catch (e) {
		toast.fail(e, "Couldn't create the folder");
		return null;
	}
}

/** The old naming dialog, for surfaces that want it. */
export const createPlaylistDialog = (songs: SongItem[] = []) => openNewPlaylist(songs);

export async function removeFromPlaylist(playlistId: string, songs: SongItem[]) {
	const rows = songs.filter((s) => s.set_video_id);
	if (!rows.length) return;
	const msg = rows.length === 1 ? 'Removed from playlist' : `Removed ${rows.length} songs from playlist`;
	try {
		if (api.isLocalPlaylist(playlistId)) {
			const { undo } = await lib.removeFromPlaylist(
				playlistId,
				rows.map((s) => Number(s.set_video_id))
			);
			toastUndo(msg, undo);
			return;
		}
		// ── lane magic-d1 ── Undo on a YouTube playlist too (MAGIC-GAPS #17). Where each row sat
		// is read first: after the removal nothing remembers it.
		const order = await orderOf(playlistId);
		if (rows.length === 1)
			await api.removeFromPlaylist(playlistId, rows[0].video_id, rows[0].set_video_id!);
		else
			await api.removeManyFromPlaylist(
				playlistId,
				rows.map((s) => [s.video_id, s.set_video_id!] as [string, string])
			);
		toastUndo(msg, () => putBack(playlistId, rows, order));
	} catch (e) {
		toast.fail(e, "Couldn't remove that from the playlist");
	}
}

/** The playlist's row ids in order (first page), or null when it can't be read quickly. */
async function orderOf(playlistId: string): Promise<string[] | null> {
	const timeout = new Promise<null>((r) => setTimeout(() => r(null), 2500));
	try {
		const p = await Promise.race([api.getPlaylist(playlistId), timeout]);
		return p ? p.items.map((s) => s.set_video_id ?? '') : null;
	} catch {
		return null;
	}
}

/** Re-add removed rows and move them back to where they were. */
async function putBack(playlistId: string, rows: SongItem[], order: string[] | null) {
	const removed = rows.map((s) => ({ setVideoId: s.set_video_id!, videoId: s.video_id }));
	const before = new Set((await api.getPlaylist(playlistId)).items.map((s) => s.set_video_id ?? ''));
	for (const r of removed) await api.addToPlaylist(playlistId, r.videoId, true);
	if (order) {
		const now = (await api.getPlaylist(playlistId)).items;
		const fresh = matchFresh(removed, before, now);
		for (const m of restoreMoves(order, removed, fresh))
			await api.moveInPlaylist(playlistId, m.setVideoId, m.before);
	}
	toast(rows.length === 1 ? 'Song restored' : `${rows.length} songs restored`);
	// The playlist page keeps its own copy of the rows: remount it if it's the one on screen.
	if (page.url?.pathname === playlistHref(playlistId)) ui.epoch++;
}

export async function deletePlaylist(item: BrowseItem, ask = true) {
	if (ask) {
		const { confirmDialog } = await import('./confirm.svelte');
		const ok = await confirmDialog({
			title: 'Delete from Your Library?',
			body: `This will delete ${item.title} from Your Library.`,
			confirmLabel: 'Delete'
		});
		if (!ok) return;
	}
	try {
		if (api.isLocalPlaylist(item.id)) {
			toastUndo('Removed from Your Library', await lib.deletePlaylist(item.id));
			return;
		}
		await api.deletePlaylist(item.id);
		forgetPlaylist(item.id);
		toast('Removed from Your Library');
	} catch (e) {
		toast.fail(e, "Couldn't delete the playlist");
	}
}

export function editPlaylistDetails(item: BrowseItem) {
	goto(`${playlistHref(item.id)}?edit=1`);
}

// ── Library: save / pin ──────────────────────────────────────────────────────────────────────

export const isInLibrary = (item: BrowseItem): boolean =>
	lib.isSaved(item.id) || isOwnPlaylist(item.id) || inLibrary(item);

export async function toggleInLibrary(item: BrowseItem) {
	try {
		if (lib.isSaved(item.id)) {
			toastUndo('Removed from Your Library', await lib.unsave(item.id));
		} else if (isInLibrary(item)) {
			await removeFromLibrary(item);
			toast('Removed from Your Library');
		} else {
			await addToLibrary(item);
			toast('Added to Your Library');
		}
	} catch (e) {
		toast.fail(e, "Couldn't update Your Library");
	}
}

export const MAX_PINS = api.MAX_PINS;
export const isPinned = (id: string): boolean => lib.isPinned(id);

export function togglePin(id: string) {
	const r = lib.togglePin(id);
	if (r === 'full') toast('You can only pin 4 items');
	return r;
}

export const isOwned = (item: BrowseItem) => isOwnPlaylist(item.id) || ownedByUser(item);

// ── Artists ──────────────────────────────────────────────────────────────────────────────────

export async function dontPlayArtist(id: string | undefined, name: string) {
	await blockArtist(id, name);
	toast(`We won't play ${name}`);
}

export { lib, loadLibrary, library, personal };

// ── lane magic-d1 ── "Show credits" (MAGIC-GAPS #6) ──────────────────────────────────────────

/** The Credits dialog for `song`. Mounted on first use, so no layout has to host it. */
export async function showCredits(song: SongItem) {
	const { openCredits } = await import('./credits.svelte');
	openCredits(song);
}

// ── lane magic-d1 ── "Exclude from your taste profile" (MAGIC-GAPS #16): state in taste.svelte.ts.
export { isTasteExcluded, loadTasteExcluded, toggleTasteExcluded } from './taste.svelte';

// ── Podcasts (routes owned by the podcast page builder; change here only) ──────────────────
export const showHref = (id: string) => `/show/${encodeURIComponent(id)}`;
export const YOUR_EPISODES_HREF = '/collection/episodes';
/** "Local Files" (sidebar row). The local-music page stream may move it to its own route. */
export const LOCAL_FILES_HREF = '/collection/local-files';
