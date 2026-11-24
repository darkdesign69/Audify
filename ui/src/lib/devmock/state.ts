// DEV ONLY (docs/streams/devmock.md). The mutable world the mock backend answers from: library,
// queue, playback, settings, podcasts. Every song in here comes from a captured fixture.
/* eslint-disable @typescript-eslint/no-explicit-any */
import type {
	BrowseItem,
	Folder,
	LibPlaylist,
	LikedEntry,
	Pin,
	PodcastEpisode,
	PodcastShow,
	RecentContext,
	RepeatMode,
	SavedItem,
	SongItem,
	Reverb
} from '$lib/api';
import { fx, get } from './fixtures';

export const NOW = Math.floor(Date.now() / 1000);
const DAY = 86400;

export const secsOf = (d?: string): number => {
	if (!d) return 0;
	return d.split(':').reduce((acc, p) => acc * 60 + (parseInt(p, 10) || 0), 0);
};

/** Strip the per-context flags a row carries, the way Rust's `playlist_row` does. */
const clean = (s: SongItem): SongItem => {
	const { set_video_id: _a, added_at: _b, queued: _c, queued_end: _d, autoplay: _e, ...rest } = s as any;
	void _a, _b, _c, _d, _e;
	return rest;
};

// ── the song pool ────────────────────────────────────────────────────────────────────────────
function buildPool(): SongItem[] {
	const seen = new Set<string>();
	const out: SongItem[] = [];
	const add = (list: SongItem[] | undefined) => {
		for (const s of list ?? []) {
			if (!s?.video_id || seen.has(s.video_id) || !s.thumbnail) continue;
			seen.add(s.video_id);
			out.push(clean(s));
		}
	};
	add(fx.album_discovery?.items);
	add(fx.album_ram?.items);
	add(fx.album_ok_computer?.items);
	add(fx.artist_daft_punk?.topSongs);
	add(fx.artist_radiohead?.topSongs);
	add(fx.search_songs_daft_punk);
	add(fx.playlist_search_daft_punk?.items);
	add(fx.playlist_rock?.items);
	add(fx.playlist_chart_top?.items);
	return out;
}
export const pool: SongItem[] = buildPool();
export const findSong = (id: string): SongItem | undefined => pool.find((s) => s.video_id === id);

/** A deterministic slice of the pool, wrapping around. */
export const poolSlice = (start: number, n: number): SongItem[] =>
	Array.from({ length: Math.min(n, pool.length) }, (_, i) => structuredClone(pool[(start + i) % pool.length]));

export const card = (kind: BrowseItem['kind'], id: string, title: string, subtitle?: string, thumbnail?: string): BrowseItem => ({
	kind,
	id,
	title,
	subtitle,
	thumbnail
});

const albumCard = (name: string): BrowseItem | null => {
	const a = fx[`album_${name}`];
	const id = Object.entries(fx.album_index ?? {}).find(([, v]) => v === name)?.[0];
	if (!a || !id) return null;
	return card('album', id, a.title, a.artist ?? '', a.thumbnail);
};
const artistCard = (slug: string): BrowseItem | null => {
	const a = fx[`artist_${slug}`];
	if (!a) return null;
	return card('artist', a.channelId, a.name, 'Artist', a.thumbnail);
};
const playlistCard = (slug: string): BrowseItem | null => {
	const p = fx[`playlist_${slug}`];
	const id = Object.entries(fx.playlist_index ?? {}).find(([, v]) => v === slug)?.[0];
	if (!p || !id) return null;
	return card('playlist', id, p.title, p.subtitle ?? 'Playlist', p.thumbnail);
};

// ── library ──────────────────────────────────────────────────────────────────────────────────
export interface Entry {
	rowId: number;
	addedAt: number;
	song: SongItem;
}
export interface MockPlaylist extends Omit<LibPlaylist, 'count' | 'thumbnail' | 'pinned' | 'folderId'> {
	entries: Entry[];
	deleted?: boolean;
}

let nextRow = 1;
export const newRow = () => nextRow++;

