// The library sidebar's rows, built from the local library store (`lib`), the podcasts store and,
// when signed in, the YouTube account's playlists. One normalized shape for every view.
import * as api from '$lib/api';
import { convertFileSrc } from '@tauri-apps/api/core';
import type { BrowseItem } from '$lib/api';
import { lib } from '$lib/library.svelte';
import { podcasts } from '$lib/podcasts.svelte';
import { bookHref, books } from '$lib/audiobooks.svelte'; // stream: audiobooks
import { library, local, ownedByUser } from '$lib/player.svelte';
import { LOCAL_FILES_HREF, YOUR_EPISODES_HREF, hrefFor, showHref } from '$lib/kit/actions';
import { currentSort, libraryUi, type LibFilter } from './library.svelte';
import { collections as dlCollections } from '$lib/downloads.svelte'; // stream: downloads
import { ownerName } from '$lib/localuser.svelte'; // stream: parity4
import { recentsKey } from '$lib/homelive'; // stream: magic-d4

export type LibRow = {
	id: string;
	kind: 'liked' | 'episodes' | 'local' | 'playlist' | 'album' | 'artist' | 'podcast' | 'folder' | 'audiobook';
	title: string;
	/** "Playlist • You" etc. (without the pin). */
	subtitle: string;
	/** Typed label for compact view (" • Playlist"). */
	typeLabel: string;
	creator: string;
	image?: string | null;
	href: string;
	pinned: boolean;
	byYou: boolean;
	addedAt: number;
	playedAt: number;
	/** Position for Custom order. */
	position: number;
	/** The card for menus / play. */
	item: BrowseItem;
	folderId?: string | null;
};

/** The creator from a YouTube card subtitle ("Album • Daft Punk • 2001", "Playlist • Owner • 12 songs",
 *  "Single • Artist", "Daft Punk"): drop leading type words and trailing year/count segments. */
const TYPE_WORDS = /^(album|single|ep|playlist|artist|podcast|song|video|episode|mix|radio|compilation|live)$/i;
export function creatorOf(subtitle: string | null | undefined): string {
	const parts = (subtitle ?? '')
		.split(/\s+[•·]\s+/)
		.map((p) => p.trim())
		.filter(Boolean);
	const rest = parts.filter(
		(p) => !TYPE_WORDS.test(p) && !/^\d{4}$/.test(p) &&
			!/^[\d.,]+[KMB]?\s+(songs?|tracks?|views?|episodes?|subscribers?|monthly|plays?)/i.test(p) &&
			!/^\d+\s*(hours?|hr|minutes?|min|seconds?|sec)\b/i.test(p)
	);
	return rest[0] ?? '';
}

const SECS = (n: number | null | undefined) => (n ? (n > 1e12 ? Math.floor(n / 1000) : n) : 0);
/** stream: magic-d4 — Recents' key for an item: the later of last played from and last added to /
 *  edited (`lib.lastTouched`: a like for Liked Songs), like Spotify's Recents. */
export const recentKey = (id: string, ...also: (number | null | undefined)[]) =>
	recentsKey(lib.lastPlayed[id], lib.lastTouched[id], ...also);

