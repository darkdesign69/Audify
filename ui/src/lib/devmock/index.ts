// DEV ONLY: a fake backend so the SPA runs in a plain browser (`pnpm dev`) with real-looking data,
// for visual comparison against Spotify. Installed by `src/hooks.client.ts` only when
// `import.meta.env.DEV` and there is no real `__TAURI_INTERNALS__`; production builds drop it.
// Answers every `invoke` from the captured fixtures + a stateful library/player (`state.ts`), and
// emits the same events Rust does. Docs: docs/streams/devmock.md.
/* eslint-disable @typescript-eslint/no-explicit-any */
import { downloadHandlers } from './downloads'; // stream: downloads
import { mockIPC, mockWindows, mockConvertFileSrc } from '@tauri-apps/api/mocks';
import type { BrowseItem, SongItem, PlaylistEntry, PodcastEpisode, ShelfItem, Shelf } from '$lib/api';
import { episodeSong } from '$lib/api';
import { fx, get } from './fixtures';
import { mfyPage, mfyShelf, smart2Handlers } from './smart2'; // stream: smart2
import { importHandlers } from './importmock'; // stream: onboard
import { magicD3Handlers } from './magicd3'; // lane magic-d3
import { magicD1Handlers } from './magicd1'; // lane magic-d1
import { magicD2Handlers } from './magicd2'; // lane magic-d2
import { syncHandlers } from './syncmock'; // device sync UI
import {
	NOW,
	DEFAULT_REVERB,
	EQ_PRESETS,
	allEpisodes,
	audio,
	card,
	db,
	decorateEpisode,
	findSong,
	folderOf,
	lastPlayed,
	libPlaylist,
	newRow,
	nowSong,
	player,
	playlistById,
	podcasts,
	pool,
	poolSlice,
	secsOf,
	settings,
	smart,
	snapshot,
	type MockPlaylist
} from './state';

type Args = Record<string, any>;
const LIKED = 'VLLM';
const ON_REPEAT = 'LIMUSIC_ON_REPEAT';

// ── events ───────────────────────────────────────────────────────────────────────────────────
function emit(event: string, payload: unknown) {
	void (window as any).__TAURI_INTERNALS__.invoke('plugin:event|emit', { event, payload });
}
const changed = (scope: string) => emit('library-changed', { scope });
// ── stream: magic-d4 ── the core's Recents "touched" stamps and Home's `home-changed` `played`.
const touchedAt: Record<string, number> = {};
const deletedAt: Record<string, number> = {};
const touch = (itemId: string) => {
	const at = Math.floor(Date.now() / 1000);
	touchedAt[itemId] = at;
	emit('library-changed', { scope: 'touched', itemId, at });
};
const LOCAL_SHELVES = ['shortcuts', 'recently_played', 'jump_back_in', 'top_mixes'];
/** polish::context::started: Recents and Home move the moment a context starts. */
function contextStarted(ctx: { kind?: string; id?: string; title?: string; subtitle?: string; thumbnail?: string } | undefined) {
	if (!ctx?.id || !ctx.title || !ctx.kind || ctx.kind === 'song') return;
	const kind = ctx.id === LIKED ? 'liked' : ctx.kind;
	const card = { kind, id: ctx.id, title: ctx.title, subtitle: ctx.subtitle, thumbnail: ctx.thumbnail } as unknown as BrowseItem;
	db.recents = db.recents.filter((r) => r.itemId !== ctx.id);
	db.recents.unshift({ itemId: ctx.id, kind, card, playedAt: Math.floor(Date.now() / 1000) });
	changed('played');
	emit('home-changed', { reason: 'played', context: { ...card } });
}
// ── end stream: magic-d4 ──

function nowPlayingPayload(s: SongItem | undefined) {
	if (!s) return null;
	return {
		videoId: s.video_id,
		title: s.title,
		artists: s.artists,
		artistId: s.artist_id,
		artistRuns: s.artist_runs,
		thumbnail: s.thumbnail,
		duration: s.duration,
		album: s.album ?? null,
		streamClient: 'devmock',
		rating: db.liked.some((e) => e.song.video_id === s.video_id) ? 'like' : 'indifferent',
		isVideo: !!s.is_video,
		isEpisode: !!(s as any).is_episode,
		showsLyrics: !(s as any).is_episode, // as the core's `lyrics::offered`
		live: false
	};
}
const queuePayload = () => ({
	items: clone(player.items),
	currentIndex: player.current,
	shuffle: player.shuffle,
	repeat: player.repeat,
	sourceName: player.sourceName,
	sourceId: player.sourceId,
	prevTrack: null,
	autoplay: true,
	skipReasons: {}
});
const emitQueue = () => emit('queue-changed', queuePayload());
const emitIndex = () =>
	emit('queue-index', {
		currentIndex: player.current,
		shuffle: player.shuffle,
		repeat: player.repeat,
		sourceName: player.sourceName,
		sourceId: player.sourceId,
		prevTrack: null,
		current: player.items[player.current] ?? null,
		autoplay: true
	});
const duration = () => secsOf(nowSong()?.duration) || 240;

/** ── stream: instant ── What the core does on a play: the track shows at once with `loading`
 *  (state.rs `show_loading`), and plays once its stream resolves, which here is a fixed wait
 *  (`?resolveMs=` on the devmock URL, default 900). A newer play cancels a pending one. */
const RESOLVE_MS = Number(new URLSearchParams(globalThis.location?.search ?? '').get('resolveMs') ?? 900);
let loadGen = 0;
function startCurrent(position = 0) {
	const gen = ++loadGen;
	const np = nowPlayingPayload(nowSong());
	player.position = position;
	player.paused = true;
	if (np) emit('now-playing', { ...np, loading: true });
	emit('duration', { duration: duration() });
	emit('position', { position });
	setTimeout(() => {
		if (gen !== loadGen) return; // the last tap wins
		player.paused = false;
		emit('now-playing', nowPlayingPayload(nowSong()));
		emit('playback-state', 'playing');
	}, RESOLVE_MS);
}
function jump(index: number, position = 0) {
	if (!player.items.length) return;
	player.current = Math.max(0, Math.min(index, player.items.length - 1));
	emitIndex();
	startCurrent(position);
}
function next(auto = false) {
	if (auto && player.repeat === 'one') return startCurrent(0);
	if (player.current + 1 < player.items.length) return jump(player.current + 1);
	if (player.repeat === 'all') return jump(0);
	// Autoplay: append a few pool tracks, as radio continuation would.
	const more = poolSlice(Math.floor(Math.random() * pool.length), 5).map((s) => ({ ...s, autoplay: true }));
	player.items.push(...more);
	emit('queue-appended', { items: more, len: player.items.length, currentIndex: player.current });
	jump(player.current + 1);
}
/** Arguments as plain data: what a real Tauri invoke delivers after JSON serialisation. Binary
 *  payloads (file uploads) are passed through untouched. */
function plainArgs(args: unknown): unknown {
	if (!args || typeof args !== 'object' || args instanceof ArrayBuffer || ArrayBuffer.isView(args)) return args ?? {};
	try {
		return JSON.parse(JSON.stringify(args));
	} catch {
		return args;
	}
}

/** `structuredClone`, except a Svelte `$state` proxy (which it refuses with a DataCloneError) is
 *  copied through JSON. Lane magic-d1, MAGIC-GAPS #21. */
function clone<T>(v: T): T {
	try {
		return structuredClone(v);
	} catch {
		return JSON.parse(JSON.stringify(v));
	}
}

// A single pick (stream: autoplay): the song, then Autoplay rows; no context.
function singlePick(item: SongItem, from: string | null) {
	const auto = poolSlice(Math.floor(Math.random() * pool.length), 10)
		.filter((s) => s.video_id !== item.video_id)
		.map((s) => ({ ...s, autoplay: true }));
	replaceQueue([item, ...auto], 0, from, null);
}

