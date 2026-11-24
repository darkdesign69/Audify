// Spotify's menus (docs/spotify-spec/screens/menus.md), as data. Each builder returns MenuEntry[];
// the `open*Menu` helpers open them at a right-click point or under a "…" button.
import * as api from '$lib/api';
import ArrowCircleDownIcon from 'phosphor-svelte/lib/ArrowCircleDownIcon'; // stream: downloads
import { collectionState, isCollectionDownloaded, retryDownloads, toggleDownload } from '$lib/downloads.svelte'; // stream: downloads
import ArrowClockwiseIcon from 'phosphor-svelte/lib/ArrowClockwiseIcon';
import type { BrowseItem, Folder, SongItem } from '$lib/api';
import PlusIcon from 'phosphor-svelte/lib/PlusIcon';
import MinusCircleIcon from 'phosphor-svelte/lib/MinusCircleIcon';
import HeartIcon from 'phosphor-svelte/lib/HeartIcon';
import QueueIcon from 'phosphor-svelte/lib/QueueIcon';
import ArrowBendDownRightIcon from 'phosphor-svelte/lib/ArrowBendDownRightIcon';
import BroadcastIcon from 'phosphor-svelte/lib/BroadcastIcon';
import UserIcon from 'phosphor-svelte/lib/UserIcon';
import VinylRecordIcon from 'phosphor-svelte/lib/VinylRecordIcon';
import ShareFatIcon from 'phosphor-svelte/lib/ShareFatIcon';
import LinkIcon from 'phosphor-svelte/lib/LinkIcon';
import PencilSimpleIcon from 'phosphor-svelte/lib/PencilSimpleIcon';
import TrashIcon from 'phosphor-svelte/lib/TrashIcon';
import FolderSimpleIcon from 'phosphor-svelte/lib/FolderSimpleIcon';
import FolderSimplePlusIcon from 'phosphor-svelte/lib/FolderSimplePlusIcon';
import PushPinIcon from 'phosphor-svelte/lib/PushPinIcon';
import PushPinSlashIcon from 'phosphor-svelte/lib/PushPinSlashIcon';
import ProhibitIcon from 'phosphor-svelte/lib/ProhibitIcon';
import EyeSlashIcon from 'phosphor-svelte/lib/EyeSlashIcon';
import MusicNotesPlusIcon from 'phosphor-svelte/lib/MusicNotesPlusIcon';
import UserPlusIcon from 'phosphor-svelte/lib/UserPlusIcon';
import UserMinusIcon from 'phosphor-svelte/lib/UserMinusIcon';
import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
import XIcon from 'phosphor-svelte/lib/XIcon';
import IdentificationCardIcon from 'phosphor-svelte/lib/IdentificationCardIcon'; // lane magic-d1
import UserCircleMinusIcon from 'phosphor-svelte/lib/UserCircleMinusIcon'; // lane magic-d1
import UserCirclePlusIcon from 'phosphor-svelte/lib/UserCirclePlusIcon'; // lane magic-d1
import * as A from '../actions';
import { lib } from '$lib/library.svelte';
import { toast, toastUndo } from '../toast';
import { confirmDialog } from '../confirm.svelte'; // stream: downloads
import { openMenu } from './state.svelte';
import { copyText } from '$lib/clipboard';
import type { ItemMenuContext, MenuEntry, MenuTarget, TrackMenuContext } from './types';

const sep: MenuEntry = { type: 'separator' };

/** Strip leading/trailing/double separators left by conditional items. */
function tidy(entries: (MenuEntry | false | null | undefined)[]): MenuEntry[] {
	const out: MenuEntry[] = [];
	for (const e of entries) {
		if (!e) continue;
		if (e.type === 'separator' && (!out.length || out[out.length - 1].type === 'separator')) continue;
		out.push(e);
	}
	while (out.length && out[out.length - 1].type === 'separator') out.pop();
	return out;
}

function shareSub(item: { kind: BrowseItem['kind'] | 'song'; id: string }, label: string): MenuEntry {
	return {
		type: 'sub',
		label: 'Share',
		icon: ShareFatIcon,
		width: 220,
		items: [{ label, icon: LinkIcon, onSelect: () => A.copyLink(item) }]
	};
}

// ── Tracks ───────────────────────────────────────────────────────────────────────────────────

