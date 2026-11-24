// Lyrics colours (lyrics-and-expanded-player.md, now-playing-panel.md "Lyrics preview card"):
// Spotify paints the lyrics view and the preview card in one saturated artwork-derived colour per
// track; lines still to come are dark (near-black in the same hue), the current line is white and
// sung lines dim white. Derived from the cover's dominant colour (`$lib/artcolor`) in OKLab:
// background L 0.30–0.40 (white text stays readable, ≥ 6:1) with the chroma kept and pushed a little
// (≤ 0.17), upcoming lines L 0.14. `soft` is the old light tint, for text over the darker
// fullscreen gradient where dark lines would vanish.
import { artworkAccent } from '$lib/artcolor';

export type LyricsPalette = {
	/** Panel / card background. */
	bg: string;
	/** Upcoming lines (dark), loading bars, the preview card's other lines. */
	line: string;
	/** A light tint of the hue: lines over a dark background (fullscreen). */
	soft: string;
	/** A darker band of the background, for gradients (fullscreen, artwork mode). */
	deep: string;
};

export const NEUTRAL_PALETTE: LyricsPalette = {
	bg: 'rgb(83 83 83)',
	line: 'rgb(18 18 18)',
	soft: 'rgb(179 179 179)',
	deep: 'rgb(40 40 40)'
};

const lin = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const gam = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);

function hexToOklab(hex: string): [number, number, number] | null {
	const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
	if (!m) return null;
	const n = parseInt(m[1], 16);
	const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => lin(c / 255));
	const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
	const mm = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
	const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
	return [
		0.2104542553 * l + 0.793617785 * mm - 0.0040720468 * s,
		1.9779984951 * l - 2.428592205 * mm + 0.4505937099 * s,
		0.0259040371 * l + 0.7827717662 * mm - 0.808675766 * s
	];
}

function oklch(L: number, C: number, h: number): string {
	const a = C * Math.cos(h);
	const b = C * Math.sin(h);
	const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
	const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
	const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
	const rgb = [
		4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
		-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
		-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s
	].map((v) => Math.round(Math.min(1, Math.max(0, gam(v))) * 255));
	return `rgb(${rgb.join(' ')})`;
}

/** The palette for a cover's dominant colour (hex), or the neutral grey one. */
export function paletteFromHex(hex: string | null | undefined): LyricsPalette {
	const lab = hex ? hexToOklab(hex) : null;
	if (!lab) return NEUTRAL_PALETTE;
	const [L0, a0, b0] = lab;
	const C0 = Math.hypot(a0, b0);
	// Near-grey covers: Spotify falls back to its grey lyrics card.
	if (C0 < 0.025) return NEUTRAL_PALETTE;
	const h = Math.atan2(b0, a0);
	const L = Math.min(Math.max(L0, 0.3), 0.4);
	// Vivid: Spotify's lyrics colours are saturated even for a muted cover. A dull cover's chroma
	// is lifted (×1.4), a loud one capped where sRGB still holds it at this lightness.
	const C = Math.min(C0 * 1.4, 0.17);
	return {
		bg: oklch(L, C, h),
		line: oklch(0.14, Math.min(C * 0.5, 0.05), h),
		soft: oklch(0.86, Math.min(C0 * 0.6, 0.06), h),
		deep: oklch(L * 0.55, C * 0.7, h)
	};
}

const cache = new Map<string, LyricsPalette>();

export async function paletteFor(url: string | null | undefined): Promise<LyricsPalette> {
	if (!url) return NEUTRAL_PALETTE;
	const hit = cache.get(url);
	if (hit) return hit;
	let p = NEUTRAL_PALETTE;
	try {
		p = paletteFromHex(await artworkAccent(url));
	} catch {
		// unreadable cover: neutral
	}
	if (cache.size > 200) cache.clear();
	cache.set(url, p);
	return p;
}

/** Reactive palette following a (reactive) cover URL. Construct during component init. */
export class LyricsTint {
	bg = $state(NEUTRAL_PALETTE.bg);
	line = $state(NEUTRAL_PALETTE.line);
	soft = $state(NEUTRAL_PALETTE.soft);
	deep = $state(NEUTRAL_PALETTE.deep);
	constructor(url: () => string | null | undefined) {
		$effect(() => {
			const u = url();
			let stale = false;
			paletteFor(u).then((p) => {
				if (stale) return;
				this.bg = p.bg;
				this.line = p.line;
				this.soft = p.soft;
				this.deep = p.deep;
			});
			return () => (stale = true);
		});
	}
}
