// Lane magic-d3: Liked Songs filter chips (#7) and Enhance (#13), the pure parts, so
// `smarttags.check.ts` can run them in node. The tags themselves come from Rust (smart/tags.rs).

export type TagKind = 'genre' | 'mood';

/** One chip over Liked Songs: a genre or mood and the liked tracks under it. */
export interface TagChip {
	label: string;
	kind: TagKind;
	videoIds: string[];
}

export interface LikedChips {
	chips: TagChip[];
	/** Likes whose tags are known so far, out of `total` (the lookup runs in the background). */
	done: number;
	total: number;
}

/** The rows under a chip, in the list's own order; all rows when no chip is picked. */
export function filterByChip<T extends { video_id: string }>(rows: T[], chip: TagChip | null | undefined): T[] {
	if (!chip) return rows;
	const ids = new Set(chip.videoIds);
	return rows.filter((r) => ids.has(r.video_id));
}

/** A picked chip that's no longer offered (its tracks were unliked) is dropped. */
export function keepChip(label: string | null, chips: TagChip[]): string | null {
	return label && chips.some((c) => c.label === label) ? label : null;
}

/** Enhance puts one suggestion after every `ENHANCE_EVERY` songs (smartshuffle.rs ENHANCE_EVERY). */
export const ENHANCE_EVERY = 2;

/** How many suggestions fit in a list of `n` songs: none after the last song. */
export function enhanceSlots(n: number, recs: number, every = ENHANCE_EVERY): number {
	return Math.max(0, Math.min(recs, Math.floor((n - 1) / every)));
}

/** The list with its suggestions mixed in: what Play queues while Enhance is on. Mirrors
 *  `smartshuffle::interleave`. */
export function interleave<T>(tracks: T[], recs: T[], every = ENHANCE_EVERY): T[] {
	const k = enhanceSlots(tracks.length, recs.length, every);
	const out: T[] = [];
	tracks.forEach((t, i) => {
		out.push(t);
		if ((i + 1) % every === 0 && (i + 1) / every <= k) out.push(recs[(i + 1) / every - 1]);
	});
	return out;
}

/** Enhance is offered where Smart Shuffle is: your own playlists and Liked Songs. */
export const enhanceEligible = (id: string | null | undefined) =>
	!!id && (id.startsWith('LOCALPLAYLIST:') || id === 'VLLM');