export function trackMenu(songs: SongItem[], ctx: TrackMenuContext = {}): MenuEntry[] {
	if (!songs.length) return [];
	const own =
		!!ctx.playlistId && (ctx.own ?? (A.isOwnPlaylist(ctx.playlistId) && ctx.playlistId !== api.LIKED_MUSIC_ID));
	const removable = own && songs.some((s) => s.set_video_id);
	const allLiked = songs.every((s) => A.isLiked(s));
	const local = songs.some((s) => api.isLocalId(s.video_id));
	const removeFromPlaylist: MenuEntry | false = removable && {
		label: 'Remove from this playlist',
		icon: MinusCircleIcon,
		onSelect: async () => {
			ctx.onRemove?.(songs);
			await A.removeFromPlaylist(ctx.playlistId!, songs);
		}
	};
	const likeItem: MenuEntry | false = !local && {
		label: allLiked ? 'Remove from your Liked Songs' : 'Save to your Liked Songs',
		icon: HeartIcon,
		onSelect: () => (allLiked ? A.unlikeSongs(songs) : A.likeSongs(songs))
	};
	const queue: MenuEntry[] = [
		{ label: 'Add to queue', icon: QueueIcon, onSelect: () => A.addToQueue(songs) },
		{ label: 'Play next', icon: ArrowBendDownRightIcon, onSelect: () => A.playNext(songs) }
	];
	const removeFromQueue: MenuEntry | false = ctx.queueIndex !== undefined && {
		label: 'Remove from queue',
		icon: MinusCircleIcon,
		onSelect: () => A.removeFromQueue(ctx.queueIndex!)
	};

	if (songs.length > 1) {
		return tidy([
			{ type: 'playlist-picker', songs },
			removeFromPlaylist,
			likeItem,
			...queue
		]);
	}

	const song = songs[0];
	// Spotify lists "Go to artist" in every context (album and artist pages too); only "Go to
	// album" is dropped on that album's own page.
	let artists = A.songArtists(song).filter((a) => a.id);
	// Album tracks often come without artist ids: fall back to the page's artist.
	if (!artists.length && ctx.artistId) artists = [{ id: ctx.artistId, name: song.artists }];
	const goArtist: MenuEntry | false =
		artists.length === 1
			? { label: 'Go to artist', icon: UserIcon, onSelect: () => A.goArtist(artists[0].id!) }
			: artists.length > 1 && {
					type: 'sub',
					label: 'Go to artist',
					icon: UserIcon,
					items: artists.map((a) => ({ label: a.name, onSelect: () => A.goArtist(a.id!) }))
				};
	const goAlbum: MenuEntry | false = !!song.album_id &&
		song.album_id !== ctx.albumId && {
			label: 'Go to album',
			icon: VinylRecordIcon,
			onSelect: () => A.goAlbum(song.album_id!)
		};
	const hide: MenuEntry | false = !local &&
		!own && {
			label: lib.isHidden(song.video_id) ? 'Unhide song' : 'Hide song',
			icon: EyeSlashIcon,
			onSelect: async () => {
				if (lib.isHidden(song.video_id)) {
					await lib.unhideSong(song.video_id, ctx.playlistId);
					toast('Song unhidden');
				} else toastUndo('Song hidden', await lib.hideSong(song, ctx.playlistId));
			}
		};

	// ── lane magic-d1 ── Spotify's "Exclude from your taste profile" (MAGIC-GAPS #16).
	const excluded = A.isTasteExcluded(song.video_id);
	const tasteItem: MenuEntry | false = !local && {
		label: excluded ? 'Include in your taste profile' : 'Exclude from your taste profile',
		icon: excluded ? UserCirclePlusIcon : UserCircleMinusIcon,
		onSelect: () => A.toggleTasteExcluded({ kind: 'song', id: song.video_id, title: song.title }, () => [song])
	};

	return tidy([
		{ type: 'playlist-picker', songs },
		removeFromPlaylist,
		likeItem,
		!!song.is_episode && episodeDownloadEntry(song),
		...queue,
		removeFromQueue,
		hide,
		tasteItem,
		sep,
		// Opens the radio as a page; the music keeps playing until its Play (MAGIC-GAPS #1).
		!local && { label: 'Go to song radio', icon: BroadcastIcon, onSelect: () => A.songRadio(song) },
		goArtist,
		goAlbum,
		!local && { label: 'Show credits', icon: IdentificationCardIcon, onSelect: () => A.showCredits(song) },
		sep,
		!local && shareSub({ kind: 'song', id: song.video_id }, 'Copy song link')
	]);
}

