// Self-check for the picture's sync math (videosync.ts).
//
//     node --experimental-strip-types ui/src/lib/videosync.check.ts
import { GTK, MAC, jumped, seekTarget, steadyStep, trimRate, videoHeightFor, type Trim } from './videosync.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const near = (a: number, b: number) => Math.abs(a - b) < 1e-9;

// Quality: device pixels, a 720p floor, a 1080p ceiling.
ok(videoHeightFor(200, 1) === 720, 'a tiny box still gets 720p, never 360p');
ok(videoHeightFor(500, 1) === 720, '500 px on a 1x screen');
ok(videoHeightFor(500, 2) === 1080, 'the same box on Retina is 1000 px: 1080p');
ok(videoHeightFor(1400, 2) === 1080, 'theater on Retina: capped at 1080p');
ok(videoHeightFor(800, 0) === 1080, 'no dpr reads as 1x');

// Seek lead: music, not wall time.
ok(seekTarget(100, 1, false, false, MAC) === 101, '1x: one second ahead');
ok(seekTarget(100, 3, false, false, MAC) === 103, '3x: three seconds of music ahead');
ok(seekTarget(100, 3, true, false, MAC) === 100, 'paused: no lead');
ok(seekTarget(100, 2, false, true, MAC) === 100, 'buffered: lands within a frame, no lead');

// Trim rates follow the tempo.
ok(near(trimRate(0.2, 1), 1.1), 'small lag: 10% faster');
ok(near(trimRate(-0.2, 1), 0.9), 'small lead: 10% slower');
ok(near(trimRate(0.8, 2), 2.5), '0.8 s behind at 2x: 25% faster than 2x');
ok(near(trimRate(2, 3), 4.5), '2 s behind at 3x: 50% faster than 3x');

// Steady state.
const idle: Trim = { trimming: false, trimSpeed: 1, outOfBand: 0 };
let r = steadyStep(3, 1, false, 1, idle, MAC);
ok(r.step.kind === 'seek', 'past seekFrom: one seek');
r = steadyStep(3, 1, true, 1, idle, MAC);
ok(r.step.kind === 'seek', 'a scrub while paused still moves the picture');
r = steadyStep(0.05, 1, false, 1, idle, MAC);
ok(r.step.kind === 'none' && r.st.outOfBand === 0, 'in band: nothing');
r = steadyStep(0.2, 1, true, 1, idle, MAC);
ok(r.step.kind === 'none', 'paused: nothing to trim');

// A sustained 0.2 s lag: the old GTK band never touched it; the Mac band trims it after 3 ticks.
let st = idle;
for (let i = 0; i < 20; i++) {
	r = steadyStep(0.2, 1, false, 1, st, GTK);
	ok(r.step.kind === 'none', 'GTK: 0.2 s is inside its band');
	st = r.st;
}
st = idle;
const kinds: string[] = [];
for (let i = 0; i < 3; i++) {
	r = steadyStep(0.2, 1, false, 1, st, MAC);
	kinds.push(r.step.kind);
	st = r.st;
}
ok(kinds.join() === 'none,none,rate', `Mac: trims on the third tick (${kinds})`);
ok(r.step.kind === 'rate' && near(r.step.rate, 1.1) && st.trimming, 'catches up at 1.1x');
// Held while closing, then back to the tempo once inside trimTo.
r = steadyStep(0.1, 1, false, 1.1, st, MAC);
ok(r.step.kind === 'none' && r.st.trimming, 'still closing: hold, write nothing');
r = steadyStep(0.03, 1, false, 1.1, r.st, MAC);
ok(r.step.kind === 'rate' && r.step.rate === 1 && !r.st.trimming, 'closed: back to 1x');

// A tempo change mid-trim: drop the trim and follow the new speed at once.
r = steadyStep(0.1, 2.5, false, 1.1, { trimming: true, trimSpeed: 1, outOfBand: 0 }, MAC);
ok(r.step.kind === 'rate' && r.step.rate === 2.5 && !r.st.trimming, 'follows 2.5x at once');
// And at 3x a lag trims relative to 3x.
st = { trimming: false, trimSpeed: 3, outOfBand: 2 };
r = steadyStep(-0.6, 3, false, 3, st, MAC);
ok(r.step.kind === 'rate' && near(r.step.rate, 2.25), 'ahead at 3x: slows to 2.25x');

// Jumps in mpv's clock (seek, chapter jump) vs. just running.
ok(!jumped(null, 10, 0, 1, false), 'no history');
ok(!jumped({ pos: 10, at: 0 }, 10.25, 250, 1, false), 'a normal tick');
ok(!jumped({ pos: 10, at: 0 }, 10.75, 250, 3, false), 'a normal tick at 3x');
ok(jumped({ pos: 10, at: 0 }, 300, 250, 1, false), 'a chapter jump');
ok(jumped({ pos: 300, at: 0 }, 290, 250, 1, false), 'a skip back');
ok(!jumped({ pos: 10, at: 0 }, 10, 5000, 1, true), 'paused: standing still is no jump');
ok(jumped({ pos: 10, at: 0 }, 40, 5000, 1, true), 'a scrub while paused is');

console.log('ok');
