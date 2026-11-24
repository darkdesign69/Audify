// Pure helpers for the playlist / album / Liked Songs pages (stream p-collections).
import type { SongItem } from '$lib/api';

/** "3:47" → seconds (same as the kit's `durationSecs`; inlined so the self-check runs in Node). */
const durationSecs = (d: string | undefined): number =>
	d ? d.split(':').reduce((acc, p) => acc * 60 + (parseInt(p, 10) || 0), 0) : 0;

/** Sum of the rows' durations, seconds. */
export const totalSecs = (songs: SongItem[]): number =>
	songs.reduce((n, s) => n + durationSecs(s.duration), 0);

/** Spotify's meta-line duration: "25 min 6 sec", "1 hr 32 min", "about 11 hr" (long lists).
 *  Playlists (`approx`) say "about 3 hr 30 min" once they pass an hour; albums don't. */
export function formatTotal(secs: number, count: number, approx = false): string {
	if (secs <= 0) return '';
	const h = Math.floor(secs / 3600);
	const m = Math.floor((secs % 3600) / 60);
	const s = Math.floor(secs % 60);
	if (h === 0) return s ? `${m} min ${s} sec` : `${m} min`;
	if (count > 100 || h >= 10) return `about ${m >= 30 ? h + 1 : h} hr`;
	const exact = m ? `${h} hr ${m} min` : `${h} hr`;
	return approx ? `about ${exact}` : exact;
}

/** "7 songs, 25 min 6 sec" / "1 song" / "" for none. */
export function songsMeta(songs: SongItem[], count = songs.length, approx = false): string {
	if (!count) return '';
	const n = count === 1 ? '1 song' : `${count.toLocaleString('en-US')} songs`;
	const d = formatTotal(totalSecs(songs), count, approx);
	return d ? `${n}, ${d}` : n;
}

/** First four distinct album covers, for the default 2x2 playlist mosaic (fewer than 4 → none). */
export function mosaicCovers(songs: SongItem[]): string[] {
	const seen = new Set<string>();
	const out: string[] = [];
	for (const s of songs) {
		const key = s.album_id ?? s.album ?? s.thumbnail;
		if (!s.thumbnail || !key || seen.has(key)) continue;
		seen.add(key);
		out.push(s.thumbnail);
		if (out.length === 4) break;
	}
	return out.length === 4 ? out : [];
}

/** "Album • 2018" → { type: 'Album', year: '2018' }. YouTube's album subtitle. */
export function albumKind(subtitle: string | undefined): { type: string; year: string } {
	const parts = (subtitle ?? '').split('•').map((p) => p.trim()).filter(Boolean);
	const year = parts.find((p) => /^\d{4}$/.test(p)) ?? '';
	const t = parts.find((p) => /^(album|single|ep|compilation|audiobook)$/i.test(p));
	const type = t ? (t.toUpperCase() === 'EP' ? 'EP' : t[0].toUpperCase() + t.slice(1).toLowerCase()) : 'Album';
	return { type, year };
}

/** The author out of a YouTube playlist subtitle ("Playlist • Catherine • 2024" → "Catherine"). */
export function playlistAuthor(subtitle: string | undefined): string | undefined {
	const parts = (subtitle ?? '').split('•').map((p) => p.trim()).filter(Boolean);
	return parts.find(
		(p) =>
			!/^(playlist|album|single|ep|public|private|unlisted)$/i.test(p) &&
			!/^\d{4}$/.test(p) &&
			!/\d[\d,.]*\s*[KMB]?\s*(songs?|tracks?|views?|episodes?|minutes?|hours?|saves?)/i.test(p)
	);
}

/** Case-insensitive filter over title / artists / album (find in playlist, ⌘F). */
export function matchSong(s: SongItem, q: string): boolean {
	const n = q.trim().toLocaleLowerCase();
	if (!n) return true;
	return (
		s.title.toLocaleLowerCase().includes(n) ||
		(s.artists ?? '').toLocaleLowerCase().includes(n) ||
		(s.album ?? '').toLocaleLowerCase().includes(n)
	);
}
