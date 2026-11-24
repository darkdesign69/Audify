// The music/podcast picture's sync math (VideoSurface.svelte), as pure functions so it can be
// checked without a webview: videosync.check.ts.
//
// mpv is the master clock: it plays the sound, and the muted `<video>` element follows it. A small
// drift is closed by nudging `playbackRate` for a moment; a large one (a track change, a scrub, a
// chapter jump, a long stall) by one seek, aimed past what the re-buffer will cost.

export type Tuning = {
	/** Start trimming past this much drift (s)... */
	trimFrom: number;
	/** ...and stop once back inside this. */
	trimTo: number;
	/** How many ~4 Hz ticks the drift must stay out of band before a trim starts. */
	trimTicks: number;
	/** Past this, a trim would take too long to sit through: seek instead. */
	seekFrom: number;
	/** Wall-clock seconds an unbuffered seek costs; the target leads mpv by that much music. */
	seekCost: number;
};

/** macOS WebKit (AVFoundation underneath): a `playbackRate` write is seamless, so the bands are
 *  tight. The old 0.35 s band with a two-second guard let the picture sit a third of a second
 *  behind the voice indefinitely, which is the visible lip-sync lag the owner reported. */
export const MAC: Tuning = { trimFrom: 0.12, trimTo: 0.04, trimTicks: 3, seekFrom: 2.5, seekCost: 1 };

/** WebKitGTK: a rate write is a flushing GStreamer seek that flickers (#107), so a correction
 *  starts only after two seconds out of band and the rate is written twice per correction at most. */
export const GTK: Tuning = { trimFrom: 0.35, trimTo: 0.15, trimTicks: 8, seekFrom: 2.5, seekCost: 1 };

/** The picture height to ask the core for. Device pixels, not CSS ones: a Retina Mac's box is
 *  twice its CSS height, and asking by CSS pixels is what handed it 360p and 480p. Never below
 *  720p (the low picture the owner saw), never above 1080p (the core's ceiling). */
export function videoHeightFor(cssPx: number, dpr: number): number {
	const px = cssPx * (dpr > 0 ? dpr : 1);
	return px > 720 ? 1080 : 720;
}

/** Where a seek should aim. An unbuffered seek costs about `seekCost` wall-clock seconds, during
 *  which mpv plays on at `speed`; so the lead is that much *music*, not that much time. A lead
 *  of one second at 3x landed two seconds behind. A buffered seek lands within a frame, and
 *  while paused nothing moves: neither gets a lead. */
export function seekTarget(mpv: number, speed: number, paused: boolean, buffered: boolean, t: Tuning) {
	return mpv + (paused || buffered ? 0 : t.seekCost * (speed || 1));
}

/** Catch-up rate for a gap, picked once when a trim starts and held until it closes. */
export function trimRate(drift: number, speed: number) {
	const a = Math.abs(drift);
	const k = a > 1.2 ? 0.5 : a > 0.5 ? 0.25 : 0.1;
	return speed * (1 + Math.sign(drift) * k);
}

/** Whether mpv's clock jumped since the last sample (a seek, a chapter jump, a skip) rather than
 *  just ran. Such a jump re-runs the converge phase at once, instead of waiting out the seek
 *  cooldown with the picture seconds away. */
export function jumped(
	prev: { pos: number; at: number } | null,
	pos: number,
	at: number,
	speed: number,
	paused: boolean
) {
	if (!prev) return false;
	const expected = paused ? prev.pos : prev.pos + ((at - prev.at) / 1000) * speed;
	return Math.abs(pos - expected) > 1.5;
}

export type Trim = { trimming: boolean; trimSpeed: number; outOfBand: number };
export type Step =
	| { kind: 'none' }
	| { kind: 'seek' }
	| { kind: 'rate'; rate: number };

/**
 * One steady-state tick, once the picture has converged. `drift` is mpv minus the picture
 * (positive: the picture is behind), `rate` the element's current `playbackRate`. Returns what to
 * do and the trim state to keep. The caller owns the seek cooldown.
 */
export function steadyStep(
	drift: number,
	speed: number,
	paused: boolean,
	rate: number,
	st: Trim,
	t: Tuning
): { step: Step; st: Trim } {
	const a = Math.abs(drift);
	if (a > t.seekFrom) return { step: { kind: 'seek' }, st: { ...st, trimming: false, outOfBand: 0 } };
	if (paused) return { step: { kind: 'none' }, st };
	let s = { ...st };
	// A tempo change makes the held rate meaningless: it was picked against the old speed.
	if (s.trimming && s.trimSpeed !== speed) s.trimming = false;
	if (s.trimming) {
		if (a > t.trimTo) return { step: { kind: 'none' }, st: s }; // still closing
		s.trimming = false;
	}
	if (rate !== speed) return { step: { kind: 'rate', rate: speed }, st: s }; // end / follow tempo
	s.outOfBand = a > t.trimFrom ? s.outOfBand + 1 : 0;
	if (s.outOfBand < t.trimTicks) return { step: { kind: 'none' }, st: s };
	s = { trimming: true, trimSpeed: speed, outOfBand: 0 };
	return { step: { kind: 'rate', rate: trimRate(drift, speed) }, st: s };
}