function rows(): LibRow[] {
	const out: LibRow[] = [];
	const liked = api.LIKED_ID;
	// Like Spotify, Liked Songs exists in the library once there is something in it.
	if (lib.likedCount > 0 || lib.isPinned(liked)) out.push({
		id: liked,
		kind: 'liked',
		title: 'Liked Songs',
		subtitle: `Playlist • ${lib.likedCount} song${lib.likedCount === 1 ? '' : 's'}`,
		typeLabel: 'Playlist',
		creator: 'You',
		href: `/playlist/${liked}`,
		pinned: lib.isPinned(liked),
		byYou: true,
		addedAt: 0,
		playedAt: recentKey(liked),
		position: -2,
		item: { kind: 'playlist', id: liked, title: 'Liked Songs' }
	});
	if (podcasts.saved.length) {
		out.push({
			id: 'EPISODES',
			kind: 'episodes',
			title: 'Your Episodes',
			subtitle: `Saved & downloaded episodes`,
			typeLabel: 'Playlist',
			creator: 'You',
			href: YOUR_EPISODES_HREF,
			pinned: lib.isPinned('EPISODES'),
			byYou: true,
			addedAt: 0,
			playedAt: recentKey('EPISODES'),
			position: -1,
			item: { kind: 'playlist', id: 'EPISODES', title: 'Your Episodes' }
		});
	}
	if (local.folders.length || local.songs.length) {
		out.push({
			id: 'LOCALFILES',
			kind: 'local',
			title: 'Local Files',
			subtitle: `Playlist • ${local.songs.length} song${local.songs.length === 1 ? '' : 's'}`,
			typeLabel: 'Playlist',
			creator: 'You',
			href: LOCAL_FILES_HREF,
			pinned: lib.isPinned('LOCALFILES'),
			byYou: true,
			addedAt: 0,
			playedAt: recentKey('LOCALFILES'),
			position: -0.5,
			item: { kind: 'playlist', id: 'LOCALFILES', title: 'Local Files' }
		});
	}
	lib.playlists.forEach((p, i) => {
		out.push({
			id: p.id,
			kind: 'playlist',
			title: p.name,
			subtitle: `Playlist • ${ownerName()}`,
			typeLabel: 'Playlist',
			creator: ownerName(),
			image: p.cover ? (/^(https?|asset|data):/.test(p.cover) ? p.cover : convertFileSrc(p.cover)) : p.thumbnail,
			href: `/playlist/${encodeURIComponent(p.id)}`,
			pinned: p.pinned || lib.isPinned(p.id),
			byYou: true,
			addedAt: SECS(p.createdAt),
			playedAt: recentKey(p.id, p.lastPlayedAt) || SECS(p.updatedAt),
			position: i,
			item: { kind: 'playlist', id: p.id, title: p.name, thumbnail: p.thumbnail ?? undefined },
			folderId: p.folderId
		});
	});
	const seen = new Set(out.map((r) => r.id));
	lib.saved.forEach((s, i) => {
		if (seen.has(s.id)) return;
		seen.add(s.id);
		const kind = s.kind;
		const creator = creatorOf(s.subtitle);
		out.push({
			id: s.id,
			kind,
			title: s.title,
			subtitle:
				kind === 'artist' ? 'Artist' : `${kind === 'album' ? 'Album' : 'Playlist'}${creator ? ` • ${creator}` : ''}`,
			typeLabel: kind === 'artist' ? 'Artist' : kind === 'album' ? 'Album' : 'Playlist',
			creator,
			image: s.thumbnail,
			href: hrefFor(s.card),
			pinned: lib.isPinned(s.id),
			byYou: false,
			addedAt: SECS(s.addedAt),
			playedAt: recentKey(s.id) || SECS(s.addedAt),
			position: 1000 + i,
			item: s.card,
			folderId: lib.folderOf(s.id)?.id ?? null
		});
	});
	// Signed-in YouTube library playlists that aren't saved locally.
	library.items.forEach((p, i) => {
		if (p.kind !== 'playlist' || seen.has(p.id) || p.id === api.LIKED_MUSIC_ID || api.isLocalPlaylist(p.id)) return;
		seen.add(p.id);
		const own = ownedByUser(p);
		const creator = creatorOf(p.subtitle);
		out.push({
			id: p.id,
			kind: 'playlist',
			title: p.title,
			subtitle: `Playlist${creator ? ` • ${creator}` : ''}`,
			typeLabel: 'Playlist',
			creator,
			image: p.thumbnail,
			href: hrefFor(p),
			pinned: lib.isPinned(p.id),
			byYou: own,
			addedAt: 0,
			playedAt: recentKey(p.id),
			position: 2000 + i,
			item: p,
			folderId: lib.folderOf(p.id)?.id ?? null
		});
	});
	podcasts.follows.forEach((s, i) => {
		if (seen.has(s.id)) return;
		out.push({
			id: s.id,
			kind: 'podcast',
			title: s.title,
			subtitle: `Podcast • ${s.author}`,
			typeLabel: 'Podcast',
			creator: s.author,
			image: s.thumbnail,
			href: showHref(s.id),
			pinned: lib.isPinned(s.id),
			byYou: false,
			addedAt: 0,
			playedAt: recentKey(s.id),
			position: 3000 + i,
			item: { kind: 'playlist', id: s.id, title: s.title, subtitle: s.author, thumbnail: s.thumbnail ?? undefined }
		});
	});
	// stream: audiobooks — saved books.
	books.saved.forEach((b, i) => {
		out.push({
			id: b.id,
			kind: 'audiobook',
			title: b.title,
			subtitle: `Audiobook • ${b.author}`,
			typeLabel: 'Audiobook',
			creator: b.author,
			image: b.thumbnail,
			href: bookHref(b.id),
			pinned: lib.isPinned(b.id),
			byYou: false,
			addedAt: 0,
			playedAt: recentKey(b.id),
			position: 4000 + i,
			item: { kind: 'playlist', id: b.id, title: b.title, subtitle: b.author, thumbnail: b.thumbnail ?? undefined }
		});
	});
	return out;
}

