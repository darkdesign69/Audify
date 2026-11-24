// Run: node --experimental-strip-types ui/src/lib/eq-curve.check.ts
import { EQ_BANDS, autoPreamp, biquad, clampGain, freqLabel, freqToX, magnitudeDb, responseDb, xToFreq } from './eq-curve.ts';

function ok(condition: boolean, message: string) {
	if (!condition) throw new Error(message);
}
const near = (a: number, b: number, eps = 0.05) => Math.abs(a - b) < eps;

for (const b of EQ_BANDS) {
	const c = biquad(b.kind, b.freq, b.q, 0);
	for (const f of [30, 500, 5000, 19000]) ok(near(magnitudeDb(c, f), 0, 1e-9), `0 dB ${b.kind} is the identity`);
}
ok(near(magnitudeDb(biquad('peaking', 1000, 1, 6), 1000), 6, 1e-9), 'peak hits its gain at its centre');
ok(near(magnitudeDb(biquad('lowshelf', 60, 0.7, -9), 60), -4.5), 'shelf is half way at the corner');
ok(near(magnitudeDb(biquad('lowshelf', 60, 0.7, -9), 10), -9, 0.2), 'low shelf full gain far below');
ok(near(magnitudeDb(biquad('highshelf', 15000, 0.7, 12), 15000), 6), 'high shelf half way at the corner');
ok(near(magnitudeDb(biquad('highshelf', 15000, 0.7, 12), 200), 0), 'high shelf flat far below');
// Same numbers the Rust tests pin (dsp.rs preamp_keeps_the_peak_at_zero).
ok(autoPreamp([0, 0, 0, 0, 0, 0]) === 0, 'flat needs no preamp');
ok(autoPreamp([-6, -6, -6, -6, -6, -6]) === 0, 'cuts need no preamp');
ok(near(autoPreamp([0, 0, 0, 6, 0, 0]), -6), 'one boost');
ok(autoPreamp([12, 12, 12, 12, 12, 12]) === -12, 'capped at -12');
ok(responseDb([0, 0, 6, 6, 0, 0], 997) > 6, 'overlapping boosts add');
ok(near(xToFreq(freqToX(1234)), 1234, 1e-6), 'log axis round-trips');
ok(freqToX(20) === 0 && near(freqToX(20000), 1, 1e-12), 'axis ends');
ok(clampGain(20) === 12 && clampGain(-13) === -12 && clampGain(NaN) === 0 && clampGain(1.234) === 1.2, 'clamp');
ok(Object.is(clampGain(-0.04), 0), 'no negative zero');
ok(['60Hz', '150Hz', '400Hz', '1KHz', '2.4KHz', '15KHz'].join() === EQ_BANDS.map((b) => freqLabel(b.freq)).join(), 'labels');
console.log('ok');
