// Spotless audio stream: the equalizer's frequency-response maths for the UI, the same RBJ
// Audio EQ Cookbook formulas FFmpeg's lowshelf/equalizer/highshelf filters use (and
// crates/player/src/dsp.rs), so the curve on screen is the curve the user hears. The magnitude
// and log-axis helpers follow DSSSP (NumberOneBot/dsssp, MIT) `math.ts` / `scale.ts`.
// No Tauri imports: a Node self-check (eq-curve.check.ts) runs it directly.

export type BandKind = 'lowshelf' | 'peaking' | 'highshelf';
export interface EqBand {
	kind: BandKind;
	freq: number;
	q: number;
}

/** Spotify's six bands. Mirrors `dsp::EQ_BANDS`; `audioEqInfo()` returns the same from Rust. */
export const EQ_BANDS: EqBand[] = [
	{ kind: 'lowshelf', freq: 60, q: 0.7 },
	{ kind: 'peaking', freq: 150, q: 1 },
	{ kind: 'peaking', freq: 400, q: 1 },
	{ kind: 'peaking', freq: 1000, q: 1 },
	{ kind: 'peaking', freq: 2400, q: 1 },
	{ kind: 'highshelf', freq: 15000, q: 0.7 }
];
export const EQ_MAX_DB = 12;
export const EQ_RATE = 48000;
export const F_MIN = 20;
export const F_MAX = 20000;

export interface Biquad {
	b0: number;
	b1: number;
	b2: number;
	a1: number;
	a2: number;
}

export function biquad(kind: BandKind, freq: number, q: number, gainDb: number, rate = EQ_RATE): Biquad {
	const A = Math.pow(10, gainDb / 40);
	const w0 = (2 * Math.PI * freq) / rate;
	const cos = Math.cos(w0);
	const alpha = Math.sin(w0) / (2 * q);
	let b0: number, b1: number, b2: number, a0: number, a1: number, a2: number;
	if (kind === 'peaking') {
		[b0, b1, b2] = [1 + alpha * A, -2 * cos, 1 - alpha * A];
		[a0, a1, a2] = [1 + alpha / A, -2 * cos, 1 - alpha / A];
	} else {
		const k = 2 * Math.sqrt(A) * alpha;
		const s = kind === 'lowshelf' ? 1 : -1;
		b0 = A * (A + 1 - s * (A - 1) * cos + k);
		b1 = s * 2 * A * (A - 1 - s * (A + 1) * cos);
		b2 = A * (A + 1 - s * (A - 1) * cos - k);
		a0 = A + 1 + s * (A - 1) * cos + k;
		a1 = -s * 2 * (A - 1 + s * (A + 1) * cos);
		a2 = A + 1 + s * (A - 1) * cos - k;
	}
	return { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 };
}

/** |H(e^jw)| in dB. */
export function magnitudeDb(c: Biquad, freq: number, rate = EQ_RATE): number {
	const w = (2 * Math.PI * freq) / rate;
	const [c1, s1, c2, s2] = [Math.cos(w), Math.sin(w), Math.cos(2 * w), Math.sin(2 * w)];
	const nr = c.b0 + c.b1 * c1 + c.b2 * c2;
	const ni = -(c.b1 * s1 + c.b2 * s2);
	const dr = 1 + c.a1 * c1 + c.a2 * c2;
	const di = -(c.a1 * s1 + c.a2 * s2);
	return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** The summed response of all bands at `freq` (cascaded biquads: dB adds). */
export function responseDb(gains: number[], freq: number, bands: EqBand[] = EQ_BANDS): number {
	let sum = 0;
	bands.forEach((b, i) => {
		const g = gains[i] ?? 0;
		if (g !== 0) sum += magnitudeDb(biquad(b.kind, b.freq, b.q, g), freq);
	});
	return sum;
}

/** Log-spaced frequencies from F_MIN to F_MAX. */
export function logFrequencies(n = 200): number[] {
	const lo = Math.log(F_MIN);
	const hi = Math.log(F_MAX);
	return Array.from({ length: n }, (_, i) => Math.exp(lo + ((hi - lo) * i) / (n - 1)));
}

/** Frequency → 0..1 across the log axis, and back. */
export const freqToX = (f: number) => (Math.log(f) - Math.log(F_MIN)) / (Math.log(F_MAX) - Math.log(F_MIN));
export const xToFreq = (x: number) => Math.exp(Math.log(F_MIN) + x * (Math.log(F_MAX) - Math.log(F_MIN)));

/** The automatic pre-gain the player applies (Rust `dsp::auto_preamp`): minus the curve's peak
 *  above 0 dB, capped at -12. For display only; the player computes its own. */
export function autoPreamp(gains: number[], bands: EqBand[] = EQ_BANDS): number {
	let peak = -Infinity;
	for (const f of logFrequencies(241)) peak = Math.max(peak, responseDb(gains, f, bands));
	const p = -Math.max(0, peak);
	return Math.round(Math.max(p, -EQ_MAX_DB) * 100) / 100 + 0;
}

/** Clamp to ±12 and round to 0.1 dB, like the player does. */
export function clampGain(g: number): number {
	if (!Number.isFinite(g)) return 0;
	return Math.round(Math.max(-EQ_MAX_DB, Math.min(EQ_MAX_DB, g)) * 10) / 10 + 0;
}

/** "60Hz", "1KHz", "2.4KHz", Spotify's axis labels. */
export function freqLabel(f: number): string {
	return f >= 1000 ? `${+(f / 1000).toFixed(1)}KHz` : `${f}Hz`;
}