function replaceQueue(items: SongItem[], start: number | null, name: string | null, id: string | null, shuffle?: boolean, startSecs?: number | null) {
	let list = items.map((s) => clone(s));
	let at = start ?? 0;
	if (shuffle) {
		const head = start != null ? list.splice(start, 1) : [];
		list.sort(() => Math.random() - 0.5);
		list = [...head, ...list];
		at = 0;
	}
	player.items = list;
	player.shuffle = !!shuffle || player.shuffle;
	player.sourceName = name;
	player.sourceId = id;
	player.current = Math.min(at, Math.max(0, list.length - 1));
	emitQueue();
	startCurrent(startSecs ?? 0);
}
/** Insert behind the playing row and any earlier "Play next" rows. */
function insertNext(items: SongItem[], from?: string) {
	let at = player.current + 1;
	while (player.items[at]?.queued && !player.items[at]?.queued_end) at++;
	player.items.splice(at, 0, ...items.map((s) => ({ ...clone(s), queued: true, queued_from: from })));
	emitQueue();
}
function appendQueued(items: SongItem[], from?: string) {
	let at = player.current + 1;
	while (player.items[at]?.queued) at++;
	player.items.splice(at, 0, ...items.map((s) => ({ ...clone(s), queued: true, queued_end: true, queued_from: from })));
	emitQueue();
}

let ticker: ReturnType<typeof setInterval> | undefined;
function startTicker() {
	if (ticker) return;
	ticker = setInterval(() => {
		if (player.paused || !nowSong()) return;
		player.position += 0.25 * player.speed;
		if (player.position >= duration()) return next(true);
		emit('position', { position: player.position });
	}, 250);
}

// ── page builders ────────────────────────────────────────────────────────────────────────────
const isLiked = (id: string) => db.liked.some((e) => e.song.video_id === id);
const withRating = (s: SongItem): SongItem => ({ ...s, rating: isLiked(s.video_id) ? 'like' : (s.rating ?? 'indifferent') });

function localPage(p: MockPlaylist) {
	const items = p.entries.map((e) => ({ ...withRating(e.song), set_video_id: String(e.rowId), added_at: e.addedAt }));
	return {
		title: p.name,
		subtitle: `${items.length} songs`,
		thumbnail: items[0]?.thumbnail,
		description: p.description || undefined,
		cover: p.cover ?? undefined,
		items,
		owned: true,
		collaborative: false
	};
}
function likedPage() {
	return {
		title: 'Liked Songs',
		subtitle: `${db.liked.length} songs`,
		items: db.liked.map((e) => ({ ...e.song, rating: 'like', added_at: e.addedAt })),
		owned: false,
		collaborative: false
	};
}
function bySlug(index: string, id: string, prefix: string, fallback: string) {
	const slug = fx[index]?.[id];
	if (!slug) warnOnce(`${prefix}:${id}`, `no ${prefix} fixture for ${id}; serving ${fallback}`);
	return get(`${prefix}_${slug ?? fallback}`);
}
function playlistPage(id: string) {
	if (id === LIKED) return likedPage();
	if (id.startsWith('MFY_')) {
		const p = mfyPage(id);
		if (!p) throw "This playlist isn't ready yet";
		return p;
	}
	if (id === ON_REPEAT)
		return { title: 'Your Rotation', subtitle: '25 songs', items: poolSlice(3, 25), owned: false, collaborative: false };
	if (id.startsWith('LOCALPLAYLIST:')) {
		const p = playlistById(id);
		if (!p) throw 'gone';
		return localPage(p);
	}
	const page = bySlug('playlist_index', id, 'playlist', 'rock');
	page.items = page.items.map(withRating);
	return page;
}
function albumPage(id: string) {
	const page = bySlug('album_index', id, 'album', 'discovery');
	page.items = page.items.map(withRating);
	page.inLibrary = db.saved.some((s) => s.id === id);
	return page;
}
function artistPage(id: string) {
	const page = bySlug('artist_index', id, 'artist', 'daft_punk');
	page.subscribed = db.saved.some((s) => s.id === id);
	page.topSongs = page.topSongs.map(withRating);
	return page;
}
const searchSlug = (q: string) => (/radio|ok comp/i.test(q ?? '') ? 'radiohead' : 'daft_punk');

const shelfItem = (c: BrowseItem): ShelfItem => ({
	kind: c.kind,
	id: c.id,
	title: c.title,
	subtitle: c.subtitle,
	thumbnail: c.thumbnail,
	explicit: !!c.explicit,
	song:
		c.kind === 'song'
			? { video_id: c.id, title: c.title, artists: c.subtitle ?? '', thumbnail: c.thumbnail, duration: c.duration, album_id: c.albumId, artist_runs: c.artistRuns }
			: undefined
});
function smartHome(): Shelf[] {
	const shelves: Shelf[] = [];
	const recentCards = db.recents.map((r) => r.card).filter(Boolean) as BrowseItem[];
	const shortcuts: ShelfItem[] = recentCards.slice(0, 8).map((c) =>
		c.id === LIKED ? { kind: 'liked', id: LIKED, title: 'Liked Songs', explicit: false } : shelfItem(c)
	);
	shelves.push({ id: 'shortcuts', title: 'Shortcuts', layout: 'shortcuts', source: 'local', items: shortcuts });
	const related: BrowseItem[] = [
		...(fx.artist_daft_punk?.sections?.find((s: any) => s.items[0]?.kind === 'artist')?.items ?? []),
		...(fx.artist_radiohead?.sections?.find((s: any) => s.items[0]?.kind === 'artist')?.items ?? [])
	];
	const mixes: ShelfItem[] = Array.from({ length: 6 }, (_, i) => {
		const a = related[i * 2];
		const b = related[i * 2 + 1];
		return {
			kind: 'daily_mix',
			id: `dailymix:${a?.id ?? i}`,
			title: `Your Mix ${i + 1}`,
			subtitle: `${[a?.title, b?.title].filter(Boolean).join(', ')} and more`,
			thumbnail: a?.thumbnail,
			explicit: false
		};
	});
	void mixes;
	shelves.push(mfyShelf()); // stream: smart2: the stored Made for you playlists
	shelves.push({ id: 'recently_played', title: 'Recently played', layout: 'cards', source: 'local', items: recentCards.map(shelfItem) });
	const home = fx.home;
	(home?.sections ?? []).forEach((s: any, i: number) => {
		shelves.push({
			id: `yt:${s.kind ?? i}`,
			title: s.title,
			layout: s.items.every((c: BrowseItem) => c.kind === 'song') ? 'songs' : 'cards',
			source: 'youtube',
			items: s.items.map(shelfItem)
		});
		if (i === 1 && fx.artist_daft_punk) {
			const more = fx.artist_daft_punk.sections.find((x: any) => x.items[0]?.kind === 'artist');
			if (more)
				shelves.push({
					id: `more_like:${fx.artist_daft_punk.channelId}`,
					title: 'More like Daft Punk',
					layout: 'cards',
					source: 'youtube',
					items: more.items.map(shelfItem)
				});
		}
	});
	shelves.push({
		id: 'jump_back_in',
		title: 'Jump back in',
		layout: 'cards',
		source: 'local',
		items: db.saved.filter((s) => s.kind === 'album').map((s) => shelfItem(s.card))
	});
	const nr = fx.new_releases?.sections?.[0];
	if (nr) shelves.push({ id: 'new_releases', title: 'New releases for you', layout: 'cards', source: 'youtube', items: nr.items.map(shelfItem) });
	// The core's moods shelf (smart::home): YouTube's tiles, each with its cached cover.
	const moods = (fx.moods ?? []).flatMap((s: any) => s.items).slice(0, 24);
	if (moods.length)
		shelves.push({
			id: 'moods',
			title: 'Moods & genres',
			layout: 'moods',
			source: 'youtube',
			items: moods.map((m: any) => ({
				kind: 'mood',
				id: m.params,
				title: m.title,
				params: m.params,
				color: m.color,
				thumbnail: fx.mood_art?.[m.params],
				explicit: false
			}))
		});
	return clone(shelves);
}