const PLAYLISTS: [string, string, number, number][] = [
	// name, description, start in pool, track count
	['Late Night Drive', 'Neon, synths and empty highways.', 30, 60],
	['Discovery Deep Cuts', '', 0, 14],
	['Rock Classics', 'The 90s, loud.', 60, 25],
	['Focus Flow', 'Instrumental-ish things to work to.', 20, 18],
	['Gym', '', 45, 22],
	['Sunday Morning', 'Coffee first.', 12, 12],
	['Road Trip 2026', '', 80, 33],
	['Robot Rock', 'Helmets on.', 5, 9]
];

function seedPlaylists(): MockPlaylist[] {
	return PLAYLISTS.map(([name, description, start, n], i) => {
		const createdAt = NOW - (200 - i * 20) * DAY;
		return {
			id: `LOCALPLAYLIST:${i + 1}`,
			name,
			description,
			cover: null,
			createdAt,
			updatedAt: NOW - (i * 3 + 1) * DAY,
			lastPlayedAt: NOW - (i * 2 + 1) * 3600,
			entries: poolSlice(start, n).map((song, j) => ({
				rowId: newRow(),
				addedAt: createdAt + j * 3 * 3600,
				song
			}))
		};
	});
}

export const db = {
	liked: poolSlice(0, 40).map((song, i) => ({ song, addedAt: NOW - i * 2 * DAY - 3600 })) as LikedEntry[],
	playlists: seedPlaylists(),
	folders: [
		{ id: 'FOLDER:1', name: 'Moods', parentId: null, position: 0, createdAt: NOW - 90 * DAY, updatedAt: NOW - 9 * DAY, items: ['LOCALPLAYLIST:4', 'LOCALPLAYLIST:6'] },
		{ id: 'FOLDER:2', name: 'Workout', parentId: null, position: 1, createdAt: NOW - 80 * DAY, updatedAt: NOW - 8 * DAY, items: ['LOCALPLAYLIST:5'] }
	] as Folder[],
	pins: [
		{ itemId: 'VLLM', kind: 'liked', position: 0 },
		{ itemId: 'LOCALPLAYLIST:1', kind: 'playlist', position: 1 }
	] as Pin[],
	saved: [] as SavedItem[],
	recents: [] as RecentContext[],
	hidden: [] as { videoId: string; contextId?: string | null; song?: SongItem | null; hiddenAt: number }[],
	privateSession: { enabled: false } as { enabled: boolean; since?: number | null; expiresAt?: number | null },
	blocked: [] as { id?: string; name: string }[],
	nextPlaylist: PLAYLISTS.length + 1,
	nextFolder: 3
};

{
	const saved: [BrowseItem | null, number][] = [
		[albumCard('discovery'), 3],
		[albumCard('ram'), 12],
		[albumCard('ok_computer'), 20],
		[artistCard('daft_punk'), 5],
		[artistCard('radiohead'), 30],
		[playlistCard('rock'), 15],
		[playlistCard('chart_top'), 40]
	];
	// Saved the way an album page saves them: subtitle = the artist (the sidebar adds "Album •").
	for (const it of (fx.search_cards_daft_punk_albums ?? []).slice(0, 3))
		saved.push([{ ...it, subtitle: String(it.subtitle ?? '').split(' • ')[1] ?? it.subtitle }, 50]);
	for (const [c, days] of saved) {
		if (!c || db.saved.some((s) => s.id === c.id)) continue;
		db.saved.push({
			id: c.id,
			kind: c.kind as SavedItem['kind'],
			title: c.title,
			subtitle: c.subtitle ?? null,
			thumbnail: c.thumbnail ?? null,
			card: c,
			synced: false,
			addedAt: NOW - days * DAY
		});
	}
	const recent: [BrowseItem | null, string, number][] = [
		[albumCard('discovery'), 'album', 600],
		[card('playlist', 'LOCALPLAYLIST:1', 'Late Night Drive', 'Playlist • You', db.playlists[0].entries[0].song.thumbnail), 'playlist', 3600],
		[card('playlist', 'VLLM', 'Liked Songs', 'Playlist'), 'liked', 5 * 3600],
		[artistCard('radiohead'), 'artist', 9 * 3600],
		[playlistCard('rock'), 'playlist', DAY],
		[albumCard('ram'), 'album', 2 * DAY],
		[albumCard('ok_computer'), 'album', 3 * DAY],
		[card('playlist', 'LOCALPLAYLIST:3', 'Rock Classics', 'Playlist • You', db.playlists[2].entries[0].song.thumbnail), 'playlist', 4 * DAY],
		[artistCard('daft_punk'), 'artist', 5 * DAY],
		[playlistCard('chart_top'), 'playlist', 6 * DAY]
	];
	for (const [c, kind, ago] of recent) {
		if (c) db.recents.push({ itemId: c.id, kind, card: c, playedAt: NOW - ago });
	}
}

