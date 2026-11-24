// Fetching for the artist's discography (artist page + Show all page). Everything goes through
// `pagecache`, so the artist page's background fetch makes Show all open instantly, and both
// pages share one in-flight request per list.
import * as api from '$lib/api';
import type { AlbumPage, ArtistCarousel, ArtistPage, BrowseItem, PlaylistPage, SongItem } from '$lib/api';
import { getCached, putCached } from '$lib/pagecache';
import { discography, type Discography } from './discography';

const inflight = new Map<string, Promise<unknown>>();

/** Cached + deduplicated fetch: the same key asked twice at once is one request. */
async function once<T>(key: string, fetch: () => Promise<T>): Promise<T> {
	const hit = getCached<T>(key);
	if (hit) return hit;
	const running = inflight.get(key) as Promise<T> | undefined;
	if (running) return running;
	const p = fetch()
		.then((v) => {
			putCached(key, v);
			return v;
		})
		.finally(() => inflight.delete(key));
	inflight.set(key, p);
	return p;
}

/** Every release of a carousel (its "More" grid), else the carousel's own cards. */
export async function fullList(s: ArtistCarousel | undefined): Promise<BrowseItem[]> {
	if (!s) return [];
	if (!s.moreBrowseId) return s.items;
	try {
		const items = await once(`list:${s.moreBrowseId}:${s.moreParams ?? ''}`, () =>
			api.getBrowseGrid(s.moreBrowseId!, s.moreParams)
		);
		// Only release cards count: anything else means the grid isn't the release list.
		const releases = items.filter((i) => i.kind === 'album');
		return releases.length ? releases : s.items;
	} catch {
		return s.items;
	}
}

const albumsOf = (a: ArtistPage) => a.sections.find((s) => s.kind === 'albums');
const singlesOf = (a: ArtistPage) => a.sections.find((s) => s.kind === 'singles');

/** The discography from the artist page's own carousels and Popular rows (no fetch). */
export const carouselDiscography = (a: ArtistPage): Promise<Discography> =>
	discography(albumsOf(a)?.items ?? [], singlesOf(a)?.items ?? [], a.topSongs);

/** The discography from every album and single (the carousels' "More" grids); `songs` rank Popular. */
export async function fullDiscography(
	a: ArtistPage,
	songs: SongItem[] | Promise<SongItem[]> = a.topSongs
): Promise<Discography> {
	const [al, si, top] = await Promise.all([fullList(albumsOf(a)), fullList(singlesOf(a)), songs]);
	return discography(al, si, top);
}

/** The artist's top-songs playlist (cached under the same key the artist page reads). */
export const topSongsPlaylist = (pid: string): Promise<PlaylistPage> => once(`playlist:${pid}`, () => api.getPlaylist(pid));

/** The top songs, most popular first: the page's rows (with play counts), then the playlist's. */
export async function topSongs(a: ArtistPage): Promise<SongItem[]> {
	if (!a.topSongsId) return a.topSongs;
	try {
		const pl = await topSongsPlaylist(a.topSongsId);
		const seen = new Set(a.topSongs.map((s) => s.video_id));
		return [...a.topSongs, ...pl.items.filter((s) => !seen.has(s.video_id))];
	} catch {
		return a.topSongs;
	}
}

/** One release's tracks (the list view), cached under the album page's own key. */
export const albumTracks = (id: string): Promise<AlbumPage> => once(`album:${id}`, () => api.getAlbum(id));
