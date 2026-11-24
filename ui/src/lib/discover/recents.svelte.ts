// Search's "Recent searches": the entities you opened from search results (Spotify keeps those,
// not the raw queries). Plain submitted queries still live in `personal.searches` (the top bar's
// dropdown); this list is the cards on /search. Persisted per machine in localStorage.
import { browser } from '$app/environment';
import type { BrowseItem, ExploreItem } from '$lib/api';

export type RecentItem = Pick<ExploreItem, 'kind' | 'id' | 'title' | 'subtitle' | 'thumbnail'>;

const KEY = 'encore:recent-search-items';
const MAX = 20;

function read(): RecentItem[] {
	if (!browser) return [];
	try {
		const v = JSON.parse(localStorage.getItem(KEY) ?? '[]');
		return Array.isArray(v) ? v.slice(0, MAX) : [];
	} catch {
		return [];
	}
}

export const recentSearches = $state({ items: read() });

function save() {
	try {
		localStorage.setItem(KEY, JSON.stringify(recentSearches.items));
	} catch {
		/* private mode */
	}
}

export function addRecent(item: BrowseItem | ExploreItem | RecentItem) {
	const entry: RecentItem = {
		kind: item.kind,
		id: item.id,
		title: item.title,
		subtitle: item.subtitle,
		thumbnail: item.thumbnail
	};
	recentSearches.items = [entry, ...recentSearches.items.filter((r) => r.id !== item.id)].slice(0, MAX);
	save();
}

export function removeRecent(id: string) {
	recentSearches.items = recentSearches.items.filter((r) => r.id !== id);
	save();
}

export function clearRecents() {
	recentSearches.items = [];
	save();
}