// ── stream: home-tiles-pods ── the core's Podcasts chip (smart::podhome) from the podcast fixtures.
const POD_CATEGORIES: [string, string, string][] = [
	['1303', 'Comedy', '#be123c'], ['1489', 'News', '#15803d'], ['1488', 'True Crime', '#9d2bb0'],
	['1324', 'Society & Culture', '#0e6ba8'], ['1545', 'Sports', '#a16207'], ['1321', 'Business', '#3f7d20'],
	['1512', 'Health & Fitness', '#b0279f'], ['1487', 'History', '#d6246e'], ['1533', 'Science', '#0b7a75'],
	['1318', 'Technology', '#6a2fd8'], ['1309', 'TV & Film', '#1f4fa8'], ['1304', 'Education', '#c2410c']
];
const epItem = (e: PodcastEpisode): ShelfItem => ({
	kind: 'episode', id: e.id, title: e.title, subtitle: e.showTitle, thumbnail: e.thumbnail ?? undefined,
	song: episodeSong(e), params: e.showId ?? undefined, explicit: false
});
const showItem = (x: any): ShelfItem => ({ kind: 'podcast', id: x.id, title: x.title, subtitle: x.author, thumbnail: x.thumbnail, explicit: false });
function podcastHome(): Shelf[] {
	const r = get('podcast_search_huberman') ?? { shows: [], episodes: [] };
	const eps = allEpisodes();
	const started = eps.filter((e) => e.progress.fraction > 0 && !e.progress.played);
	const fresh = eps.filter((e) => !e.progress.played);
	const tiles = [...started.map((e) => ({ ...epItem(e), progress: e.progress.fraction })), ...fresh.map((e) => ({ ...epItem(e), newEpisode: true }))]
		.filter((t, i, a) => a.findIndex((x) => x.id === t.id) === i).slice(0, 8);
	const followed = new Set(podcasts.follows.map((f) => f.title));
	const shows = (r.shows as any[]).filter((x) => !followed.has(x.title));
	const shelf = (id: string, title: string, items: ShelfItem[], layout: Shelf['layout'] = 'cards'): Shelf =>
		({ id, title, layout, source: id === 'shortcuts' || id === 'pod_new' || id === 'pod_shows' ? 'local' : 'apple', items });
	return [
		shelf('shortcuts', '', tiles, 'shortcuts'),
		shelf('pod_new', 'New episodes', fresh.slice(0, 12).map(epItem)),
		shelf('pod_for_you', 'Episodes for you', (r.episodes as PodcastEpisode[]).map(decorateEpisode).slice(0, 12).map(epItem)),
		shelf('pod_shows', 'Your shows', podcasts.follows.map(showItem)),
		shelf('pod_like', 'Shows you might like', shows.slice(0, 10).map(showItem)),
		shelf('pod_because', `Because you listened to ${podcasts.follows[0]?.title ?? 'Huberman Lab'}`, shows.slice(5, 15).map(showItem)),
		shelf('pod_top', 'Top podcasts', [...shows].reverse().map(showItem)),
		shelf('pod_categories', 'Podcast categories', POD_CATEGORIES.map(([id, title, color], i) => ({
			kind: 'podcast_category', id: `podcat:${id}`, title, params: id, color, thumbnail: shows[i % Math.max(shows.length, 1)]?.thumbnail, explicit: false
		})), 'moods')
	].filter((s) => s.items.length);
}

function listeningStats() {
	const top = pool.slice(0, 50);
	const topSongs = top.slice(0, 20).map((song, i) => ({ song, plays: 40 - i * 2 + (i % 3), secs: (40 - i * 2) * secsOf(song.duration) }));
	const artists = new Map<string, { name: string; id: string | null; thumbnail: string | null; plays: number; secs: number }>();
	const albums = new Map<string, { id: string; title: string; artist: string; thumbnail: string | null; plays: number; secs: number }>();
	for (const t of topSongs) {
		const name = t.song.artists.split(',')[0];
		const a = artists.get(t.song.artist_id ?? name) ?? { name, id: t.song.artist_id ?? null, thumbnail: t.song.thumbnail ?? null, plays: 0, secs: 0 };
		a.plays += t.plays;
		a.secs += t.secs;
		artists.set(t.song.artist_id ?? name, a);
		if (t.song.album_id) {
			const al = albums.get(t.song.album_id) ?? { id: t.song.album_id, title: t.song.album ?? '', artist: name, thumbnail: t.song.thumbnail ?? null, plays: 0, secs: 0 };
			al.plays += t.plays;
			al.secs += t.secs;
			albums.set(t.song.album_id, al);
		}
	}
	const days: [string, number][] = Array.from({ length: 30 }, (_, i) => {
		const d = new Date((NOW - (29 - i) * 86400) * 1000);
		return [d.toISOString().slice(0, 10), 1800 + ((i * 7919) % 5400)];
	});
	const secs = days.reduce((a, [, s]) => a + s, 0);
	return {
		secs,
		prevSecs: Math.round(secs * 0.83),
		plays: topSongs.reduce((a, t) => a + t.plays, 0),
		songs: top.length,
		newSongs: 17,
		artists: artists.size,
		days,
		hours: Array.from({ length: 168 }, (_, i) => ((i % 24) > 7 ? ((i * 31) % 900) : 0)),
		topSongs,
		topArtists: [...artists.values()].sort((a, b) => b.plays - a.plays),
		topAlbums: [...albums.values()].sort((a, b) => b.plays - a.plays),
		topPodcasts: [],
		podcastSecs: 0,
		episodes: 0,
		firstAt: NOW - 120 * 86400,
		streak: 12,
		bestStreak: 31
	};
}

