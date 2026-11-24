// Cached lookups for the Now Playing view's cards: the artist (About the artist, artist-image
// mode) and the playing podcast episode (description). One request per id per session; failures
// are dropped from the cache so the next track change retries.
import * as api from '$lib/api';

const artists = new Map<string, Promise<api.ArtistPage>>();
const episodes = new Map<string, Promise<api.PodcastEpisode>>();

function cached<T>(map: Map<string, Promise<T>>, id: string, load: () => Promise<T>): Promise<T> {
	let p = map.get(id);
	if (!p) {
		p = load();
		p.catch(() => map.delete(id));
		if (map.size > 50) map.delete(map.keys().next().value!);
		map.set(id, p);
	}
	return p;
}

export const artistInfo = (id: string) => cached(artists, id, () => api.getArtist(id));
export const episodeInfo = (id: string) => cached(episodes, id, () => api.podcastEpisode(id));

/** "70,844,258 monthly listeners" from whatever count YouTube gave (listeners or subscribers). */
export function audienceLine(a: api.ArtistPage): string | null {
	const ml = a.monthlyListeners?.trim();
	if (ml) {
		if (/listener/i.test(ml)) return ml;
		// YouTube says "66.1M monthly audience"; Spotify's wording is "monthly listeners".
		const n = ml.replace(/\s*monthly\s+audience\s*$/i, '').replace(/\s*monthly\s*$/i, '');
		return `${n} monthly listeners`;
	}
	const subs = a.subscribers?.trim();
	if (subs) return /subscriber/i.test(subs) ? subs : `${subs} subscribers`;
	return null;
}

/** Plain text of sanitised show-notes HTML. */
export function plainText(html: string | null | undefined): string {
	if (!html) return '';
	const d = new DOMParser().parseFromString(html, 'text/html');
	return (d.body.textContent ?? '').replace(/\s+\n/g, '\n').trim();
}