export const playlistById = (id: string) => db.playlists.find((p) => p.id === id && !p.deleted);
export const folderOf = (id: string) => db.folders.find((f) => f.items.includes(id))?.id ?? null;

export const libPlaylist = (p: MockPlaylist): LibPlaylist => ({
	id: p.id,
	name: p.name,
	description: p.description,
	cover: p.cover ?? null,
	thumbnail: p.entries[0]?.song.thumbnail ?? null,
	count: p.entries.length,
	createdAt: p.createdAt,
	updatedAt: p.updatedAt,
	folderId: folderOf(p.id),
	pinned: db.pins.some((x) => x.itemId === p.id),
	lastPlayedAt: p.lastPlayedAt ?? null
});

export const lastPlayed = (): Record<string, number> =>
	Object.fromEntries(db.recents.map((r) => [r.itemId, r.playedAt]));

export const snapshot = () => ({
	likedIds: db.liked.map((e) => e.song.video_id),
	playlists: db.playlists.filter((p) => !p.deleted).map(libPlaylist),
	folders: structuredClone(db.folders),
	pins: structuredClone(db.pins),
	saved: structuredClone(db.saved),
	lastPlayed: lastPlayed(),
	hiddenSongIds: db.hidden.filter((h) => !h.contextId).map((h) => h.videoId),
	privateSession: { ...db.privateSession },
	legacyMigrated: true
});

// ── player ───────────────────────────────────────────────────────────────────────────────────
const discovery = fx.album_discovery;
export const DEFAULT_REVERB: Reverb = {
	enabled: false,
	preset: 0,
	dry: 1,
	wet: 0.3,
	decay: 0.5,
	preDelay: 20,
	damping: 0.5,
	reflections: 0.5
};

export const player = {
	items: [] as SongItem[],
	current: 0,
	shuffle: false,
	repeat: 'off' as RepeatMode,
	sourceName: discovery?.title ?? 'Discovery' as string | null,
	sourceId: (discovery?.playlistId ?? null) as string | null,
	paused: false,
	position: 47,
	volume: 70,
	speed: 1,
	semitones: 0,
	reverb: DEFAULT_REVERB
};
{
	const album: SongItem[] = (discovery?.items ?? poolSlice(0, 14)).map(clean);
	const queued = poolSlice(14, 2).map((s) => ({ ...s, queued: true, queued_from: fx.album_ram?.title }));
	// Now playing: "Digital Love" (track 3) with two "Next in queue" rows behind it, then the rest
	// of the album as "Next from: Discovery".
	player.current = Math.min(2, album.length - 1);
	player.items = [...album.slice(0, player.current + 1), ...queued, ...album.slice(player.current + 1)];
}

export const nowSong = (): SongItem | undefined => player.items[player.current];

// ── podcasts ─────────────────────────────────────────────────────────────────────────────────
const showPage = fx.podcast_show_huberman;
export const podcasts = {
	follows: [] as PodcastShow[],
	saved: new Set<string>(),
	progress: {} as Record<string, { positionSecs: number; durationSecs: number; fraction: number; played: boolean; updatedAt: number }>
};
{
	const shows: PodcastShow[] = [];
	if (showPage) shows.push({ ...showPage.show, followed: true });
	for (const s of (fx.podcast_search_huberman?.shows ?? []) as PodcastShow[]) {
		if (shows.length >= 4) break;
		if (!shows.some((x) => x.id === s.id) && /podcast/i.test(s.title + s.author)) shows.push({ ...s, followed: true });
	}
	podcasts.follows = shows;
	const eps: PodcastEpisode[] = showPage?.episodes ?? [];
	eps.slice(1, 3).forEach((e, i) => {
		const dur = Number(e.durationSecs ?? 3600);
		const pos = Math.round(dur * (i ? 0.62 : 0.25));
		podcasts.progress[e.id] = { positionSecs: pos, durationSecs: dur, fraction: pos / dur, played: false, updatedAt: NOW - (i + 1) * DAY };
	});
	if (eps[4]) {
		const d = Number(eps[4].durationSecs ?? 3600);
		podcasts.progress[eps[4].id] = { positionSecs: d, durationSecs: d, fraction: 1, played: true, updatedAt: NOW - 6 * DAY };
	}
	eps.slice(0, 2).forEach((e) => podcasts.saved.add(e.id));
}

