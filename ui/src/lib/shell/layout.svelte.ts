// The app frame's state (docs/spotify-spec/screens/shell-layout.md): left library width/collapse,
// right panel width/view, lyrics takeover. Persisted to localStorage; remembered across launches.
import { browser } from '$app/environment';

export const LEFT_MIN = 280;
export const LEFT_MAX = 420;
export const LEFT_COLLAPSED = 72;
export const RIGHT_MIN = 280;
export const RIGHT_MAX = 420;

export type RightView = 'now-playing' | 'queue' | 'devices';

const KEY = 'encore:shell';

type Saved = {
	leftWidth: number;
	leftCollapsed: boolean;
	rightWidth: number;
	rightView: RightView | null;
};

function load(): Saved {
	const d: Saved = { leftWidth: LEFT_MIN, leftCollapsed: false, rightWidth: RIGHT_MIN, rightView: 'now-playing' };
	if (!browser) return d;
	try {
		const s = JSON.parse(localStorage.getItem(KEY) ?? '{}');
		return {
			leftWidth: clamp(Number(s.leftWidth) || d.leftWidth, LEFT_MIN, LEFT_MAX),
			leftCollapsed: s.leftCollapsed === true,
			rightWidth: clamp(Number(s.rightWidth) || d.rightWidth, RIGHT_MIN, RIGHT_MAX),
			rightView: ['now-playing', 'queue', 'devices'].includes(s.rightView)
				? s.rightView
				: s.rightView === null
					? null
					: d.rightView
		};
	} catch {
		return d;
	}
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

const init = load();

export const shell = $state({
	leftWidth: init.leftWidth,
	leftCollapsed: init.leftCollapsed,
	rightWidth: init.rightWidth,
	/** What the right panel shows; null = closed. */
	rightView: init.rightView as RightView | null,
	/** Lyrics take over the left sidebar + main (spec), the right panel stays. */
	lyricsOpen: false,
	/** The library opened full-width in the main view (↗). */
	libraryExpanded: false,
	/** Live drag in progress (disables text selection, shows col-resize cursor everywhere). */
	resizing: false,
	/** The window's width (bound in the root layout); 0 = unknown. Not persisted. */
	windowWidth: browser ? window.innerWidth : 0,
	/** The user opened the library although the window is narrow (see `leftRail`). Not persisted. */
	leftForcedOpen: false
});

/** The main view's smallest comfortable width. Below it a narrow window shows Your Library as the
 *  72px rail on its own, like Spotify, and widens it again when there is room. */
export const MAIN_MIN = 400;
/** Would the open library leave the main view narrower than MAIN_MIN? (8px gutters + 8px handles.) */
export function narrowWindow(): boolean {
	const w = shell.windowWidth;
	if (!w) return false;
	const right = shell.rightView ? shell.rightWidth + 8 : 0;
	return w - 16 - shell.leftWidth - 8 - right < MAIN_MIN;
}
/** Is Your Library shown as the rail (collapsed by the user, or by a narrow window)? */
export const leftRail = () => shell.leftCollapsed || (narrowWindow() && !shell.leftForcedOpen);

function save() {
	if (!browser) return;
	localStorage.setItem(
		KEY,
		JSON.stringify({
			leftWidth: shell.leftWidth,
			leftCollapsed: shell.leftCollapsed,
			rightWidth: shell.rightWidth,
			rightView: shell.rightView
		} satisfies Saved)
	);
}

/** The left column's rendered width. */
export const leftPx = () => (leftRail() ? LEFT_COLLAPSED : shell.leftWidth);

export function setLeftWidth(px: number) {
	// Dragging well below min collapses; between collapse threshold and min snaps to min.
	if (px < (LEFT_MIN + LEFT_COLLAPSED) / 2) {
		shell.leftCollapsed = true;
	} else {
		shell.leftCollapsed = false;
		shell.leftForcedOpen = true;
		shell.leftWidth = clamp(px, LEFT_MIN, LEFT_MAX);
	}
	save();
}
export function toggleLeft() {
	if (!shell.leftCollapsed && narrowWindow()) {
		// The window collapsed it, not the user: open/close it for now without touching the saved state.
		shell.leftForcedOpen = !shell.leftForcedOpen;
		return;
	}
	shell.leftForcedOpen = !shell.leftCollapsed ? false : true;
	shell.leftCollapsed = !shell.leftCollapsed;
	save();
}
export function nudgeLeft(delta: number) {
	if (leftRail() && delta > 0) {
		shell.leftCollapsed = false;
		shell.leftForcedOpen = true;
	} else if (shell.leftCollapsed && delta > 0) shell.leftCollapsed = false;
	else setLeftWidth(shell.leftWidth + delta);
	save();
}

export function setRightWidth(px: number) {
	shell.rightWidth = clamp(px, RIGHT_MIN, RIGHT_MAX);
	save();
}
export function nudgeRight(delta: number) {
	setRightWidth(shell.rightWidth + delta);
}

/** Player-bar buttons: open `view`, or close the panel if it is already showing it. */
export function toggleRight(view: RightView = 'now-playing') {
	shell.rightView = shell.rightView === view ? null : view;
	save();
}
export function openRight(view: RightView) {
	shell.rightView = view;
	save();
}
export function closeRight() {
	shell.rightView = null;
	save();
}
export function toggleLyrics() {
	shell.lyricsOpen = !shell.lyricsOpen;
}
