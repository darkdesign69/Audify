// Device sync, the words and the pure logic behind the screens (docs/SYNC.md §9). The iPhone has
// the same strings and rules in `DeviceSync.swift`: change one, change the other.
// Self-check: devices.check.ts.
import type { SyncStatus } from './api';

/** This app runs on the Mac, so "your other device" is the iPhone. */
export const OTHER = 'iPhone';

export const W = {
	menuLink: `Link your ${OTHER}`,
	menuLinked: 'Linked devices',
	section: 'Devices',
	notLinked: 'Not linked',
	notLinkedSub: `Link your ${OTHER} to keep your library, playlists, history, podcasts and settings in sync.`,
	linkDevice: 'Link a device',
	syncNow: 'Sync now',
	unlink: 'Unlink',
	unlinkBody: 'Your devices stop syncing. Everything stays on both devices.',
	errorLine: "Can't reach your other device or the sync service right now. Changes will go through when it's back.",
	introTitle: `Link your ${OTHER}`,
	introBody: `Open Spotless on your ${OTHER} and go to Settings › Devices › Link a device. Then show a code on one device and enter it on the other.`,
	showCode: 'Show code',
	enterCode: 'Enter code',
	localNetwork: 'Spotless will ask to find devices on your local network. Allow it so your devices can link.',
	showHeading: 'Enter this code on your other device',
	qrCaption: "Or scan this code with your iPhone's camera.",
	waiting: 'Waiting for your other device…',
	cancel: 'Cancel',
	expired: 'This code has expired.',
	tooManyTries: 'Too many wrong tries. Show a new code to try again.',
	newCode: 'Show a new code',
	enterHeading: 'Enter the code from your other device',
	link: 'Link',
	linking: 'Linking…',
	successTitle: 'Linked',
	successBody:
		'Everything now stays in sync between your devices: library, playlists, history, podcasts and settings. Downloads stay on each device.',
	done: 'Done',
	welcomeTitle: `Using Spotless on your ${OTHER} too?`,
	welcomeLine: 'Link them to keep everything in sync.',
	linkNow: 'Link now',
	notNow: 'Not now',
	hint: `Have Spotless on your ${OTHER}? Link it to keep everything in sync.`,
	openSystemSettings: 'Open System Settings'
} as const;

export const unlinkTitle = (s: Pick<SyncStatus, 'peers'>) =>
	s.peers.length ? `Unlink ${peerNames(s.peers.map((p) => p.name))}?` : 'Unlink this device?';

/** "A", "A and B", "A, B and 2 more". */
export function peerNames(names: string[]): string {
	const n = names.map((x) => x.trim() || 'your other device');
	if (n.length <= 1) return n[0] ?? 'your other device';
	if (n.length === 2) return `${n[0]} and ${n[1]}`;
	return `${n[0]}, ${n[1]} and ${n.length - 2} more`;
}

/** "just now", "4 min ago", "3 h ago", "1 day ago", "5 days ago". */
export function relTime(thenSecs: number, nowSecs: number): string {
	const d = Math.max(0, nowSecs - thenSecs);
	if (d < 60) return 'just now';
	if (d < 3600) return `${Math.floor(d / 60)} min ago`;
	if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
	const days = Math.floor(d / 86400);
	return days === 1 ? '1 day ago' : `${days} days ago`;
}

/** Seconds left as m:ss, never below 0:00. */
export function countdown(expiresAtSecs: number, nowSecs: number): string {
	const left = Math.max(0, Math.ceil(expiresAtSecs - nowSecs));
	return `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`;
}

/** "482913" → "482 913" (the big code). */
export const formatCode = (code: string) => (code.length === 6 ? `${code.slice(0, 3)} ${code.slice(3)}` : code);

export type StatusView = { title: string; line: string; pending: string | null; error: string | null };

/** What Settings › Devices says. `now` in seconds. */
export function statusView(s: SyncStatus, now: number): StatusView {
	if (!s.paired) return { title: W.notLinked, line: W.notLinkedSub, pending: null, error: null };
	const title = `Linked with ${peerNames(s.peers.map((p) => p.name))}`;
	const line = s.syncing
		? 'Syncing…'
		: s.lastError
			? 'Waiting for connection'
			: s.lastSync
				? `Synced ${relTime(s.lastSync, now)}`
				: 'Waiting for connection';
	const pending = s.pending > 0 ? `${s.pending} ${s.pending === 1 ? 'change' : 'changes'} waiting to sync` : null;
	return { title, line, pending, error: s.lastError ? W.errorLine : null };
}

