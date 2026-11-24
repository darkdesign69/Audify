// Per-list "Sort by" and global "View as" (Spotify remembers both). localStorage, best effort.
import type { TrackSort } from '$lib/kit/trackTable';

const SORTS = 'encore.collections.sort';
const VIEW = 'encore.collections.compact';

function read<T>(key: string, fallback: T): T {
	try {
		const v = localStorage.getItem(key);
		return v === null ? fallback : (JSON.parse(v) as T);
	} catch {
		return fallback;
	}
}

/** Liked Songs opens unsorted like Spotify: its own order is already newest first, and no
 *  column header is highlighted until the user picks a sort. */
export function loadView(id: string, liked = false): { sort: TrackSort; compact: boolean } {
	const sorts = read<Record<string, TrackSort>>(SORTS, {});
	let sort = id in sorts ? sorts[id] : null;
	// Older builds saved Date added ↓ as Liked Songs' default; that is the same order, unhighlighted.
	if (liked && sort?.key === 'added' && sort.dir === 'desc') sort = null;
	return { sort, compact: read<boolean>(VIEW, false) };
}

export function saveView(id: string, sort: TrackSort, compact: boolean) {
	if (!id) return;
	try {
		const sorts = read<Record<string, TrackSort>>(SORTS, {});
		sorts[id] = sort;
		localStorage.setItem(SORTS, JSON.stringify(sorts));
		localStorage.setItem(VIEW, JSON.stringify(compact));
	} catch {
		/* storage off: not remembered */
	}
}