/**
 * The accent "liked" check (track rows, player bar): Spotify opens the "Saved in" picker instead
 * of unliking at once. A song that isn't liked yet is liked directly (no picker), as on Spotify.
 */
export function openSavedIn(target: MenuTarget, song: SongItem) {
	if (!A.isLiked(song)) {
		void A.toggleLike(song);
		return;
	}
	openMenu(target, [{ type: 'saved-in', song }], { side: 'left', align: 'center', width: 338 });
}

export function openTrackMenu(target: MenuTarget, songs: SongItem[], ctx?: TrackMenuContext) {
	openMenu(target, trackMenu(songs, ctx), { width: 328 });
}

// ── Albums ───────────────────────────────────────────────────────────────────────────────────

async function tracksOf(item: BrowseItem): Promise<SongItem[]> {
	if (item.kind === 'album') return (await api.getAlbum(item.id)).items;
	if (item.kind === 'playlist') return (await api.getPlaylist(item.id)).items;
	if (item.kind === 'song')
		return [{ video_id: item.id, title: item.title, artists: item.subtitle ?? '', thumbnail: item.thumbnail }];
	return [];
}

async function queueItem(item: BrowseItem, next = false) {
	try {
		const songs = await tracksOf(item);
		await (next ? A.playNext(songs, item.title) : A.addToQueue(songs, item.title));
	} catch (e) {
		toast.fail(e, "Couldn't add to queue");
	}
}

// ── stream: downloads ── Spotify's "Download" / "Remove download" item on albums and playlists.
function downloadEntry(item: BrowseItem, kind: 'album' | 'playlist' | 'liked'): MenuEntry | false {
	if (api.isLocalId(item.id) || item.id === api.ON_REPEAT_ID) return false;
	const on = isCollectionDownloaded(item.id);
	return {
		label: on ? 'Remove download' : 'Download',
		icon: ArrowCircleDownIcon,
		onSelect: () =>
			toggleDownload(
				{ id: item.id, kind, title: item.title, subtitle: item.subtitle ?? null, thumbnail: item.thumbnail ?? null },
				{
					confirm: () =>
						confirmDialog({
							title: 'Remove from Downloads?',
							body: "You won't be able to play this offline.",
							confirmLabel: 'Remove'
						})
				}
			).catch((e) => toast.fail(e, "Couldn't update the download"))
	};
}

/** An episode's Download / Remove download (Retry once it failed), from its queue row: show
 *  page, episode page, Your Episodes, search, Now Playing. Downloading also saves the episode to
 *  Your Episodes (the core's rule, iOS gets the same). */
export function episodeDownloadEntry(song: SongItem): MenuEntry {
	const st = collectionState(song.video_id).state;
	if (st === 'failed') {
		return { label: 'Retry download', icon: ArrowClockwiseIcon, onSelect: () => retryDownloads().catch((e) => toast.fail(e, "Couldn't retry")) };
	}
	const target = { id: song.video_id, kind: 'episode' as const, title: song.title, subtitle: song.artists || null, thumbnail: song.thumbnail ?? null };
	return {
		label: st === 'none' ? 'Download' : st === 'done' ? 'Remove download' : 'Downloading…',
		icon: ArrowCircleDownIcon,
		onSelect: () =>
			toggleDownload(target, {
				songs: [song],
				confirm: () =>
					confirmDialog({ title: 'Remove from Downloads?', body: "You won't be able to play this offline.", confirmLabel: 'Remove' })
			}).catch((e) => toast.fail(e, "Couldn't update the download"))
	};
}

