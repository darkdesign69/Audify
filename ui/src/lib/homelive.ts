// stream: magic-d4 — Home that reacts the moment you play (MAGIC-GAPS #2). Pure helpers, so the
// self-check (homelive.check.ts) runs them in plain Node:
//
// - `contextTile` turns the `home-changed` `played` payload's context into a shortcut tile;
// - `promoteShortcut` moves that tile to the front of the grid (after Liked Songs, which Home
//   keeps first), optimistically, before any round trip;
// - `mergeLocal` folds a `smart_home_local` answer (the local shelves only) into the Home on
//   screen, leaving the YouTube shelves where they are.

export type HomeItem = {
	kind: string;
	id: string;
	title: string;
	subtitle?: string;
	thumbnail?: string;
	params?: string;
	explicit?: boolean;
};
export type HomeShelfLike<I extends HomeItem = HomeItem> = {
	id: string;
	title: string;
	layout: string;
	source: string;
	items: I[];
};

/** The shelves `smart_home_local` computes (smart/home.rs `local_shelves`). */
export const LOCAL_SHELVES = ['shortcuts', 'recently_played', 'jump_back_in', 'top_mixes'];
const TILES = 8;

/** The shortcut tile for a started context, or null when there is nothing to move (Liked Songs
 *  already leads the grid; a context with no title would render as bare artwork). */
export function contextTile(
	ctx: { kind: string; id: string; title: string; subtitle?: string; thumbnail?: string; params?: string } | undefined
): HomeItem | null {
	if (!ctx || !ctx.kind || !ctx.id || !ctx.title?.trim()) return null;
	if (ctx.kind === 'liked' || ctx.id === 'VLLM' || ctx.kind === 'song') return null;
	const tile: HomeItem = { kind: ctx.kind, id: ctx.id, title: ctx.title, explicit: false };
	if (ctx.subtitle) tile.subtitle = ctx.subtitle;
	if (ctx.thumbnail) tile.thumbnail = ctx.thumbnail;
	if (ctx.params) tile.params = ctx.params;
	return tile;
}

const same = (a: HomeItem, b: HomeItem) => a.kind === b.kind && a.id === b.id;

/** Move (or add) `tile` to the front of the shortcut grid; at most 8 tiles. A new array. */
export function promoteShortcut<S extends HomeShelfLike>(shelves: S[], tile: HomeItem): S[] {
	const at = shelves.findIndex((s) => s.layout === 'shortcuts');
	if (at < 0) {
		const grid = { id: 'shortcuts', title: '', layout: 'shortcuts', source: 'local', items: [tile] } as unknown as S;
		return [grid, ...shelves];
	}
	const old = shelves[at];
	const existing = old.items.find((i) => same(i, tile));
	// Keep the card already on screen (its art is known to load), else the new one.
	const moved = existing ? { ...existing, ...tile, thumbnail: existing.thumbnail ?? tile.thumbnail } : tile;
	const rest = old.items.filter((i) => !same(i, tile));
	const lead = rest[0]?.kind === 'liked' ? 1 : 0;
	const items = [...rest.slice(0, lead), moved, ...rest.slice(lead)].slice(0, TILES);
	const out = shelves.slice();
	out[at] = { ...old, items };
	return out;
}

/** Replace the local shelves with fresh ones; shelves that came back empty go, new ones join next
 *  to the other local shelves (the grid first). YouTube's shelves stay as they are. */
export function mergeLocal<S extends HomeShelfLike>(shelves: S[], local: S[]): S[] {
	const fresh = new Map(local.map((s) => [s.id, s]));
	const out: S[] = [];
	const placed = new Set<string>();
	for (const s of shelves) {
		if (!LOCAL_SHELVES.includes(s.id)) {
			out.push(s);
			continue;
		}
		const f = fresh.get(s.id);
		if (f) {
			out.push(f);
			placed.add(s.id);
		}
	}
	for (const s of local) {
		if (placed.has(s.id)) continue;
		if (s.id === 'shortcuts') {
			out.unshift(s);
		} else {
			// After the last local shelf (the order `local_shelves` returns them in).
			let i = -1;
			out.forEach((x, n) => {
				if (LOCAL_SHELVES.includes(x.id) && LOCAL_SHELVES.indexOf(x.id) < LOCAL_SHELVES.indexOf(s.id)) i = n;
			});
			out.splice(i + 1, 0, s);
		}
		placed.add(s.id);
	}
	return out;
}

/** Recents' sort key (seconds): the latest of when an item was played from, added to or edited. */
export function recentsKey(...stamps: (number | null | undefined)[]): number {
	return Math.max(0, ...stamps.map((n) => (n ? (n > 1e12 ? Math.floor(n / 1000) : n) : 0)));
}
