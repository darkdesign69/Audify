// Cached loaders shared by the discover pages, so Home → "Show all" → back and Search → genre
// paint instantly from the page cache (5 min) and only revalidate behind the scenes.
import * as api from '$lib/api';
import tiles from '../../../../crates/core/browse_tiles.json';
import type {
	ChartsPage,
	MoodSection,
	NewReleasesPage,
	SectionsPage,
	Shelf as HomeShelf
} from '$lib/api';
import { getCached, putCached } from '$lib/pagecache';
import { titled } from './format';

async function cached<T>(key: string, fetch: () => Promise<T>, fresh = false): Promise<T> {
	const hit = fresh ? null : getCached<T>(key);
	if (hit) return hit;
	const v = await fetch();
	putCached(key, v);
	return v;
}

export const peek = <T>(key: string) => getCached<T>(key);

export const HOME_KEY = 'discover:home';
// stream: perf — the last Home is kept across launches, so a start paints it at once (as Spotify
// does) and revalidates behind it, instead of skeletons until every Home request has answered.
// Dropped with the page cache on sign-in / sign-out (pagecache.clearCached).
export const HOME_STORE = 'encore:home:v1';
export const loadHome = async (fresh = false) => {
	const v = await cached<HomeShelf[]>(HOME_KEY, () => api.smartHome().then(titled), fresh);
	try {
		localStorage.setItem(HOME_STORE, JSON.stringify(v));
	} catch {
		/* storage full or unavailable: only the next launch's head start is lost */
	}
	return v;
};
/** The Home to paint right away: this session's, else the last launch's. */
export function peekHome(): HomeShelf[] | null {
	const hit = getCached<HomeShelf[]>(HOME_KEY);
	if (hit) return hit;
	try {
		// A Home saved before the Encore → Spotless rename points its cover files into the old data
		// folder, which the app moved to the new one at launch (src-tauri/src/migrate.rs).
		const raw = localStorage.getItem(HOME_STORE)?.replaceAll('app.encore.desktop', 'app.spotless.desktop');
		const v = raw ? (JSON.parse(raw) as HomeShelf[]) : null;
		return Array.isArray(v) && v.length ? v : null;
	} catch {
		return null;
	}
}

// stream: home-tiles-pods — Home's Podcasts chip and a podcast category's shows.
export const PODS_KEY = 'discover:podhome';
export const loadPodcastHome = (fresh = false) => cached<HomeShelf[]>(PODS_KEY, api.podcastHome, fresh);
export const loadPodcastCategory = (genre: string) =>
	cached(`discover:podcat:${genre}`, () => api.podcastCategory(genre));
/** Where a Browse-style tile (a mood/genre, or a podcast category) opens. */
export const podcastCategoryHref = (title: string, genre: string) =>
	`/list?podcat=${encodeURIComponent(genre)}&title=${encodeURIComponent(title)}`;

export const MOODS_KEY = 'discover:moods';
export const loadMoods = () => cached<MoodSection[]>(MOODS_KEY, api.getMoods);

export const loadMoodArt = (params: string[]) =>
	cached<Record<string, string>>(`discover:moodart:${params.length}:${params[0] ?? ''}`, () =>
		api.getMoodArt(params)
	);

export const chartsKey = (country?: string) => `discover:charts:${country ?? ''}`;
export const loadCharts = (country?: string) =>
	cached<ChartsPage>(chartsKey(country), () => api.getCharts(country));

export const NEW_KEY = 'discover:new';
export const loadNewReleases = () => cached<NewReleasesPage>(NEW_KEY, api.getNewReleases);

export const sectionsKey = (params: string) => `discover:genre:${params}`;
export const loadGenre = (params: string) =>
	cached<SectionsPage>(sectionsKey(params), () =>
		api.getBrowseSections(api.MOODS_CATEGORY_ID, params)
	);

/** Browse tile colours: one list for both apps, in `crates/core/browse_tiles.json` (the iPhone
 *  mirrors its hexes). `TILE` are the fixed tiles; `TILE_COLORS` cycle over genre and mood tiles. */
export const TILE = tiles.fixed;
export const TILE_COLORS: string[] = tiles.palette;

/** Grey rows playback would step over (explicit filter / unavailable), per the page brief. */
export async function markSkips(songs: import('$lib/api').SongItem[]) {
	if (!songs.length) return songs;
	try {
		const r = await api.playbackSkipReasons(songs);
		return songs.map((s) => (r[s.video_id] ? { ...s, unavailable: true } : s));
	} catch {
		return songs;
	}
}
