// Self-check for the artwork accent picker (`artcolor.ts`). No test runner in `ui/` (see
// color.check.ts) — node 22 runs TypeScript directly:
//
//     node --experimental-strip-types ui/src/lib/artcolor.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant.
import { pickAccent, toAccent } from './artcolor.ts';
import { brightness, hexToHsv, hsvToHex } from './color.ts';

/** An RGBA buffer: `n` pixels of each colour, in order. */
const buf = (...runs: [[number, number, number], number][]) =>
	new Uint8ClampedArray(runs.flatMap(([[r, g, b], n]) => Array.from({ length: n * 4 }, (_, i) => [r, g, b, 255][i % 4])));

const hue = (hex: string) => Math.round(hexToHsv(hex)!.h);

// A cover that is mostly black with a red band: area must not beat colour, or every dark cover
// themes the app grey.
const red = pickAccent(buf([[8, 8, 8], 900], [[200, 30, 30], 124]))!;
if (Math.abs(hue(red) - 0) > 20 && Math.abs(hue(red) - 360) > 20) throw new Error(`hue: ${red}`);

// Banding lands on the brightness the theme asked for, and the two themes disagree: a dark theme's
// accent has to be brighter than the surfaces under it, a light theme's darker (#137). Every hue,
// including the two that HSV value gets wrong (blue is dark at v=1, yellow is bright at v=0.5).
for (const h of [0, 45, 60, 120, 180, 240, 280, 330]) {
	const src = hsvToHex({ h, s: 0.9, v: 0.5 });
	const dark = brightness(toAccent(src, true));
	const light = brightness(toAccent(src, false));
	if (Math.abs(dark - 0.65) > 0.02) throw new Error(`dark band at ${h}: ${dark}`);
	if (Math.abs(light - 0.39) > 0.02) throw new Error(`light band at ${h}: ${light}`);
	// Still a colour, not a wash: hue survives and something of the saturation does too.
	for (const out of [toAccent(src, true), toAccent(src, false)]) {
		const hsv = hexToHsv(out)!;
		if (hsv.s < 0.3) throw new Error(`washed out at ${h}: ${out}`);
		if (Math.min(Math.abs(hsv.h - h), 360 - Math.abs(hsv.h - h)) > 3) throw new Error(`hue drift: ${out}`);
	}
}

// A dark cover: JPEG noise in the black reads as "saturated" in HSV terms ((10,12,13) is 23%) and
// used to outvote the colour by area, so dark covers (Random Access Memories, Homework) tinted grey.
const noisy = pickAccent(
	buf([[10, 12, 13], 500], [[13, 9, 12], 400], [[24, 22, 30], 300], [[100, 140, 200], 60])
)!;
if (!noisy || Math.abs(hue(noisy) - 215) > 20) throw new Error(`dark noise won: ${noisy}`);
const maroon = pickAccent(buf([[12, 10, 10], 800], [[110, 18, 24], 200]))!;
if (!maroon || (hue(maroon) > 15 && hue(maroon) < 345)) throw new Error(`maroon: ${maroon}`);
// A muted cover (a dusty slate blue on black, chroma 0.08) still has a colour: Spotify tints it blue,
// and the 0.1 chroma floor used to call it grey (docs/bench/colour.md).
const slate = pickAccent(buf([[10, 10, 12], 700], [[100, 108, 120], 324]));
if (!slate || Math.abs(hue(slate) - 216) > 20) throw new Error(`muted blue: ${slate}`);
// A few stray coloured pixels on a black-and-white cover still leave it grey.
if (pickAccent(buf([[15, 15, 15], 600], [[230, 230, 230], 400], [[200, 40, 40], 5])) !== null)
	throw new Error('stray pixels made a colour');

// The picked colour comes back raw, so the cache stays valid across a light/dark flip.
if (toAccent(red, true) === toAccent(red, false)) throw new Error('band ignores the theme');

// Greyscale artwork keeps the user's theme instead of inventing a hue out of noise.
if (pickAccent(buf([[20, 20, 20], 512], [[210, 210, 212], 512])) !== null) throw new Error('grey');
if (pickAccent(new Uint8ClampedArray(0)) !== null) throw new Error('empty');

console.log('ok');
