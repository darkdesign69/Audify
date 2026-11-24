// Artist discography in Spotify's terms (magic-gaps #14, #15). The rules (release types,
// compilations, Popular releases, the card lines) live in the core (`crates/core/src/discography.rs`),
// shared with the iPhone app; this is the shape it answers and the chip labels.
import { invoke } from '@tauri-apps/api/core';
import type { BrowseItem, SongItem } from '$lib/api';

export type ReleaseType = 'album' | 'single' | 'ep' | 'compilation';

export interface Release {
	item: BrowseItem;
	type: ReleaseType;
	/** 0 when YouTube gave none. */
	year: number;
	/** Position in YouTube's own lists: the tiebreak within a year. */
	order: number;
	/** The card line: "1997 • Album", "Latest Release • Single", "Compilation". */
	line: string;
	/** The list view's header: "Album • 1997". */
	meta: string;
}

/** Every chip's list, newest first (`popular`: the latest release pinned, then by popularity). */
export interface Discography {
	popular: Release[];
	latestId?: string;
	all: Release[];
	album: Release[];
	single: Release[];
	compilation: Release[];
}

export const EMPTY_DISCOGRAPHY: Discography = { popular: [], all: [], album: [], single: [], compilation: [] };

/** The discography filters, in Spotify's chip order. `popular` exists on the artist page only. */
export type DiscFilter = keyof Omit<Discography, 'latestId'>;

export const FILTER_LABEL: Record<DiscFilter, string> = {
	popular: 'Popular releases',
	all: 'All',
	album: 'Albums',
	single: 'Singles and EPs',
	compilation: 'Compilations'
};

export const discography = (albums: BrowseItem[], singles: BrowseItem[], topSongs: SongItem[] = []) =>
	invoke<Discography>('discography', { albums, singles, topSongs });

/** Which filters have anything to show (Popular releases or All first, whenever anything does). */
export function availableFilters(d: Discography, withPopular: boolean): DiscFilter[] {
	if (!d.all.length) return [];
	return [withPopular ? 'popular' : 'all', ...(['album', 'single', 'compilation'] as const).filter((f) => d[f].length)];
}

export type SortKey = 'date' | 'name';
export type SortDir = 'desc' | 'asc';

/** The Show all page's "Sort by": the core's newest-first list, reversed, or by name. */
export function sortReleases(newest: Release[], key: SortKey, dir: SortDir): Release[] {
	if (key === 'date') return dir === 'desc' ? newest : [...newest].reverse();
	const az = [...newest].sort(
		(a, b) => a.item.title.localeCompare(b.item.title, undefined, { sensitivity: 'base', numeric: true }) || a.order - b.order
	);
	return dir === 'asc' ? az : az.reverse();
}
