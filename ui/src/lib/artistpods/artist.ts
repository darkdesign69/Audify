// Artist page helpers (stream p-artist-pods). YouTube Music names an artist page's carousels in
// its own words ("Albums", "Singles & EPs", "Videos", "Featured on", "Fans might also like",
// "Playlists"); Spotify's page has fixed sections. The core maps one onto the other (`kind`).
import type { ArtistCarousel, BrowseItem } from '$lib/api';

export type SectionKind = ArtistCarousel['kind'];

/** "70.8M monthly listeners" / "12.3M subscribers": YouTube's numbers in Spotify's words
 *  (YouTube says "monthly audience"; a Spotify user reads "monthly listeners", parity3 P3-7). */
export function audienceLine(a: { monthlyListeners?: string; subscribers?: string }): string | null {
	const m = a.monthlyListeners?.trim().replace(/\s*monthly\s+audience\s*$/i, '');
	if (m) return /[a-z]/i.test(m.replace(/[\d.,KMB]/g, '')) ? m : `${m} monthly listeners`;
	const s = a.subscribers?.trim();
	if (s) return /subscriber/i.test(s) ? s : `${s} subscribers`;
	return null;
}