export type JoinError = { message: string; kind: 'wrong' | 'expired' | 'unreachable' | 'permission' | 'other' };

/** A core error from `sync_join` in plain words. */
export function joinError(raw: unknown): JoinError {
	const e = String(raw ?? '').replace(/^Error:\s*/, '').trim();
	const wrong = /^wrong_code(?::(\d+))?$/.exec(e);
	if (wrong) {
		if (wrong[1] === undefined) return { kind: 'wrong', message: "That code didn't match." };
		const left = Number(wrong[1]);
		if (left === 0)
			return {
				kind: 'expired',
				message: "That code didn't match, and that was the last try. Show a new code on your other device."
			};
		return { kind: 'wrong', message: `That code didn't match. ${left} ${left === 1 ? 'try' : 'tries'} left.` };
	}
	if (e === 'not_pairing') return { kind: 'expired', message: 'This code has expired. Show a new code on your other device.' };
	if (e === 'bad_code') return { kind: 'wrong', message: 'Enter all 6 digits.' };
	if (e === 'local_network_denied' || /os error 65\b|no route to host/i.test(e))
		return {
			kind: 'permission',
			message: "Spotless can't reach your local network. Allow it in System Settings › Privacy & Security › Local Network."
		};
	if (/no_host|timeout|timed out|closed|refused|unreachable|os error (60|61|64)\b/i.test(e))
		return { kind: 'unreachable', message: "Your devices can't see each other. Make sure both are on the same Wi-Fi." };
	return { kind: 'other', message: "Couldn't link your devices. Try again." };
}

/** macOS's Local Network pane. */
export const LOCAL_NETWORK_SETTINGS = 'x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork';

// ── code entry (six boxes) ──

export const EMPTY_CODE = ['', '', '', '', '', ''];

export const isPairLink = (s: string) => /^\s*(spotless|encore):\/\/pair\?/i.test(s);

/**
 * Typing or pasting `text` into box `at`: digits fill from there on (a paste of a whole code
 * spreads over the boxes), anything else is dropped. Returns the boxes and the box to focus next.
 */
export function typeInto(boxes: string[], at: number, text: string): { boxes: string[]; focus: number } {
	const digits = text.replace(/\D/g, '');
	const out = boxes.slice(0, 6);
	while (out.length < 6) out.push('');
	if (!digits) return { boxes: out, focus: at };
	// A full code pasted anywhere replaces the whole thing.
	const start = digits.length >= 6 ? 0 : at;
	let i = start;
	for (const d of digits) {
		if (i > 5) break;
		out[i++] = d;
	}
	return { boxes: out, focus: Math.min(i, 5) };
}

/** Backspace in box `at`: clears it, or the one before when it is already empty. */
export function backspace(boxes: string[], at: number): { boxes: string[]; focus: number } {
	const out = boxes.slice();
	if (out[at]) {
		out[at] = '';
		return { boxes: out, focus: at };
	}
	const prev = Math.max(0, at - 1);
	out[prev] = '';
	return { boxes: out, focus: prev };
}

export const codeOf = (boxes: string[]) => boxes.join('');
export const codeComplete = (boxes: string[]) => /^\d{6}$/.test(codeOf(boxes));

// ── the one-time hint and the welcome screen ──

export const HINT_AFTER_SECS = 3 * 86400;

/** Home's "Have Spotless on your iPhone?" banner: not linked, used for 3 days, never shown before. */
export function hintDue(p: { paired: boolean; firstUse: number | null; shown: boolean; now: number }): boolean {
	return !p.paired && !p.shown && p.firstUse !== null && p.now - p.firstUse >= HINT_AFTER_SECS;
}

// ── the code screen's end ──

/**
 * The device showing the code learns how it ended from the status: the code is gone and there is
 * a new peer (linked), the clock ran out (expired), or the last wrong try withdrew it.
 */
export function hostOutcome(
	s: Pick<SyncStatus, 'pairing' | 'peers' | 'paired'>,
	peersAtStart: number,
	expiresAt: number,
	now: number
): 'waiting' | 'linked' | 'expired' | 'tries' {
	if (s.paired && s.peers.length > peersAtStart) return 'linked';
	if (s.pairing) return now >= expiresAt ? 'expired' : 'waiting';
	return now >= expiresAt - 1 ? 'expired' : 'tries';
}
