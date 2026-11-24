// Small formatters for discover cards (Spotify's episode meta: "14 Nov • 2 min", "23 min left").
import type { EpisodeProgress, PodcastEpisode, Shelf as HomeShelf } from '$lib/api';

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function episodeDate(unix?: number | null): string {
	if (!unix) return '';
	const d = new Date(unix * 1000);
	const base = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
	return d.getFullYear() === new Date().getFullYear() ? base : `${base}, ${d.getFullYear()}`;
}

export function minutes(secs: number): string {
	const m = Math.max(1, Math.round(secs / 60));
	if (m < 60) return `${m} min`;
	const h = Math.floor(m / 60);
	const r = m % 60;
	return r ? `${h} hr ${r} min` : `${h} hr`;
}

export function episodeLength(ep: PodcastEpisode, p: EpisodeProgress = ep.progress): string {
	const total = ep.durationSecs ?? p.durationSecs;
	if (!total) return '';
	if (p.played) return 'Played';
	if (p.positionSecs > 0) return `${minutes(total - p.positionSecs)} left`;
	return minutes(total);
}

const KIND_WORDS = /^(song|video|album|single|ep|playlist|artist|profile|episode|podcast|compilation)$/i;

/**
 * The creator part of a YouTube Music subtitle, without the kind word, year and counts that
 * YouTube puts in it: "Album • Daft Punk • 2013" → "Daft Punk". Spotify's typeahead and search
 * rows add the kind themselves ("Album • Daft Punk").
 */
export function creatorOf(subtitle?: string | null): string {
	if (!subtitle) return '';
	return subtitle
		.split(/\s+[•·]\s+/)
		.map((p) => p.trim())
		.filter(
			(p) =>
				p &&
				!KIND_WORDS.test(p) &&
				!/^\d{4}$/.test(p) &&
				!/\b(views?|plays?|songs?|tracks?|subscribers?|monthly (audience|listeners))$/i.test(p) &&
				!/^\d+:\d{2}(:\d{2})?$/.test(p) &&
				!/^\d+\s*(hours?|hr|minutes?|min)\b/i.test(p)
		)
		.join(' • ');
}

/** Home never shows a card or shortcut without a title: an empty one falls back to its song's
 *  title, and an item with neither is left out (an old untitled radio context, for one). */
export function titled(shelves: HomeShelf[]): HomeShelf[] {
	return shelves.map((s) => ({
		...s,
		items: s.items
			.map((i) => (i.title?.trim() ? i : { ...i, title: i.song?.title?.trim() ?? '' }))
			.filter((i) => i.title)
	}));
}

// ── stream: parity3 ── card subtitles without YouTube's counts ("722K views • 50 tracks • 3 hours").
const COUNT_PART =
	/(\b(views?|plays?|songs?|tracks?|subscribers?|episodes?|watching)$)|(^\d+:\d{2}(:\d{2})?$)|(^\d+\s*(hours?|hr|minutes?|min)\b)/i;

/**
 * A card's subtitle, Spotify style. Count, view and length parts are dropped everywhere. A
 * playlist in YouTube's "Playlist • Owner • 633 views" shape becomes "Playlist • Owner" (`'kind'`,
 * Home / Recently played) or "By Owner" (`'by'`, search's "Featuring" shelf); free text such as a
 * description is kept as it is.
 */
export function cardSubtitle(
	kind: string,
	subtitle?: string | null,
	style: 'kind' | 'by' = 'kind'
): string {
	if (!subtitle) return kind === 'playlist' ? 'Playlist' : '';
	const parts = subtitle.split(/\s+[•·]\s+/).map((p) => p.trim()).filter(Boolean);
	const kept = parts.filter((p) => !COUNT_PART.test(p));
	if (kind === 'playlist' && (parts.length > 1 || kept.length < parts.length || KIND_WORDS.test(parts[0] ?? ''))) {
		const owner = creatorOf(subtitle);
		if (style === 'by') return owner ? `By ${owner}` : 'Playlist';
		return owner ? `Playlist • ${owner}` : 'Playlist';
	}
	return kept.join(' • ');
}
