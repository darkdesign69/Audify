// Self-check for the scrubber's release settle (stream: chapters).
//
//     node --experimental-strip-types ui/src/lib/scrub.check.ts
import { releaseSpeed, settleOnRelease } from './scrub.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

// A 1000 s bar 1000 px wide: 1 s = 1 px. Starts at 0:00 (no detent), 60, 300 and 312.
const starts = [0, 60, 300, 312];
const settle = (pos: number, speed = 0, width = 1000) => settleOnRelease(starts, pos, 1000, width, speed);
ok(settle(64) === 60, '4 px after a start, slow: onto it');
ok(settle(56) === 60, '4 px before');
ok(settle(65) === 60, '5 px: the edge of the radius');
ok(settle(66) === 66, '6 px: exactly where released');
ok(settle(150) === 150, 'far from every start');
ok(settle(63, 149) === 60, 'a slow drag still settles');
ok(settle(63, 151) === 63, 'faster than 150 px/s: never');
ok(settle(60.5, 2000) === 60.5, 'a fling never snaps, even right on a start');
ok(settle(3) === 3, 'the 0:00 start is no detent');
ok(settle(307) === 312, 'two close starts: the nearest wins');
ok(settle(305) === 300, 'and the other way');
ok(settle(64, 0, 100) === 60, 'on screen, not a share: 4 s is 0.4 px on a 100 px bar');
ok(settle(64, 0, 10_000) === 64, 'and 40 px on a 10 000 px one');
ok(settleOnRelease(starts, 64, 0, 1000, 0) === 64, 'no length');
ok(settleOnRelease([], 64, 1000, 1000, 0) === 64, 'no chapters');
ok(settleOnRelease(starts, 64, 1000, 0, 0) === 64, 'no width');

// Release speed from pointer samples (ms).
const moving = Array.from({ length: 10 }, (_, i) => ({ x: i * 5, t: i * 16 }));
ok(Math.abs(releaseSpeed(moving, 150) - 312.5) < 1, 'let go while moving');
ok(releaseSpeed(moving, 300) === 0, 'rested before letting go');
const slow = Array.from({ length: 20 }, (_, i) => ({ x: i * 0.5, t: i * 16 }));
ok(Math.abs(releaseSpeed(slow, 310) - 31.25) < 1, 'slow');
ok(releaseSpeed([], 1) === 0, 'no samples');
ok(releaseSpeed([{ x: 4, t: 1 }], 1) === 0, 'one sample: a click');

console.log('ok');
