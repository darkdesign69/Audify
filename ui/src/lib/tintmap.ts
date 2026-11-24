// Pure colour mapping behind the artwork header tints (`kit/tint.svelte.ts`), kept rune-free so
// `tintmap.check.ts` can run it in node.
//
// top (`color`): the cover colour clamped in OKLCH to L 0.30–0.45 (white text stays readable on
// the sticky bar) with chroma ≤ 0.13, hue kept.
// body (`dark`): Spotify's header band, always at OKLab L ≈ 0.32 with chroma ≤ 0.13, hue kept
// (bench, docs/bench/colour.md). Neutral covers get Spotify's rgb(83,83,83) / rgb(53,53,53).

export const NEUTRAL_TINT = 'rgb(83 83 83)';
export const LIKED_TINT = 'rgb(80 56 160)';

export type Tint = { color: string; dark: string; rgb: string };

const NEUTRAL: Tint = { color: NEUTRAL_TINT, dark: 'rgb(53 53 53)', rgb: '83 83 83' };
const BODY_L = 0.32;
const BODY_C = 0.13;

const lin = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
const gam = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);

export function hexToOklab(hex: string): [number, number, number] | null {
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

export function oklabToRgb(L: number, a: number, b: number): [number, number, number] {
	const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
	const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
	const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
	const r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
	const g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
	const bb = -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s;
	return [r, g, bb].map((v) => Math.round(Math.min(1, Math.max(0, gam(v))) * 255)) as [
		number,
		number,
		number
	];
}

/** Clamp a hex colour into a header tint (exported for tests/other callers). */
export function tintFromHex(hex: string | null | undefined): Tint {
	if (!hex) return NEUTRAL;
	const lab = hexToOklab(hex);
	if (!lab) return NEUTRAL;
	const [L0, a0, b0] = lab;
	const C0 = Math.hypot(a0, b0);
	const h = Math.atan2(b0, a0);
	const C = Math.min(C0, 0.13);
	const L = Math.min(Math.max(L0, 0.3), 0.45);
	const top = oklabToRgb(L, C * Math.cos(h), C * Math.sin(h));
	// ── stream: bench2 ── Spotify's body band: a fixed OKLab L 0.32 with chroma ≤ 0.13, hue kept
	// (read off 42 Spotify album headers, docs/bench/colour.md: body ΔE00 8.5 → 7.5).
	const dark = oklabToRgb(BODY_L, Math.min(C0, BODY_C) * Math.cos(h), Math.min(C0, BODY_C) * Math.sin(h));
	const rgb = top.join(' ');
	return { color: `rgb(${rgb})`, dark: `rgb(${dark.join(' ')})`, rgb };
}
