// stream: chapters. Chapter starts are soft detents on a scrubber, never magnets: the thumb
// follows the pointer the whole drag, and only a slow release a few pixels from a start settles
// onto it. Distances are on screen, not a share of the episode. Same release rule as
// `chapterSettle` in ios/Encore (ModelsFeatures.swift), which also damps the thumb near a start
// and ticks a haptic as it crosses one; a mouse is precise enough to need neither.

/** A release this close to a start (px) settles onto it... */
export const SETTLE_RADIUS_PX = 5;
/** ...only when the pointer was this slow (px / s) as it let go: flings land where they land. */
export const SETTLE_MAX_SPEED = 150;

/** Where a drag released at `pos` lands, for `starts` on a bar `width` px long showing 0..`max`:
 *  the nearest start strictly inside the bar when within `radius` px and the pointer moved at
 *  most `maxSpeed` px / s; otherwise exactly `pos`. */
export function settleOnRelease(
	starts: readonly number[],
	pos: number,
	max: number,
	width: number,
	speed: number,
	radius = SETTLE_RADIUS_PX,
	maxSpeed = SETTLE_MAX_SPEED
): number {
	if (!(max > 0) || !(width > 0) || Math.abs(speed) > maxSpeed) return pos;
	let best: number | null = null;
	for (const s of starts) {
		if (s <= 0 || s >= max) continue;
		if (best === null || Math.abs(s - pos) < Math.abs(best - pos)) best = s;
	}
	return best !== null && (Math.abs(best - pos) * width) / max <= radius ? best : pos;
}

export type DragSample = { x: number; t: number };

/** The pointer's speed as it let go (px / s, `t` in ms), from samples in order: over the last
 *  100 ms of movement, 0 when it had rested 80 ms before letting go at `end`. */
export function releaseSpeed(samples: readonly DragSample[], end: number): number {
	const last = samples.at(-1);
	if (!last || end - last.t >= 80) return 0;
	let first = samples[0];
	for (let i = samples.length - 1; i >= 0; i--)
		if (last.t - samples[i].t >= 100) {
			first = samples[i];
			break;
		}
	return last.t > first.t ? (Math.abs(last.x - first.x) * 1000) / (last.t - first.t) : 0;
}