export function albumMenu(item: BrowseItem): MenuEntry[] {
	const saved = A.isInLibrary(item);
	return tidy([
		{
			label: saved ? 'Remove from Your Library' : 'Add to Your Library',
			icon: saved ? CheckCircleIcon : PlusCircleIcon,
			onSelect: () => A.toggleInLibrary(item)
		},
		downloadEntry(item, 'album'),
		{ label: 'Add to queue', icon: QueueIcon, onSelect: () => queueItem(item) },
		sep,
		{ label: 'Go to album radio', icon: BroadcastIcon, onSelect: () => A.goRadio('album', item.id, item.title, item.thumbnail) },
		{
			type: 'sub',
			label: 'Add to playlist',
			icon: PlusIcon,
			width: 350,
			items: () => addToPlaylistLazy(item)
		},
		sep,
		shareSub(item, 'Copy album link')
	]);
}

/** Album/playlist "Add to playlist ▸" without fetching tracks up front: pick a target, then fetch. */
function addToPlaylistLazy(item: BrowseItem): MenuEntry[] {
	return [
		{
			label: 'New playlist',
			icon: PlusIcon,
			onSelect: async () => A.createPlaylist(await tracksOf(item), false)
		},
		sep,
		...A.editablePlaylists().map((p) => ({
			label: p.title,
			onSelect: async () => A.addToPlaylist(p, await tracksOf(item))
		}))
	];
}

// ── Artists ──────────────────────────────────────────────────────────────────────────────────

export function artistMenu(item: BrowseItem): MenuEntry[] {
	const following = lib.isSaved(item.id) || A.isInLibrary(item);
	return tidy([
		{
			label: following ? 'Unfollow' : 'Follow',
			icon: following ? UserMinusIcon : UserPlusIcon,
			onSelect: () => A.toggleInLibrary(item)
		},
		{ label: "Don't play this artist", icon: ProhibitIcon, onSelect: () => A.dontPlayArtist(item.id, item.title) },
		sep,
		{ label: 'Go to artist radio', icon: BroadcastIcon, onSelect: () => A.goRadio('artist', item.id, item.title, item.thumbnail) },
		sep,
		shareSub(item, 'Copy link to artist')
	]);
}

// ── Playlists ────────────────────────────────────────────────────────────────────────────────

export function playlistMenu(item: BrowseItem, ctx: ItemMenuContext = {}): MenuEntry[] {
	const own = A.isOwnPlaylist(item.id) && item.id !== api.LIKED_MUSIC_ID;
	const liked = item.id === api.LIKED_MUSIC_ID || item.id === api.LIKED_ID;
	const pinned = A.isPinned(item.id);
	const folders = lib.folders;
	const inFolder = lib.folderOf(item.id);
	// Spotify offers Move to folder ▸ in the playlist page's "…" menu too, not only in the sidebar.
	const moveToFolder: MenuEntry | false = own && {
			type: 'sub',
			label: 'Move to folder',
			icon: FolderSimpleIcon,
			width: 260,
			items: () =>
				tidy([
					{
						label: 'Create folder',
						icon: FolderSimplePlusIcon,
						onSelect: async () => {
							const f = await A.createFolder();
							if (f) await lib.setItemFolder(item.id, f.id);
						}
					},
					inFolder && {
						label: 'Remove from folder',
						icon: XIcon,
						onSelect: async () => toastUndo('Removed from folder', await lib.setItemFolder(item.id, null))
					},
					folders.length > 0 && sep,
					...folders.map((f) => ({
						label: f.name,
						checked: inFolder?.id === f.id,
						onSelect: async () => toastUndo(`Moved to ${f.name}`, await lib.setItemFolder(item.id, f.id))
					}))
				])
		};
	return tidy([
		{ label: 'Add to queue', icon: QueueIcon, onSelect: () => queueItem(item) },
		!own &&
			!liked && {
				label: A.isInLibrary(item) ? 'Remove from Your Library' : 'Add to Your Library',
				icon: A.isInLibrary(item) ? CheckCircleIcon : PlusCircleIcon,
				onSelect: () => A.toggleInLibrary(item)
			},
		downloadEntry(item, liked ? 'liked' : 'playlist'),
		own && { label: 'Edit details', icon: PencilSimpleIcon, onSelect: () => A.editPlaylistDetails(item) },
		own && { label: 'Delete', icon: TrashIcon, onSelect: () => A.deletePlaylist(item) },
		sep,
		ctx.sidebar && { label: 'Create playlist', icon: MusicNotesPlusIcon, onSelect: () => A.createPlaylist() },
		ctx.sidebar && { label: 'Create folder', icon: FolderSimplePlusIcon, onSelect: () => A.createFolder() },
		moveToFolder,
		{
			label: pinned ? 'Unpin playlist' : 'Pin playlist',
			icon: pinned ? PushPinSlashIcon : PushPinIcon,
			onSelect: () => A.togglePin(item.id)
		},
		sep,
		// ── lane magic-d1 ── (MAGIC-GAPS #16) Spotify lists it on playlist menus.
		!liked && {
			label: A.isTasteExcluded(item.id) ? 'Include in your taste profile' : 'Exclude from your taste profile',
			icon: A.isTasteExcluded(item.id) ? UserCirclePlusIcon : UserCircleMinusIcon,
			onSelect: () => A.toggleTasteExcluded({ kind: 'playlist', id: item.id, title: item.title }, () => tracksOf(item))
		},
		!liked &&
			// A playlist on this machine has no YouTube radio behind it.
			!api.isLocalPlaylist(item.id) && {
			label: 'Go to playlist radio',
			icon: BroadcastIcon,
			onSelect: () => A.goRadio('playlist', item.id, item.title, item.thumbnail)
		},
		!liked &&
			(api.isLocalPlaylist(item.id)
				? {
						// A local playlist has no web link; Share ▸ copies its track list instead.
						type: 'sub',
						label: 'Share',
						icon: ShareFatIcon,
						width: 220,
						items: [{ label: 'Copy song list', icon: LinkIcon, onSelect: () => copySongList(item) }]
					}
				: shareSub(item, 'Copy link to playlist'))
	]);
}