function folderRows(parent: string | null): LibRow[] {
	return lib.childFolders(parent).map((f) => ({
		id: f.id,
		kind: 'folder' as const,
		title: f.name,
		subtitle: `${f.items.length} playlist${f.items.length === 1 ? '' : 's'}`,
		typeLabel: 'Folder',
		creator: 'You',
		href: '',
		pinned: lib.isPinned(f.id),
		byYou: true,
		addedAt: Math.floor(f.createdAt),
		playedAt: Math.floor(f.updatedAt),
		position: f.position,
		item: { kind: 'playlist' as const, id: f.id, title: f.name },
		folderId: f.parentId ?? null
	}));
}

const matchesFilter = (r: LibRow, f: LibFilter | null, byYou: boolean) => {
	if (f === null) return true;
	if (f === 'playlist') return (r.kind === 'playlist' || r.kind === 'liked' || r.kind === 'episodes' || r.kind === 'local' || r.kind === 'folder') && (!byYou || r.byYou);
	if (f === 'podcast') return r.kind === 'podcast' || r.kind === 'episodes';
	return r.kind === f;
};

// ── stream: downloads ──
/** Has the user downloaded this library row (playlist, album, Liked Songs; a show with a
 *  downloaded episode)? Drives the "Downloaded" chip and the accent arrow in the subtitle. */
export function isRowDownloaded(r: LibRow): boolean {
	if (r.kind === 'folder') return false;
	if (r.kind === 'podcast') {
		for (const c of dlCollections.values()) if (c.kind === 'episode' && c.subtitle === r.title) return true;
		return false;
	}
	// Downloaded episodes are saved to Your Episodes, so that row lists under Downloaded too.
	if (r.kind === 'episodes') {
		for (const c of dlCollections.values()) if (c.kind === 'episode') return true;
		return false;
	}
	if (r.kind === 'audiobook') return dlCollections.has(`abook:${r.id}`); // stream: audiobooks
	return dlCollections.has(r.id);
}
/** Show the "Downloaded" chip at all (Spotify shows it once something is downloaded). */
export const hasDownloads = () => dlCollections.size > 0;
// ── end stream: downloads ──

/** Which filter chips have anything behind them (Spotify only shows present types). */
export function presentFilters(): LibFilter[] {
	const all = rows();
	return (['playlist', 'artist', 'album', 'podcast', 'audiobook'] as LibFilter[]).filter((f) =>
		all.some((r) => r.kind !== 'liked' && matchesFilter(r, f, false))
	);
}

/** The visible, sorted rows for the current filter / search / folder / sort. */
export function visibleRows(): { pinned: LibRow[]; rest: LibRow[] } {
	const folder = libraryUi.folderPath.at(-1) ?? null;
	const q = libraryUi.query.trim().toLowerCase();
	let all: LibRow[];
	if (folder) {
		const f = lib.folder(folder);
		const ids = f?.items ?? [];
		const base = rows().filter((r) => ids.includes(r.id));
		base.forEach((r) => (r.position = ids.indexOf(r.id)));
		all = [...folderRows(folder), ...base];
	} else {
		const inFolders = new Set(lib.folders.flatMap((f) => f.items));
		all = [...folderRows(null), ...rows().filter((r) => !inFolders.has(r.id))];
	}
	all = all.filter((r) => matchesFilter(r, libraryUi.filter, libraryUi.byYou));
	if (libraryUi.downloaded) all = all.filter((r) => isRowDownloaded(r)); // stream: downloads
	if (q) all = all.filter((r) => r.title.toLowerCase().includes(q) || r.creator.toLowerCase().includes(q));
	const sort = currentSort();
	const cmp: Record<string, (a: LibRow, b: LibRow) => number> = {
		recents: (a, b) => b.playedAt - a.playedAt || b.addedAt - a.addedAt,
		added: (a, b) => b.addedAt - a.addedAt,
		alpha: (a, b) => a.title.localeCompare(b.title),
		creator: (a, b) => a.creator.localeCompare(b.creator) || a.title.localeCompare(b.title),
		custom: (a, b) => a.position - b.position
	};
	all.sort(cmp[sort]);
	// Liked Songs always leads unpinned lists in Spotify only when pinned; otherwise it sorts.
	if (folder || q) return { pinned: [], rest: all };
	const pinOrder = lib.pinIds;
	const pinned = all.filter((r) => r.pinned).sort((a, b) => pinOrder.indexOf(a.id) - pinOrder.indexOf(b.id));
	return { pinned, rest: all.filter((r) => !r.pinned) };
}