// ── library ops ──────────────────────────────────────────────────────────────────────────────
function plAdd(id: string, songs: SongItem[], allowDuplicates = false, position?: number) {
	const p = playlistById(id);
	if (!p) throw 'gone';
	const added: PlaylistEntry[] = [];
	const duplicates: string[] = [];
	const at = position ?? p.entries.length;
	const fresh: typeof p.entries = [];
	for (const song of songs) {
		if (!allowDuplicates && p.entries.some((e) => e.song.video_id === song.video_id)) {
			duplicates.push(song.video_id);
			continue;
		}
		fresh.push({ rowId: newRow(), addedAt: Math.floor(Date.now() / 1000), song: clone(song) });
	}
	p.entries.splice(at, 0, ...fresh);
	fresh.forEach((e, i) => added.push({ rowId: e.rowId, position: at + i, addedAt: e.addedAt, song: e.song }));
	p.updatedAt = Math.floor(Date.now() / 1000);
	changed('playlists');
	return { added, duplicates };
}
function plRemove(id: string, rowIds: number[]): PlaylistEntry[] {
	const p = playlistById(id);
	if (!p) throw 'gone';
	const out: PlaylistEntry[] = [];
	p.entries = p.entries.filter((e, i) => {
		if (!rowIds.includes(e.rowId)) return true;
		out.push({ rowId: e.rowId, position: i, addedAt: e.addedAt, song: e.song });
		return false;
	});
	changed('playlists');
	return out;
}
function plMove(id: string, rowIds: number[], before?: number | null): number[] {
	const p = playlistById(id);
	if (!p) throw 'gone';
	const prev = p.entries.map((e) => e.rowId);
	const moving = p.entries.filter((e) => rowIds.includes(e.rowId));
	const rest = p.entries.filter((e) => !rowIds.includes(e.rowId));
	const at = before == null ? rest.length : Math.max(0, rest.findIndex((e) => e.rowId === before));
	rest.splice(at === -1 ? rest.length : at, 0, ...moving);
	p.entries = rest;
	changed('playlists');
	return prev;
}
function plSetOrder(id: string, rowIds: number[]): number[] {
	const p = playlistById(id);
	if (!p) throw 'gone';
	const prev = p.entries.map((e) => e.rowId);
	p.entries = rowIds.map((r) => p.entries.find((e) => e.rowId === r)!).filter(Boolean);
	changed('playlists');
	return prev;
}
function plCreate(name?: string, description?: string, folderId?: string) {
	const n = db.nextPlaylist++;
	const own = db.playlists.filter((p) => /^My Playlist #\d+$/.test(p.name)).length;
	const p: MockPlaylist = {
		id: `LOCALPLAYLIST:${n}`,
		name: name?.trim() || `My Playlist #${own + 1}`,
		description: description ?? '',
		cover: null,
		createdAt: Math.floor(Date.now() / 1000),
		updatedAt: Math.floor(Date.now() / 1000),
		entries: []
	};
	db.playlists.push(p);
	if (folderId) db.folders.find((f) => f.id === folderId)?.items.push(p.id);
	changed('playlists');
	return p;
}
function setItemFolder(itemId: string, folderId?: string | null, index?: number) {
	const from = db.folders.find((f) => f.items.includes(itemId));
	const prev = from ? { folderId: from.id, index: from.items.indexOf(itemId) } : null;
	if (from) from.items = from.items.filter((x) => x !== itemId);
	const to = folderId ? db.folders.find((f) => f.id === folderId) : null;
	if (to) to.items.splice(index ?? to.items.length, 0, itemId);
	changed('folders');
	changed('playlists');
	return prev;
}
function pin(itemId: string, kind?: string, index?: number) {
	db.pins = db.pins.filter((p) => p.itemId !== itemId);
	if (db.pins.length >= 4) throw 'pins_full';
	db.pins.splice(index ?? db.pins.length, 0, { itemId, kind: (kind ?? 'playlist') as any, position: 0 });
	db.pins.forEach((p, i) => (p.position = i));
	changed('pins');
	return clone(db.pins);
}

// ── stream: downloads ── a fake download queue (devmock/downloads.ts).
const dl = downloadHandlers(emit, (c) => {
	if (c.kind === 'liked' || c.id === LIKED) return db.liked.map((e) => e.song);
	if (c.kind === 'album') return ((albumPage(c.id) as any)?.items ?? []) as SongItem[];
	return ((playlistPage(c.id) as any)?.items ?? []) as SongItem[];
});

// ── stream: search2 ──
const isMash = (q: string) => /^[^aeiouy]{6,}$/i.test(q.replace(/[\s\d]/g, '')) || /^(.)\1{5,}$/.test(q.replace(/\s/g, ''));
function podsFor(q: string): { shows: any[]; episodes: any[] } {
	const r = /chris williamson|modern wisdom/i.test(q) ? (get('podcast_search_chris_williamson') ?? get('podcast_search_huberman')) : get('podcast_search_huberman');
	return {
		shows: (r.shows ?? []).map((s: any) => ({ ...s, followed: podcasts.follows.some((f) => f.id === s.id) })),
		episodes: (r.episodes ?? []).map(decorateEpisode)
	};
}

// ── the command table ────────────────────────────────────────────────────────────────────────
const handlers: Record<string, (a: Args) => unknown> = {
	...dl.handlers, // stream: downloads
	// browse
	get_home: (a) => get(a.params ? 'home_chip' : 'home'),
	get_home_more: () => ({ chips: [], sections: [] }),
	get_live_now: () => [],
	smart_home: smartHome,
	podcast_home: podcastHome, // stream: home-tiles-pods
	podcast_category: () => ((get('podcast_search_huberman')?.shows ?? []) as any[]).map(showItem),
	smart_home_local: () => smartHome().filter((s) => LOCAL_SHELVES.includes(s.id)), // stream: magic-d4
	...smart2Handlers(emit, () => emitQueue()), // stream: smart2
	...magicD3Handlers(emit, () => emitQueue()), // lane magic-d3
	...magicD1Handlers((item, from) => singlePick(item, from)), // lane magic-d1
	...magicD2Handlers(() => emitQueue()), // lane magic-d2
	local_user_name: () => 'Alex', // stream: parity3 (the Mac user's first name)
	smart_mix: (a) => poolSlice(Math.abs([...String(a.id)].reduce((h, c) => h * 31 + c.charCodeAt(0), 7)) % pool.length, a.size ?? 50),
	search: (a) => get(searchSlug(a.query) === 'radiohead' ? 'artist_radiohead' : 'search_songs_daft_punk')?.topSongs ?? get('search_songs_daft_punk'),
	search_videos: () => get('search_videos_daft_punk'),
	search_all: (a) => get(`search_all_${searchSlug(a.query)}`),
	search_blend_artist_top: (a) => a.songs, // stream: podtop — the order stays as searched
	search_cards: (a) => get(`search_cards_daft_punk_${a.category}`) ?? [],
	search_suggestions: () => get('search_suggestions_daft_p'),
	// ── stream: search2 ── stand-ins for `search_overview` / `search_page` (crates/core/src/search.rs):
	// "chris williamson" finds Modern Wisdom as the Top result, a typo gets "Showing results for", a
	// keyboard mash gets "Try these", and the Songs chip pages 20 rows at a time up to 160.
	search_overview: (a) => {
		const q = String(a.query ?? '').trim();
		const empty = { top: [], songs: [], albums: [], artists: [], playlists: [] };
		const pods = podsFor(q);
		if (/chris williamson/i.test(q)) {
			return {
				query: q,
				results: { ...empty, artists: [{ kind: 'artist', id: 'UC7DGghCa1lDAGnT1EPvTpqg', title: 'Chris Williamson', subtitle: 'Artist • 5 subscribers' }] },
				songs: [],
				songsContinuation: undefined,
				shows: pods.shows,
				episodes: pods.episodes,
				podsPending: false,
				topShow: 0,
				podcastsFirst: true
			};
		}
		if (isMash(q)) {
			return {
				query: q,
				results: empty,
				songs: [],
				shows: [],
				episodes: [],
				podsPending: false,
				podcastsFirst: false,
				tryThese: { songs: pool.slice(0, 8), artists: (get('search_all_daft_punk')?.artists ?? []).slice(0, 4) }
			};
		}
		const corrected = /pnuk|daft pun\b/i.test(q) ? 'daft punk' : q;
		const songs = handlers.search({ query: corrected }) as SongItem[];
		return {
			query: corrected,
			correction: corrected !== q ? { kind: 'spelling', query: corrected, original: q } : undefined,
			results: get(`search_all_${searchSlug(corrected)}`),
			songs: songs.slice(0, 20),
			songsContinuation: 'p1',
			shows: pods.shows,
			episodes: pods.episodes,
			podsPending: false,
			podcastsFirst: false
		};
	},
	search_lyrics: () => [], // stream: search2 — no lyric matches in the preview
	search_blend_lyrics: (a) => ({ songs: a.songs, leadTop: false, added: [] }),
	search_page: (a) => {
		const q = String(a.query ?? '').trim();
		const n = a.continuation ? Number(String(a.continuation).slice(1)) : 0;
		const base = { filter: a.filter, query: q, songs: [], items: [], shows: [], episodes: [] };
		if (isMash(q)) return { ...base, tryThese: { songs: pool.slice(0, 8), artists: [] } };
		if (a.filter === 'songs' || a.filter === 'videos') {
			const src = (a.filter === 'songs' ? (handlers.search({ query: q }) as SongItem[]) : (get('search_videos_daft_punk') as SongItem[])) ?? [];
			const all = [...src, ...pool];
			const rows = Array.from({ length: 20 }, (_, i) => {
				const s = all[(n * 20 + i) % all.length];
				return n * 20 + i < all.length ? s : { ...s, video_id: `${s.video_id}~${n}-${i}` };
			});
			return { ...base, songs: rows, continuation: n < 7 ? `p${n + 1}` : undefined };
		}
		if (a.filter === 'podcasts') return { ...base, shows: podsFor(q).shows };
		if (a.filter === 'episodes') return { ...base, episodes: podsFor(q).episodes };
		if (a.filter === 'profiles')
			return { ...base, items: [{ kind: 'artist', id: 'UCuY9UGOYjCnZoaWvj7FAU-g', title: q || 'Profile', subtitle: 'Profile • @profile' }] };
		return { ...base, items: get(`search_cards_daft_punk_${a.filter}`) ?? [] };
	},
	song: (a) => findSong(a.videoId) ?? pool[0],
	get_album: (a) => albumPage(a.id),
	get_artist: (a) => artistPage(a.id),
	// The preview has no core: every release as an album, in the order given (the real rules are
	// `crates/core/src/discography.rs`).
	discography: (a) => {
		const rels = [...(a.albums ?? []), ...(a.singles ?? [])].map((item: any, order: number) => {
			const year = Number(/\b(19|20)\d{2}\b/.exec(item.subtitle ?? '')?.[0] ?? 0);
			return { item, type: 'album', year, order, line: year ? `${year} • Album` : 'Album', meta: 'Album' };
		});
		return { popular: rels, all: rels, album: rels, single: [], compilation: [] };
	},
	get_artist_appears_on: (a) => get(`artist_appears_on_${fx.artist_index?.[a.id] ?? 'daft_punk'}`) ?? [],
	get_playlist: (a) => playlistPage(a.id),
	get_playlist_more: () => ({ items: [] }),
	get_playlist_suggestions: () => ({ items: poolSlice(70, 10) }),
	playlist_recommendations: (a) =>
		poolSlice(40 + (a.refresh ?? 0) * 10, a.n ?? 10).map((song) => ({ song, votes: 3, because: [], basis: 'playlist' })),
	get_moods: () => get('moods'),
	get_mood_art: (a) => Object.fromEntries((a.params as string[]).filter((p) => fx.mood_art?.[p]).map((p) => [p, fx.mood_art[p]])),
	get_browse_grid: () => get('genre_grid'),
	get_browse_sections: (a) => {
		const page = get('genre_sections');
		const tile = (fx.moods ?? []).flatMap((s: any) => s.items).find((t: any) => t.params === a.params);
		return { ...page, title: tile?.title ?? fx.genre_meta?.title ?? page.title };
	},
	get_charts: () => get('charts'),
	get_new_releases: () => get('new_releases'),
	get_library: () => db.playlists.filter((p) => !p.deleted).map((p) => card('playlist', p.id, p.name, `${p.entries.length} songs`, p.entries[0]?.song.thumbnail)),
	local_playlists: () => db.playlists.filter((p) => !p.deleted).map((p) => card('playlist', p.id, p.name, `${p.entries.length} songs`, p.entries[0]?.song.thumbnail)),
	get_library_albums: () => db.saved.filter((s) => s.kind === 'album').map((s) => s.card),
	get_library_artists: () => db.saved.filter((s) => s.kind === 'artist').map((s) => s.card),
	get_library_subscriptions: () => [],
	get_history: () => [
		{ title: 'Today', items: poolSlice(0, 8) },
		{ title: 'Yesterday', items: poolSlice(20, 10) }
	],
	play_counts: () => Object.fromEntries(pool.slice(0, 30).map((s, i) => [s.video_id, 30 - i])),
	listening_stats: listeningStats,
	top_artists: (a) => listeningStats().topArtists.slice(0, a.limit ?? 10),
	clear_listening_history: () => null,
	playlist_index: () => {
		const map: Record<string, string[]> = {};
		for (const p of db.playlists) for (const e of p.entries) (map[e.song.video_id] ??= []).push(p.id);
		return map;
	},
	sync_playlist_index: () => handlers.playlist_index({}),
	readonly_playlists: () => [],
	recent_play_contexts: (a) =>
		db.recents.slice(0, a.limit ?? 50).map((r, i) => ({
			kind: r.kind,
			id: r.itemId,
			title: r.card?.title ?? '',
			subtitle: r.card?.subtitle,
			thumbnail: r.card?.thumbnail,
			plays: 10 - Math.min(i, 9),
			playedAt: r.playedAt
		})),

	// player
	get_queue: queuePayload,
	get_playback: () => ({
		speed: player.speed,
		semitones: player.semitones,
		reverb: player.reverb,
		now: nowPlayingPayload(nowSong()),
		paused: player.paused,
		position: player.position,
		duration: duration(),
		volume: player.volume
	}),
	// A single pick (stream: autoplay): the song, then Autoplay rows; no context.
	play: (a) => singlePick(a.item, a.from ?? null),
	play_index: (a) => jump(a.index, a.start ?? 0),
	play_playlist: (a) => {
		const r = replaceQueue(a.items, a.start, a.sourceName ?? a.context?.title ?? null, a.sourceId ?? a.context?.id ?? null, a.shuffle, a.startSecs);
		contextStarted(a.context); // stream: magic-d4
		return r;
	},
	start_radio: (a) => {
		const seed = a.kind === 'song' ? findSong(a.id) : undefined;
		const title = `${a.name ?? seed?.title ?? 'Song'} Radio`;
		replaceQueue([...(seed ? [seed] : []), ...poolSlice(Math.floor(Math.random() * pool.length), 25)], 0, title, null);
		// lane magic-d6: api.rs start_radio records the radio as a context (artist → "mix").
		contextStarted({ kind: a.kind === 'artist' ? 'mix' : 'radio', id: a.id, title, thumbnail: seed?.thumbnail ?? undefined });
	},
	play_next: (a) => insertNext(a.items, a.from),
	add_to_queue: (a) => appendQueued(a.items, a.from),
	remove_from_queue: (a) => {
		player.items.splice(a.index, 1);
		emitQueue();
	},
	move_in_queue: (a) => {
		const [row] = player.items.splice(a.from, 1);
		player.items.splice(a.to, 0, row);
		emitQueue();
	},
	clear_queued: () => {
		player.items = player.items.filter((s, i) => i <= player.current || !(s.queued || s.queued_end));
		emitQueue();
	},
	next_track: () => next(),
	prev_track: () => (player.position > 3 ? startCurrent(0) : jump(player.current - 1)),
	back_to_previous: () => null,
	toggle_shuffle: () => {
		player.shuffle = !player.shuffle;
		emitIndex();
	},
	set_repeat: (a) => {
		player.repeat = a.mode;
		emitIndex();
	},
	toggle_pause: () => {
		player.paused = !player.paused;
		emit('playback-state', player.paused ? 'paused' : 'playing');
	},
	seek: (a) => {
		player.position = Math.max(0, Math.min(a.position, duration()));
		emit('position', { position: player.position });
	},
	set_volume: (a) => {
		player.volume = a.volume;
		emit('volume', a.volume);
	},
	set_playback_params: (a) => {
		player.speed = a.speed;
		player.semitones = a.semitones;
		player.reverb = a.reverb ?? DEFAULT_REVERB;
		emit('playback-params', { speed: a.speed, semitones: a.semitones, reverb: player.reverb });
	},
	switch_version: () => null,
	video_stream: () => null,
	forget_video_stream: () => null,
	playback_clock: () => ({ position: player.position, atMs: Date.now(), speed: player.speed, paused: player.paused }),
	playback_skip_reasons: () => ({}),
	playback_health: () => ({ status: 'ok', ok: true, client: 'devmock', videoId: pool[0]?.video_id, elapsedMs: 420, checkedAt: Math.floor(Date.now() / 1000) }),
	explicit_flags: (a) => Object.fromEntries((a.items as SongItem[]).map((s) => [s.video_id, !!s.explicit])),
	playable_flags: (a) => Object.fromEntries((a.items as SongItem[]).map((s) => [s.video_id, true])),
	taste_profile: () => ({ artists: [], tracks: [] }),

	// lyrics
	get_lyrics: () => get('lyrics_one_more_time') ?? null,
	choose_lyrics_source: () => get('lyrics_one_more_time') ?? null,
	set_lyrics_offset: () => null,
	lyrics_parse: () => [],
	lyrics_providers: () =>
		['youtube', 'lrclib', 'netease', 'qq', 'kugou', 'unison', 'boidu', 'lyricsplus'].map((id) => ({
			id,
			name: { lrclib: 'LRCLIB', youtube: 'YouTube Music', qq: 'QQ Music', netease: 'NetEase', kugou: 'KuGou', lyricsplus: 'LyricsPlus', unison: 'Unison', boidu: 'Boidu' }[id] ?? id,
			on: true
		})),

	// writes (YouTube-side ones answer as if they worked)
	rate: (a) => {
		if (a.rating === 'like') handlers.lib_like({ songs: [findSong(a.videoId) ?? { video_id: a.videoId, title: '', artists: '' }] });
		else handlers.lib_unlike({ videoIds: [a.videoId] });
		emit('rating', { videoId: a.videoId, rating: a.rating });
	},
	add_to_playlist: (a) => {
		if (!a.playlistId.startsWith('LOCALPLAYLIST:')) return true;
		return plAdd(a.playlistId, [findSong(a.videoId)!].filter(Boolean), a.allowDuplicates).added.length > 0;
	},
	add_to_local_playlist: (a) => {
		const r = plAdd(a.playlistId, a.items, false);
		return (a.items as SongItem[]).map((s) => !r.duplicates.includes(s.video_id));
	},
	remove_from_playlist: (a) => (a.playlistId === LIKED ? handlers.lib_unlike({ videoIds: [a.videoId] }) : plRemove(a.playlistId, [Number(a.setVideoId)]), null),
	remove_many_from_playlist: (a) => (plRemove(a.playlistId, (a.tracks as [string, string][]).map(([, r]) => Number(r))), null),
	move_in_playlist: (a) => (plMove(a.playlistId, [Number(a.setVideoId)], a.beforeSetVideoId == null ? null : Number(a.beforeSetVideoId)), null),
	create_playlist: (a) => plCreate(a.title).id,
	edit_playlist_details: (a) => {
		const p = playlistById(a.playlistId);
		if (p) {
			if (a.name != null) p.name = a.name;
			if (a.description != null) p.description = a.description;
			changed('playlists');
		}
	},
	set_playlist_cover: () => ({}),
	delete_playlist: (a) => handlers.lib_playlist_delete({ id: a.playlistId }),
	set_playlist_sort: () => null,
	subscribe: (a) => {
		if (a.subscribed) {
			const slug = fx.artist_index?.[a.channelId];
			const page = slug ? fx[`artist_${slug}`] : null;
			handlers.lib_save({ card: card('artist', a.channelId, page?.name ?? 'Artist', 'Artist', page?.thumbnail) });
		} else handlers.lib_unsave({ id: a.channelId });
	},
	set_song_saved: () => null,
	set_album_saved: () => null,

	// library (library.rs)
	lib_snapshot: () => ({ ...snapshot(), lastTouched: { ...touchedAt } }), // stream: magic-d4
	lib_liked_ids: () => db.liked.map((e) => e.song.video_id),
	lib_liked_songs: () => clone(db.liked),
	lib_like: (a) => {
		const out = (a.songs as SongItem[]).map((s) => {
			if (isLiked(s.video_id)) return false;
			db.liked.unshift({ song: clone(s), addedAt: Math.floor(Date.now() / 1000) });
			return true;
		});
		changed('liked');
		if (out.some(Boolean)) touch(LIKED); // stream: magic-d4
		return out;
	},
	lib_unlike: (a) => {
		const removed = db.liked.filter((e) => a.videoIds.includes(e.song.video_id));
		db.liked = db.liked.filter((e) => !a.videoIds.includes(e.song.video_id));
		changed('liked');
		return removed;
	},
	lib_restore_likes: (a) => {
		db.liked.push(...a.entries);
		db.liked.sort((x, y) => y.addedAt - x.addedAt);
		changed('liked');
	},
	lib_saved: (a) => clone(db.saved.filter((s) => !a.kind || s.kind === a.kind)),
	lib_save: (a) => {
		if (db.saved.some((s) => s.id === a.card.id)) return false;
		const c = a.card as BrowseItem;
		db.saved.unshift({ id: c.id, kind: c.kind as any, title: c.title, subtitle: c.subtitle ?? null, thumbnail: c.thumbnail ?? null, card: c, synced: !!a.synced, addedAt: Math.floor(Date.now() / 1000) });
		changed('saved');
		return true;
	},
	lib_unsave: (a) => {
		const s = db.saved.find((x) => x.id === a.id) ?? null;
		db.saved = db.saved.filter((x) => x.id !== a.id);
		changed('saved');
		return s;
	},
	lib_restore_saved: (a) => {
		db.saved.unshift(a.item);
		changed('saved');
	},
	lib_mark_synced: () => null,
	lib_playlists: () => db.playlists.filter((p) => !p.deleted).map(libPlaylist),
	lib_playlist_create: (a) => libPlaylist(plCreate(a.name, a.description, a.folderId)),
	lib_playlist_update: (a) => {
		const p = playlistById(a.id);
		if (!p) throw 'gone';
		if (a.name != null) p.name = a.name;
		if (a.description != null) p.description = a.description;
		changed('playlists');
		touch(a.id); // stream: magic-d4
		return libPlaylist(p);
	},
	lib_playlist_delete: (a) => {
		const p = playlistById(a.id);
		if (p) p.deleted = true;
		deletedAt[a.id] = Math.floor(Date.now() / 1000); // stream: magic-d4
		db.pins = db.pins.filter((x) => x.itemId !== a.id);
		changed('playlists');
	},
	lib_playlist_restore: (a) => {
		const p = db.playlists.find((x) => x.id === a.id);
		if (!p) throw 'gone';
		p.deleted = false;
		delete deletedAt[a.id];
		changed('playlists');
		touch(a.id); // stream: magic-d4
		return libPlaylist(p);
	},
	// stream: magic-d4 — Settings › Recover playlists.
	lib_deleted_playlists: () =>
		db.playlists
			.filter((p) => p.deleted)
			.map((p) => {
				const at = deletedAt[p.id] ?? Math.floor(Date.now() / 1000);
				return { id: p.id, name: p.name, count: p.entries.length, thumbnail: p.entries[0]?.song.thumbnail ?? null, deletedAt: at, purgeAt: at + 30 * 86_400 };
			})
			.sort((x, y) => y.deletedAt - x.deletedAt),
	lib_playlist_entries: (a) => (playlistById(a.id)?.entries ?? []).map((e, i) => ({ rowId: e.rowId, position: i, addedAt: e.addedAt, song: e.song })),
	lib_playlist_duplicates: (a) => (a.videoIds as string[]).filter((v) => playlistById(a.id)?.entries.some((e) => e.song.video_id === v)),
	lib_playlist_add: (a) => {
		const r = plAdd(a.id, a.songs, a.allowDuplicates, a.position);
		touch(a.id); // stream: magic-d4
		return r;
	},
	lib_playlist_remove: (a) => plRemove(a.id, a.rowIds),
	lib_playlist_insert_entries: (a) => {
		const p = playlistById(a.id);
		if (!p) throw 'gone';
		for (const e of [...a.entries].sort((x: PlaylistEntry, y: PlaylistEntry) => x.position - y.position))
			p.entries.splice(e.position, 0, { rowId: e.rowId, addedAt: e.addedAt, song: e.song });
		changed('playlists');
	},
	lib_playlist_move: (a) => plMove(a.id, a.rowIds, a.before),
	lib_playlist_set_order: (a) => plSetOrder(a.id, a.rowIds),
	lib_folders: () => clone(db.folders),
	lib_folder_create: (a) => {
		const now = Math.floor(Date.now() / 1000);
		const f = { id: `FOLDER:${db.nextFolder++}`, name: a.name?.trim() || 'New Folder', parentId: a.parentId ?? null, position: db.folders.length, createdAt: now, updatedAt: now, items: [] };
		db.folders.push(f);
		changed('folders');
		return f;
	},
	lib_folder_rename: (a) => {
		const f = db.folders.find((x) => x.id === a.id);
		const old = f?.name ?? '';
		if (f) f.name = a.name;
		changed('folders');
		return old;
	},
	lib_folder_move: (a) => {
		const f = db.folders.find((x) => x.id === a.id);
		const prev = { folderId: f?.parentId ?? null, index: f?.position ?? 0 };
		if (f) {
			f.parentId = a.parentId ?? null;
			f.position = a.index ?? f.position;
		}
		changed('folders');
		return prev;
	},
	lib_folder_delete: (a) => {
		const f = db.folders.find((x) => x.id === a.id);
		if (!f) throw 'gone';
		db.folders = db.folders.filter((x) => x.id !== a.id);
		changed('folders');
		changed('playlists');
		return { folder: f, childFolders: [], items: f.items.map((id, i) => [id, i]) };
	},
	lib_folder_restore: (a) => {
		const f = { ...a.snapshot.folder, items: (a.snapshot.items as [string, number][]).map(([id]) => id) };
		db.folders.push(f);
		changed('folders');
		changed('playlists');
		return f;
	},
	lib_folder_set_item: (a) => setItemFolder(a.itemId, a.folderId, a.index),
	lib_pins: () => clone(db.pins),
	lib_pin: (a) => pin(a.itemId, a.kind, a.index),
	lib_unpin: (a) => {
		const at = db.pins.findIndex((p) => p.itemId === a.itemId);
		db.pins = db.pins.filter((p) => p.itemId !== a.itemId);
		db.pins.forEach((p, i) => (p.position = i));
		changed('pins');
		return at < 0 ? null : at;
	},
	lib_pins_reorder: (a) => {
		db.pins = (a.itemIds as string[]).map((id, i) => ({ ...db.pins.find((p) => p.itemId === id)!, position: i })).filter((p) => p.itemId);
		changed('pins');
		return clone(db.pins);
	},
	lib_note_played: (a) => {
		db.recents = db.recents.filter((r) => r.itemId !== a.itemId);
		db.recents.unshift({ itemId: a.itemId, kind: a.kind ?? 'playlist', card: a.card ?? null, playedAt: Math.floor(Date.now() / 1000) });
		changed('played');
	},
	lib_recent_contexts: (a) => clone(db.recents.slice(0, a.limit ?? 50)),
	lib_recent_tracks: (a) => poolSlice(0, a.limit ?? 50).map((song, i) => ({ song, playedAt: NOW - i * 1800, plays: 1 + (i % 4) })),
	lib_last_played: lastPlayed,
	lib_hidden_songs: (a) => db.hidden.filter((h) => (a.contextId ? h.contextId === a.contextId : true)),
	lib_hide_song: (a) => {
		db.hidden.push({ videoId: a.videoId, contextId: a.contextId ?? null, song: a.song ?? null, hiddenAt: Math.floor(Date.now() / 1000) });
		changed('hidden');
		return true;
	},
	lib_unhide_song: (a) => {
		const h = db.hidden.find((x) => x.videoId === a.videoId && (x.contextId ?? null) === (a.contextId ?? null)) ?? null;
		db.hidden = db.hidden.filter((x) => x !== h);
		changed('hidden');
		return h;
	},
	lib_private_session: () => ({ ...db.privateSession }),
	lib_set_private_session: (a) => {
		db.privateSession = a.enabled ? { enabled: true, since: Math.floor(Date.now() / 1000), expiresAt: Math.floor(Date.now() / 1000) + 6 * 3600 } : { enabled: false };
		changed('private');
		return { ...db.privateSession };
	},
	lib_migrate_legacy: () => ({ already: true, saved: 0, pins: 0, recent: 0 }),
	lib_export: () => ({ path: '~/Downloads/Spotless Library.zip', likedSongs: db.liked.length, playlists: db.playlists.length, files: [] }),

	// podcasts
	podcast_search: () => {
		const r = get('podcast_search_huberman');
		return {
			shows: (r.shows ?? []).map((s: any) => ({ ...s, followed: podcasts.follows.some((f) => f.id === s.id) })),
			episodes: (r.episodes ?? []).map(decorateEpisode)
		};
	},
	// stream: podtop — a rough stand-in for `podcasts::search::top_show`: no YouTube card and a
	// show titled like the query.
	podcast_top_show: (a) => {
		const n = (x: string) => x.toLowerCase().replace(/\b(the|podcast)\b|[^a-z0-9 ]/g, ' ').replace(/\s+/g, ' ').trim();
		const i = ((a.shows ?? []) as { title: string }[]).findIndex((s) => n(s.title) === n(String(a.query ?? '')));
		return a.top || i < 0 ? null : i;
	},
	podcast_show: (a) => {
		const p = get('podcast_show_huberman');
		const followed = podcasts.follows.find((f) => f.id === a.id);
		if (a.id !== p.show.id) {
			warnOnce(`show:${a.id}`, `no show fixture for ${a.id}; serving Huberman Lab`);
			if (followed) p.show = { ...followed };
		}
		p.show.followed = podcasts.follows.some((f) => f.id === p.show.id);
		p.episodes = p.episodes.map(decorateEpisode);
		if (a.sort === 'oldest') p.episodes.reverse();
		p.sort = a.sort ?? 'newest';
		return p;
	},
	podcast_show_more: () => ({ episodes: [], continuation: null }),
	podcast_episode: (a) => {
		const e = allEpisodes().find((x) => x.id === a.id);
		if (!e) warnOnce(`ep:${a.id}`, `no episode fixture for ${a.id}`);
		const full = fx.podcast_episode_huberman;
		return e ? { ...e, description: e.id === full?.id ? full.description : (e.description ?? full?.description) } : decorateEpisode(full);
	},
	// stream: chapters — a few chapters for every episode, so the sections show in the mock.
	podcast_chapters: (a) => {
		const d = Number(a.durationSecs) || 3600;
		const at = [0, 0.08, 0.31, 0.55, 0.8].map((f) => Math.round(f * d));
		const titles = ['Introduction', 'How sleep works', 'Light in the morning', 'Caffeine timing', 'Listener questions'];
		const label = (t: number) => (t >= 3600 ? `${Math.floor(t / 3600)}:${String(Math.floor(t / 60) % 60).padStart(2, '0')}` : `${Math.floor(t / 60)}`) + `:${String(t % 60).padStart(2, '0')}`;
		return {
			source: 'feed',
			chapters: at.map((t, i) => ({ startSecs: t, endSecs: at[i + 1] ?? d, title: titles[i], startLabel: label(t) }))
		};
	},
	podcast_episode_video: () => null, // stream: video-pods (the mock has no video streams)
	podcast_follow: (a) => {
		const s = { ...a.show, followed: true };
		if (!podcasts.follows.some((f) => f.id === s.id)) podcasts.follows.unshift(s);
		return s;
	},
	podcast_unfollow: (a) => {
		podcasts.follows = podcasts.follows.filter((f) => f.id !== a.id);
	},
	podcast_follows: () => clone(podcasts.follows),
	podcast_add_feed: () => {
		throw 'not a feed (dev mock)';
	},
	podcast_new_episodes: (a) => allEpisodes().filter((e) => !e.progress.played).slice(0, a.limit ?? 20),
	podcast_continue_listening: (a) => allEpisodes().filter((e) => e.progress.fraction > 0 && !e.progress.played).slice(0, a.limit ?? 10),
	podcast_saved_episodes: () => allEpisodes().filter((e) => e.saved),
	podcast_save_episode: (a) => {
		if (a.saved) podcasts.saved.add(a.episode.id);
		else podcasts.saved.delete(a.episode.id);
	},
	podcast_mark_played: (a) => {
		const e = allEpisodes().find((x) => x.id === a.id);
		const d = Number(e?.durationSecs ?? 0);
		const p = { positionSecs: a.played ? d : 0, durationSecs: d, fraction: a.played ? 1 : 0, played: !!a.played, updatedAt: Math.floor(Date.now() / 1000) };
		podcasts.progress[a.id] = p;
		emit('podcasts:progress', { id: a.id, progress: p });
		return p;
	},
	podcast_mark_show_played: () => 0,
	podcast_progress: (a) => Object.fromEntries((a.ids as string[]).filter((id) => podcasts.progress[id]).map((id) => [id, podcasts.progress[id]])),
	podcast_refresh: () => ({ checked: podcasts.follows.length, failed: 0, newEpisodes: [] as PodcastEpisode[] }),
	podcast_opml_export: () => '<?xml version="1.0"?><opml version="2.0"><body/></opml>',
	podcast_opml_import: () => ({ found: 0, added: 0, alreadyFollowed: 0 }),
	podcast_settings: () => ({ sponsorblock: true, sponsorblockCategories: ['sponsor', 'selfpromo'], country: 'US', podcastindex: false }),
	podcast_set_settings: (a) => ({ sponsorblock: true, sponsorblockCategories: ['sponsor', 'selfpromo'], country: 'US', podcastindex: false, ...a.settings }),
	podcast_youtube_match: () => true,
	podcast_set_youtube_match: (a) => a.on,
	podcast_youtube_twin: () => null,

	// audio
	audio_settings: () => clone(audio),
	audio_eq_info: () => ({
		bands: [
			{ kind: 'lowshelf', freq: 60, q: 0.7 },
			{ kind: 'peaking', freq: 150, q: 1 },
			{ kind: 'peaking', freq: 400, q: 1 },
			{ kind: 'peaking', freq: 1000, q: 1 },
			{ kind: 'peaking', freq: 2400, q: 1 },
			{ kind: 'highshelf', freq: 15000, q: 0.7 }
		],
		presets: EQ_PRESETS.map(([name, gains]) => ({ name, gains })),
		maxDb: 12
	}),
	set_eq: (a) => {
		const preset = EQ_PRESETS.find(([, g]) => g.every((v, i) => v === a.gains[i]))?.[0] ?? null;
		audio.eq = { ...audio.eq, enabled: a.enabled, gains: a.gains, preset };
		return { ...audio.eq };
	},
	set_eq_preset: (a) => {
		const p = EQ_PRESETS.find(([n]) => n.toLowerCase() === String(a.name).toLowerCase());
		if (p) audio.eq = { ...audio.eq, enabled: true, gains: [...p[1]], preset: p[0] };
		return { ...audio.eq };
	},
	set_normalize_volume: (a) => void (audio.normalize = a.on),
	set_volume_level: (a) => void (audio.level = a.level),
	set_crossfade_secs: (a) => (audio.crossfadeSecs = Math.max(0, Math.min(12, a.secs))),
	audio_devices: () => ({
		devices: [
			{ name: 'auto', description: 'System default' },
			{ name: 'coreaudio/BuiltInSpeakerDevice', description: 'MacBook Pro Speakers' },
			{ name: 'coreaudio/airpods', description: 'AirPods Pro' }
		],
		selected: audio.device,
		active: audio.device
	}),
	set_audio_device: (a) => {
		audio.device = a.name;
		const d = handlers.audio_devices({});
		emit('audio-devices', d);
		return d;
	},
	set_sleep_timer: (a) => {
		audio.sleep = a.endOfTrack
			? { mode: { kind: 'endOfTrack' }, remaining: null, fadeSecs: 10 }
			: { mode: { kind: 'at', endsAt: Math.floor(Date.now() / 1000) + a.minutes * 60 }, remaining: a.minutes * 60, fadeSecs: 10 };
		emit('sleep-timer', audio.sleep);
		return { ...audio.sleep };
	},
	cancel_sleep_timer: () => {
		audio.sleep = { mode: null, remaining: null, fadeSecs: 10 };
		emit('sleep-timer', audio.sleep);
		return { ...audio.sleep };
	},
	sleep_timer: () => ({ ...audio.sleep }),

	// settings, account, app
	get_settings: () => ({ ...settings }),
	set_setting: (a) => void (settings[a.key] = a.value),
	get_stream_clients: () => ['WEB_REMIX', 'IOS', 'ANDROID_VR', 'VISIONOS', 'TV'],
	clear_caches: () => null,
	smart_settings: () => ({ ...smart, private_session: db.privateSession.enabled }),
	smart_set_setting: (a) => {
		if (a.key === 'shuffle_mode') smart.shuffle_mode = a.value;
		if (a.key === 'explicit_filter') smart.explicit_filter = a.value === 'true';
	},
	get_global_hotkeys: () => ({ enabled: false, bindings: {} }),
	global_hotkeys_on_wayland: () => false,
	set_global_hotkeys: (a) => ({ success: true, config: a.config, errors: {} }),
	reset_global_hotkeys: () => ({ success: true, config: { enabled: false, bindings: {} }, errors: {} }),
	get_account: () => ({ signedIn: false }),
	get_account_identities: () => [],
	get_google_accounts: () => [],
	sign_out: () => null,
	login_webview: () => null,
	cancel_login: () => null,
	get_blocked_artists: () => clone(db.blocked),
	block_artist: (a) => (db.blocked.push({ id: a.id, name: a.name }), clone(db.blocked)),
	unblock_artist: (a) => ((db.blocked = db.blocked.filter((b) => (b.id ?? b.name) !== a.key)), clone(db.blocked)),
	// No saved audiobooks in the mock: without this the library sidebar's rows() threw on null.
	audiobook_saved: () => [],
	audiobook_save: () => true,
	get_local_library: () => ({ folders: [], albums: [], artists: [], songs: [], removed: [] }),
	add_local_folder: () => ({ folders: [], albums: [], artists: [], songs: [], removed: [] }),
	remove_local_folder: () => ({ folders: [], albums: [], artists: [], songs: [], removed: [] }),
	...importHandlers(emit), // stream: onboard (the Spotify import)
	...syncHandlers(emit), // device sync (Settings › Devices, the linking dialog)
	import_source: () => null,
	take_launch_args: () => [],
	can_self_update: () => false,
	open_external: (a) => void window.open(a.url, '_blank', 'noopener'),
	quit_app: () => null,
	diagnostics: () => 'Spotless dev mock — no backend',
	diagnostics_summary: () => 'Spotless dev mock',
	save_diagnostics: () => null,
	network_reachable: () => true,
	log_ui: () => null,
	show_main: () => true,
	open_mini: () => null,
	close_mini: () => null,
	set_mini_compact: () => null,
	theater_fullscreen: () => null,
	title_bar_double_click: () => 'Maximize'
};

// ── plugins (window, app, updater, dialog, clipboard, process, deep-link, webview) ────────────
function plugin(cmd: string, a: Args): unknown {
	const [ns, op] = cmd.slice('plugin:'.length).split('|');
	const dpr = window.devicePixelRatio || 1;
	switch (ns) {
		case 'window':
			if (op.startsWith('is_')) return op === 'is_visible' || op === 'is_focused' || op === 'is_enabled' || op === 'is_decorated' || op === 'is_resizable';
			if (op === 'scale_factor') return dpr;
			if (op === 'inner_size' || op === 'outer_size') return { width: innerWidth * dpr, height: innerHeight * dpr };
			if (op === 'inner_position' || op === 'outer_position') return { x: 0, y: 0 };
			if (op === 'theme') return 'dark';
			if (op === 'title') return document.title;
			if (op === 'get_all_windows') return ['main'];
			return null;
		case 'app':
			return { version: '0.1.0-dev', name: 'Spotless', tauri_version: '2.0.0', identifier: 'app.spotless.desktop' }[op] ?? null;
		case 'updater':
			return null; // check → no update
		case 'clipboard-manager':
			if (op === 'write_text') return navigator.clipboard?.writeText(a.text ?? a.data?.plainText?.text ?? '').catch(() => {});
			if (op === 'read_text') return '';
			return null;
		case 'dialog':
			return op === 'ask' || op === 'confirm' ? window.confirm(a.message ?? '') : null;
		default:
			return null; // process, webview zoom, menu, global-shortcut, autostart …
	}
}

// ── install ──────────────────────────────────────────────────────────────────────────────────
const warned = new Set<string>();
function warnOnce(key: string, msg: string) {
	if (warned.has(key)) return;
	warned.add(key);
	console.warn(`[devmock] ${msg}`);
}

/** What an unmocked command answers: the empty value its name suggests. */
function fallback(cmd: string): unknown {
	warnOnce(cmd, `unmocked command "${cmd}" → empty value`);
	if (/(_flags|_index|_counts|_map)$/.test(cmd)) return {};
	if (/s$|^search|^lib_|_list$/.test(cmd)) return [];
	return null;
}

export function installDevMock() {
	const w = window as any;
	if (w.__TAURI_INTERNALS__?.invoke) return;
	// `?window=mini` previews the mini player window (the layout renders only <MiniPlayer/> there).
	const label = new URLSearchParams(location.search).get('window') === 'mini' ? 'mini' : 'main';
	mockWindows(label, label === 'mini' ? 'main' : 'mini');
	mockConvertFileSrc('macos');
	mockIPC(
		async (cmd, args) => {
			// The real IPC serialises arguments; Svelte `$state` proxies passed straight in here made
			// every handler's `structuredClone` throw DataCloneError (saving from search, Play on an
			// Enhanced playlist). Copy them as the bridge would. Lane magic-d1, MAGIC-GAPS #21.
			const a = plainArgs(args) as Args;
			if (cmd.startsWith('plugin:')) return plugin(cmd, a);
			const h = cmd in handlers ? handlers[cmd] : undefined;
			const out = h ? h(a) : fallback(cmd);
			// A little latency, so skeletons and loading states are seen the way the app shows them.
			await new Promise((r) => setTimeout(r, h && /^(get_|search|smart_home|podcast_show|podcast_search)/.test(cmd) ? 120 : 0));
			return out === undefined ? null : out;
		},
		{ shouldMockEvents: true }
	);
	// ── stream: search-dropdown ── the overview's `dropdown`, roughly `search::dropdown_order`: the Top
	// result, four songs, then rounds of artist / album / two songs / playlist / show, 30 at most.
	const overviewMock = handlers.search_overview;
	handlers.search_overview = (a) => {
		const o = overviewMock(a) as any;
		const r = o.results ?? {};
		const card = (item: any) => item && { type: item.kind === 'song' ? 'song' : 'card', ...(item.kind === 'song' ? { song: { video_id: item.id, title: item.title, artists: item.subtitle ?? '', thumbnail: item.thumbnail } } : { item }) };
		const out: any[] = [];
		const top = o.topShow != null ? { type: 'show', show: o.shows[o.topShow] } : card(r.top?.[0] ?? r.artists?.[0]);
		if (top) out.push(top);
		const songs = [...(o.songs ?? []), ...(o.tryThese?.songs ?? [])];
		out.push(...songs.slice(0, 4).map((song: any) => ({ type: 'song', song })));
		for (let i = 0; i < 10; i++) {
			out.push(card(r.artists?.[i]), card(r.albums?.[i]));
			out.push(...songs.slice(4 + 2 * i, 6 + 2 * i).map((song: any) => ({ type: 'song', song })));
			out.push(card(r.playlists?.[i]), o.shows?.[i] && { type: 'show', show: o.shows[i] });
		}
		for (const it of o.tryThese?.artists ?? []) out.push(card(it));
		const seen = new Set<string>();
		const id = (d: any) => d.song?.video_id ?? d.item?.id ?? d.show?.id;
		o.dropdown = out.filter((d) => d && !seen.has(id(d)) && seen.add(id(d))).slice(0, 30);
		return o;
	};
	startTicker();
	w.__ENCORE_DEVMOCK__ = { db, player, podcasts, settings, handlers, emit, downloads: dl.control };
	console.info(
		`%c[devmock]%c browser mock backend on: ${pool.length} songs, ${db.playlists.length} playlists, ${Object.keys(fx).length} fixtures. Inspect window.__ENCORE_DEVMOCK__.`,
		'color:#ff2d7a;font-weight:bold',
		''
	);
}