async function copySongList(item: BrowseItem) {
	try {
		const songs = await tracksOf(item);
		const lines = songs.map((t) => `${t.title} — ${t.artists}`);
		await copyText([item.title, '', ...lines].join('\n'));
		toast('Song list copied to clipboard');
	} catch (e) {
		toast.fail(e, "Couldn't copy the song list");
	}
}

// ── Folders ──────────────────────────────────────────────────────────────────────────────────

export function folderMenu(folder: Folder, ctx: ItemMenuContext = {}): MenuEntry[] {
	const pinned = A.isPinned(folder.id);
	return tidy([
		ctx.onRename && { label: 'Rename', icon: PencilSimpleIcon, onSelect: ctx.onRename },
		{
			label: 'Delete',
			icon: TrashIcon,
			onSelect: async () => toastUndo('Folder deleted', await lib.deleteFolder(folder.id))
		},
		sep,
		{ label: 'Create playlist', icon: MusicNotesPlusIcon, onSelect: () => A.createPlaylist([], true, folder.id) },
		{ label: 'Create folder', icon: FolderSimplePlusIcon, onSelect: () => A.createFolder(folder.id) },
		{
			label: pinned ? 'Unpin folder' : 'Pin folder',
			icon: pinned ? PushPinSlashIcon : PushPinIcon,
			onSelect: () => A.togglePin(folder.id)
		}
	]);
}

// ── Any card ────────────────────────────────────────────────────────────────────────────────

/** The right menu for a card/library row by kind. */
export function itemMenu(item: BrowseItem, ctx: ItemMenuContext = {}): MenuEntry[] {
	switch (item.kind) {
		case 'song':
			return trackMenu(
				[
					{
						video_id: item.id,
						title: item.title,
						artists: item.subtitle ?? '',
						thumbnail: item.thumbnail,
						album_id: item.albumId,
						artist_runs: item.artistRuns
					}
				],
				{}
			);
		case 'album':
			return albumMenu(item);
		case 'artist':
			return artistMenu(item);
		default:
			return playlistMenu(item, ctx);
	}
}

export function openItemMenu(target: MenuTarget, item: BrowseItem, ctx?: ItemMenuContext) {
	// Spotify's playlist menu is ~220px wide.
	openMenu(target, itemMenu(item, ctx), item.kind === 'playlist' ? { width: 220 } : {});
}

export function openFolderMenu(target: MenuTarget, folder: Folder, ctx?: ItemMenuContext) {
	openMenu(target, folderMenu(folder, ctx));
}

/** Queue panel row: the track menu plus "Remove from queue". */
export function openQueueItemMenu(target: MenuTarget, song: SongItem, queueIndex: number) {
	openTrackMenu(target, [song], { queueIndex });
}
