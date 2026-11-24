// Artwork-derived header tints (spec: interactions-motion "Header colour from artwork").
//
//   const tint = new ArtworkTint(() => coverUrl);   // in a component's <script>
//   tint.color   → 'rgb(83 83 83)'   header top / sticky bar / action-bar gradient source
//   tint.dark    → 'rgb(53 53 53)'   header body (darker band)
//   tint.rgb     → '83 83 83'        channels, for alpha: `rgb(${tint.rgb} / 0.7)`
//
// Or one-shot: `await tintFor(url)` → { color, dark, rgb }.
// The colour is the cover's dominant colour (`$lib/artcolor.ts` histogram), clamped in OKLCH to
// L ≤ 0.45 (white text stays readable) with chroma capped, hue kept; the body band sits at Spotify's
// fixed lightness (`$lib/tintmap.ts`, docs/bench/colour.md). Greyscale/unreadable covers fall back
// to Spotify's neutral rgb(83,83,83) / rgb(53,53,53).
import { artworkAccent } from '$lib/artcolor';
import { NEUTRAL_TINT, LIKED_TINT, tintFromHex, type Tint } from '$lib/tintmap';

export { NEUTRAL_TINT, LIKED_TINT, tintFromHex, type Tint };

const NEUTRAL = tintFromHex(null);

/** Tint for a cover URL (memoized by artcolor's cache). */
export async function tintFor(url: string | null | undefined): Promise<Tint> {
	if (!url) return NEUTRAL;
	try {
		return tintFromHex(await artworkAccent(url));
	} catch {
		return NEUTRAL;
	}
}

/** Parse 'rgb(r g b)' / 'rgb(r,g,b)' / '#rrggbb' into 'r g b' channels (for alpha variants). */
export function channels(color: string): string {
	const hex = /^#([0-9a-f]{6})$/i.exec(color.trim());
	if (hex) {
		const n = parseInt(hex[1], 16);
		return `${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255}`;
	}
	const m = color.match(/[\d.]+/g);
	return m && m.length >= 3 ? m.slice(0, 3).join(' ') : '83 83 83';
}

/** Reactive tint that follows a (reactive) cover URL getter. Construct during component init. */
export class ArtworkTint {
	color = $state(NEUTRAL.color);
	dark = $state(NEUTRAL.dark);
	rgb = $state(NEUTRAL.rgb);
	constructor(url: () => string | null | undefined) {
		$effect(() => {
			const u = url();
			let stale = false;
			tintFor(u).then((t) => {
				if (stale) return;
				this.color = t.color;
				this.dark = t.dark;
				this.rgb = t.rgb;
			});
			return () => (stale = true);
		});
	}
}