export const decorateEpisode = (e: PodcastEpisode): PodcastEpisode => ({
	...e,
	progress: podcasts.progress[e.id] ?? { positionSecs: 0, durationSecs: Number(e.durationSecs ?? 0), fraction: 0, played: false, updatedAt: 0 },
	saved: podcasts.saved.has(e.id)
});
export const allEpisodes = (): PodcastEpisode[] => {
	const list: PodcastEpisode[] = [...(showPage?.episodes ?? [])];
	for (const e of fx.podcast_search_huberman?.episodes ?? []) if (!list.some((x) => x.id === e.id)) list.push(e);
	if (fx.podcast_episode_huberman && !list.some((x) => x.id === fx.podcast_episode_huberman.id)) list.push(fx.podcast_episode_huberman);
	return list.map(decorateEpisode);
};

// ── settings ─────────────────────────────────────────────────────────────────────────────────
export const settings: Record<string, string> = {
	native_chrome: 'overlay',
	os_corners: 'false',
	native_video: 'false',
	locale: 'en'
};
export const smart = { shuffle_mode: 'smart' as 'smart' | 'standard', explicit_filter: false };
export const audio = {
	eq: { enabled: false, gains: [0, 0, 0, 0, 0, 0], preset: 'Flat' as string | null, preamp: 0 },
	normalize: true,
	level: 'normal' as 'quiet' | 'normal' | 'loud',
	crossfadeSecs: 0,
	device: 'auto',
	sleep: { mode: null as any, remaining: null as number | null, fadeSecs: 10 }
};
export const EQ_PRESETS: [string, number[]][] = [
	['Flat', [0, 0, 0, 0, 0, 0]],
	['Acoustic', [5.8, 3.5, 0.8, 0.7, 5, 3.5]],
	['Bass Booster', [6, 3.6, 1.2, -0.2, 0, 0]],
	['Bass Reducer', [-6, -3.6, -1.2, 0.2, 0, 0]],
	['Classical', [5.1, 3.7, -0.3, -2.5, 2.2, 5.8]],
	['Dance', [5.1, 4.7, -0.8, 2.7, 6, 0.5]],
	['Deep', [5.5, 1.4, 1.4, 3.1, -1.3, -7.2]],
	['Electronic', [4.9, 1.8, -2.5, 1.7, 1.5, 7.5]],
	['Hip-Hop', [5.6, 2.7, 0.3, -1.4, 1.4, 4.5]],
	['Jazz', [4.3, 2.4, -0.1, -2.3, 1.7, 5.8]],
	['Latin', [4.9, 0.4, -0.8, -1.7, -0.3, 6.9]],
	['Loudness', [6.6, 0.6, -1.8, 0.8, -2.9, 2.3]],
	['Lounge', [-3.1, -0.7, 3.3, 2.4, -1.2, 1.7]],
	['Piano', [3.2, 0.3, 3.2, 0, 5.2, 5.3]],
	['Pop', [-1.7, -0.1, 3.1, 3.6, 0.4, -2.3]],
	['R&B', [4.2, 6.6, -2.5, -2.1, 4, 5.7]],
	['Rock', [5.5, 3.2, -0.1, -1.8, 2.5, 7]],
	['Small Speakers', [6, 3.6, 1.1, 0.5, -2.8, -6.6]],
	['Spoken Word', [-3.2, -0.1, 1.9, 3.4, 5.3, 0.4]],
	['Treble Booster', [0, 0, -0.1, 0.2, 4.2, 8.6]],
	['Treble Reducer', [0, 0, 0.1, -0.2, -4.2, -8.6]],
	['Vocal Booster', [-2.2, -3.1, 3.6, 2.9, 2.2, -2.1]]
];

export { get };
