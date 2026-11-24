// Pure helpers for the kit's TrackTable / TrackRow: column sets per variant, the responsive
// column drop, date-added and duration formatting, and the songs drag payload.
import type { SongItem } from '$lib/api';

export type TrackVariant = 'playlist' | 'album' | 'artist' | 'queue' | 'search';
export type TrackColumn = 'index' | 'title' | 'artist' | 'album' | 'added' | 'plays' | 'duration';
export type TrackSortKey = 'title' | 'artist' | 'album' | 'added' | 'duration';
export type TrackSort = { key: TrackSortKey; dir: 'asc' | 'desc' } | null;

/** Drag payload MIME for one or more songs (JSON `SongItem[]`). Sidebar playlists accept it. */
export const SONGS_MIME = 'application/x-encore-songs';

/** Full column set per variant (before the width-based drop). */
export function columnsFor(variant: TrackVariant): TrackColumn[] {
	switch (variant) {
		case 'album':
			return ['index', 'title', 'duration'];
		case 'artist':
			return ['index', 'title', 'plays', 'duration'];
		case 'search':
		case 'queue':
			return ['index', 'title', 'album', 'duration'];
		default:
			return ['index', 'title', 'album', 'added', 'duration'];
	}
}

/** Spotify drops "Date added" first, then "Album", as the main view narrows. Compact view
 *  (playlist ▸ View as ▸ Compact) moves the artists out of the title cell into their own column. */
export function visibleColumns(variant: TrackVariant, width: number, compact = false): TrackColumn[] {
	let cols = columnsFor(variant);
	if (compact && variant !== 'album') cols = cols.flatMap((c) => (c === 'title' ? ['title', 'artist'] : [c])) as TrackColumn[];
	if (compact && width > 0 && width < 600) cols = cols.filter((c) => c !== 'artist');
	if (width > 0 && width < 700) cols = cols.filter((c) => c !== 'added');
	if (width > 0 && width < 500) cols = cols.filter((c) => c !== 'album' && c !== 'plays');
	return cols;
}

const TRACK = {
	index: '16px',
	title: 'minmax(120px,4fr)',
	artist: 'minmax(120px,2fr)',
	album: 'minmax(120px,2fr)',
	added: 'minmax(120px,2fr)',
	plays: 'minmax(120px,2fr)',
	duration: 'minmax(120px,1fr)'
} as const;

/** `grid-template-columns` for a column set. */
export function gridTemplate(cols: TrackColumn[]): string {
	return cols.map((c) => TRACK[c]).join(' ');
}

export const COLUMN_LABEL: Record<TrackColumn, string> = {
	index: '#',
	title: 'Title',
	artist: 'Artist',
	album: 'Album',
	added: 'Date added',
	plays: 'Plays',
	duration: 'Duration'
};

export const SORTABLE: Partial<Record<TrackColumn, TrackSortKey>> = {
	title: 'title',
	artist: 'artist',
	album: 'album',
	added: 'added',
	duration: 'duration'
};

/** asc → desc → none (custom order). */
export function nextSort(cur: TrackSort, key: TrackSortKey): TrackSort {
	if (!cur || cur.key !== key) return { key, dir: 'asc' };
	if (cur.dir === 'asc') return { key, dir: 'desc' };
	return null;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** "3 weeks ago" under a month, else "Sep 29, 2023" (Spotify English). `at` is unix seconds. */
export function formatAdded(at: number | undefined, now = Date.now()): string {
	if (!at) return '';
	const ms = at * 1000;
	const s = Math.max(0, Math.floor((now - ms) / 1000));
	if (s < 30 * 86400) {
		if (s < 60) return 'just now';
		const m = Math.floor(s / 60);
		if (m < 60) return m === 1 ? '1 minute ago' : `${m} minutes ago`;
		const h = Math.floor(m / 60);
		if (h < 24) return h === 1 ? '1 hour ago' : `${h} hours ago`;
		const d = Math.floor(h / 24);
		if (d < 7) return d === 1 ? '1 day ago' : `${d} days ago`;
		const w = Math.floor(d / 7);
		return w === 1 ? '1 week ago' : `${w} weeks ago`;
	}
	const dt = new Date(ms);
	return `${MONTHS[dt.getMonth()]} ${dt.getDate()}, ${dt.getFullYear()}`;
}

/** "3:47" style string → seconds (sorting). */
export function durationSecs(d: string | undefined): number {
	if (!d) return 0;
	return d.split(':').reduce((acc, p) => acc * 60 + (parseInt(p, 10) || 0), 0);
}

/** Sort a copy of `tracks` (stable). `null` returns the input order. Returns original indices. */
export function sortedOrder(tracks: SongItem[], sort: TrackSort): number[] {
	const idx = tracks.map((_, i) => i);
	if (!sort) return idx;
	const key = (s: SongItem): string | number => {
		switch (sort.key) {
			case 'title':
				return s.title.toLocaleLowerCase();
			case 'artist':
				return (s.artists ?? '').toLocaleLowerCase();
			case 'album':
				return (s.album ?? '').toLocaleLowerCase();
			case 'added':
				return s.added_at ?? 0;
			case 'duration':
				return durationSecs(s.duration);
		}
	};
	const dir = sort.dir === 'asc' ? 1 : -1;
	return idx.sort((a, b) => {
		const ka = key(tracks[a]);
		const kb = key(tracks[b]);
		return ka < kb ? -dir : ka > kb ? dir : a - b;
	});
}

/** Read a songs payload off a drag event (sidebar drop targets). */
export function getDragSongs(e: DragEvent): SongItem[] | null {
	const raw = e.dataTransfer?.getData(SONGS_MIME);
	if (!raw) return null;
	try {
		const v = JSON.parse(raw);
		return Array.isArray(v) ? (v as SongItem[]) : null;
	} catch {
		return null;
	}
}

export const isSongsDrag = (e: DragEvent): boolean =>
	!!e.dataTransfer?.types.includes(SONGS_MIME);

/** Set the songs payload and a Spotify-style pill ghost (#282828, 14px white, menu shadow). */
export function setDragSongs(e: DragEvent, songs: SongItem[]): void {
	const dt = e.dataTransfer;
	if (!dt) return;
	dt.effectAllowed = 'copyMove';
	dt.setData(SONGS_MIME, JSON.stringify(songs));
	dt.setData('text/plain', songs.length === 1 ? songs[0].title : `${songs.length} songs`);
	const ghost = document.createElement('div');
	ghost.textContent =
		songs.length === 1 ? `${songs[0].title} • ${songs[0].artists}` : `${songs.length} songs`;
	Object.assign(ghost.style, {
		position: 'fixed',
		top: '-1000px',
		left: '-1000px',
		padding: '8px 12px',
		background: '#282828',
		color: '#fff',
		borderRadius: '4px',
		font: '400 14px/18px var(--font-sans)',
		boxShadow: '0 16px 24px rgb(0 0 0 / 0.3), 0 6px 8px rgb(0 0 0 / 0.2)',
		maxWidth: '320px',
		whiteSpace: 'nowrap',
		overflow: 'hidden',
		textOverflow: 'ellipsis'
	});
	document.body.appendChild(ghost);
	dt.setDragImage(ghost, 12, 16);
	setTimeout(() => ghost.remove(), 0);
}
