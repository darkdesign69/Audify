// Self-check for the header tint mapping (`tintmap.ts`):
//
//     node --experimental-strip-types ui/src/lib/tintmap.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant.
import { hexToOklab, tintFromHex } from './tintmap.ts';

const lab = (rgb: string) => {
	const [r, g, b] = rgb.match(/\d+/g)!.map(Number);
	return hexToOklab('#' + [r, g, b].map((c) => c.toString(16).padStart(2, '0')).join(''))!;
};
const dist = (a: number[], b: number[]) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);

// The body band sits at Spotify's fixed lightness, whatever the cover's own (± gamut clipping).
for (const hex of ['#f3ad09', '#041815', '#c2dce7', '#0068b1', '#d6494d']) {
	const L = lab(tintFromHex(hex).dark)[0];
	if (Math.abs(L - 0.32) > 0.02) throw new Error(`body L ${L} for ${hex}`);
}
// Neutral covers: Spotify's greys.
if (tintFromHex(null).dark !== 'rgb(53 53 53)' || tintFromHex(null).color !== 'rgb(83 83 83)') throw new Error('neutral');
const grey = lab(tintFromHex('#777777').dark);
if (Math.hypot(grey[1], grey[2]) > 0.005) throw new Error('grey body got a hue');
// Hue is kept: a red cover gives a red body.
const [r, g, b] = tintFromHex('#d6494d').dark.match(/\d+/g)!.map(Number);
if (!(r > g + 30 && r > b + 30)) throw new Error('red body lost its hue');
// White title text stays readable on the body (WCAG ≥ 4.5).
const lin = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
for (const hex of ['#f3ad09', '#c2dce7', '#ffffff', '#00ff00', '#ff00ff', '#ffff00']) {
	const [rr, gg, bb] = tintFromHex(hex).dark.match(/\d+/g)!.map((c) => lin(+c / 255));
	const c = 1.05 / (0.2126 * rr + 0.7152 * gg + 0.0722 * bb + 0.05);
	if (c < 4.5) throw new Error(`white contrast ${c.toFixed(2)} on ${hex}`);
}
// Distance to Spotify's real body colours: (our artcolor pick, Spotify header body) for the 42
// albums of docs/bench/colour.md. Mean CIEDE2000 there: 8.5 with the old L×0.72 band, 7.5 now.
const PAIRS: [string | null, string][] = [["#8998b1","#28354f"],[null,"#353535"],[null,"#35353d"],[null,"#353535"],[null,"#353535"],["#0068b1","#002f73"],["#69554b","#5b1e2f"],["#d6494d","#65000a"],["#363450","#401828"],["#4e8ab5","#003368"],["#ab9175","#003b36"],["#0377aa","#003367"],["#f3ad09","#572600"],["#eb6e72","#650006"],["#8a96a7","#3a333a"],["#0974aa","#003659"],[null,"#3a3434"],["#4c2819","#4d2d10"],["#cf9250","#353535"],[null,"#233939"],["#041815","#2a3922"],["#4a4870","#600041"],["#746653","#383030"],["#cb9076","#413131"],["#ad757a","#650006"],["#9a192b","#65000d"],["#2e293b","#302838"],["#896e54","#401818"],["#4f1715","#481010"],[null,"#3a3434"],["#8e4e37","#650005"],["#6c2f30","#63002c"],["#332515","#383030"],["#924a54","#650011"],["#c2dce7","#24383e"],["#90181a","#650002"],["#7578a7","#32305b"],["#71152a","#650012"],["#740327","#650021"],["#484f5a","#00365b"],[null,"#013659"],["#ecac8b","#640222"]];
const mean =
	PAIRS.reduce((s, [pick, sp]) => s + dist(lab(tintFromHex(pick).dark), hexToOklab(sp)!), 0) / PAIRS.length;
if (mean > 0.037) throw new Error(`mean body OKLab distance to Spotify ${mean.toFixed(4)}`);
console.log(`ok (mean body ΔEok to Spotify ${(mean * 100).toFixed(2)})`);
