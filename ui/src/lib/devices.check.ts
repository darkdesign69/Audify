// Self-check for the device-sync helpers in `devices.ts` (the iPhone's DeviceSyncLogicTests check
// the same rules):
//
//     node --experimental-strip-types ui/src/lib/devices.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant. Not imported by the app.
import {
	EMPTY_CODE,
	backspace,
	codeComplete,
	codeOf,
	countdown,
	formatCode,
	hintDue,
	hostOutcome,
	isPairLink,
	joinError,
	peerNames,
	relTime,
	statusView,
	typeInto,
	unlinkTitle
} from './devices.ts';
import type { SyncStatus } from './api';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const eq = (a: unknown, b: unknown, what: string) => ok(JSON.stringify(a) === JSON.stringify(b), `${what}: ${JSON.stringify(a)}`);

// --- countdown and times ------------------------------------------------------------------------
eq(countdown(1300, 1000), '5:00', 'five minutes');
eq(countdown(1299.2, 1000), '5:00', 'a part second rounds up to the full one');
eq(countdown(1059, 1000), '0:59', 'seconds padded');
eq(countdown(1009, 1000), '0:09', 'single digit padded');
eq(countdown(900, 1000), '0:00', 'never negative');
eq(relTime(1000, 1030), 'just now', 'under a minute');
eq(relTime(1000, 1000 + 120), '2 min ago', 'minutes');
eq(relTime(1000, 1000 + 3 * 3600 + 5), '3 h ago', 'hours');
eq(relTime(1000, 1000 + 86400 + 5), '1 day ago', 'one day');
eq(relTime(1000, 1000 + 5 * 86400), '5 days ago', 'days');
eq(relTime(2000, 1000), 'just now', 'a clock a bit ahead reads as now');
eq(formatCode('482913'), '482 913', 'big code in two halves');

// --- names and status ---------------------------------------------------------------------------
eq(peerNames(['iPhone']), 'iPhone', 'one');
eq(peerNames(['iPhone', 'iPad']), 'iPhone and iPad', 'two');
eq(peerNames(['A', 'B', 'C', 'D']), 'A, B and 2 more', 'more');
eq(peerNames([]), 'your other device', 'none');

const base: SyncStatus = {
	paired: false,
	deviceId: 'd',
	deviceName: 'Mac',
	platform: 'mac',
	pairId: null,
	peers: [],
	pending: 0,
	lastSync: null,
	lastError: null,
	relay: { enabled: true, ok: null },
	lanPort: null,
	pairing: null,
	syncing: false,
	active: true
};
const peer = { id: 'p', name: "Alex's iPhone", platform: 'ios', lastSeen: 0, lan: true };
eq(statusView(base, 0).title, 'Not linked', 'not linked');
const linked = { ...base, paired: true, peers: [peer], lastSync: 1000 };
eq(statusView(linked, 1010).title, "Linked with Alex's iPhone", 'linked title');
eq(statusView(linked, 1010).line, 'Synced just now', 'synced just now');
eq(statusView(linked, 1130).line, 'Synced 2 min ago', 'synced minutes ago');
eq(statusView({ ...linked, lastSync: null }, 0).line, 'Waiting for connection', 'never synced');
eq(statusView({ ...linked, syncing: true }, 0).line, 'Syncing…', 'syncing');
const err = statusView({ ...linked, lastError: 'relay: 503', pending: 3 }, 1010);
eq(err.line, 'Waiting for connection', 'error waits');
eq(err.pending, '3 changes waiting to sync', 'pending plural');
ok(!!err.error && !err.error.includes('503'), 'the raw error never shows');
eq(statusView({ ...linked, pending: 1 }, 1010).pending, '1 change waiting to sync', 'pending singular');
eq(unlinkTitle(linked), "Unlink Alex's iPhone?", 'unlink title');
eq(unlinkTitle(base), 'Unlink this device?', 'unlink title alone');

// --- join errors --------------------------------------------------------------------------------
eq(joinError('wrong_code:2').message, "That code didn't match. 2 tries left.", 'tries left');
eq(joinError('wrong_code:1').message, "That code didn't match. 1 try left.", 'one try');
eq(joinError('wrong_code:0').kind, 'expired', 'last try spent');
eq(joinError('wrong_code').message, "That code didn't match.", 'no count');
eq(joinError('not_pairing').kind, 'expired', 'expired');
eq(joinError('no_host').kind, 'unreachable', 'nobody found');
eq(joinError('connect timeout').kind, 'unreachable', 'timeout');
eq(joinError('Connection refused (os error 61)').kind, 'unreachable', 'refused');
eq(joinError('No route to host (os error 65)').kind, 'permission', 'macOS local network denied');
eq(joinError('local_network_denied').kind, 'permission', 'denied');
eq(joinError('bad_key').kind, 'other', 'anything else');
ok(joinError('bad_key').message === "Couldn't link your devices. Try again.", 'other wording');

// --- code entry ---------------------------------------------------------------------------------
let r = typeInto(EMPTY_CODE, 0, '4');
eq(r, { boxes: ['4', '', '', '', '', ''], focus: 1 }, 'a digit moves on');
r = typeInto(r.boxes, 1, 'x');
eq(r.focus, 1, 'a letter is dropped');
eq(codeOf(r.boxes), '4', 'nothing added');
r = typeInto(EMPTY_CODE, 3, '482 913');
eq(codeOf(r.boxes), '482913', 'a pasted code fills every box wherever it lands');
ok(codeComplete(r.boxes), 'complete');
eq(r.focus, 5, 'focus stays on the last box');
r = typeInto(['4', '8', '', '', '', ''], 2, '29');
eq(codeOf(r.boxes), '4829', 'a short paste fills on from the box');
eq(r.focus, 4, 'and moves past it');
eq(backspace(['4', '8', '', '', '', ''], 2), { boxes: ['4', '', '', '', '', ''], focus: 1 }, 'backspace in an empty box clears the one before');
eq(backspace(['4', '8', '', '', '', ''], 1), { boxes: ['4', '', '', '', '', ''], focus: 1 }, 'backspace clears its own box');
ok(!codeComplete(['4', '8', '', '', '', '']), 'incomplete');
ok(isPairLink('spotless://pair?v=1&code=1'), 'pair link');
ok(!isPairLink('482913'), 'a code is not a link');

// --- hint and host outcome ----------------------------------------------------------------------
ok(hintDue({ paired: false, firstUse: 0, shown: false, now: 3 * 86400 }), 'three days');
ok(!hintDue({ paired: false, firstUse: 0, shown: false, now: 3 * 86400 - 1 }), 'not before');
ok(!hintDue({ paired: true, firstUse: 0, shown: false, now: 9e9 }), 'not when linked');
ok(!hintDue({ paired: false, firstUse: 0, shown: true, now: 9e9 }), 'once');
const pairing = { code: '482913', qr: '', expiresAt: 1300 };
eq(hostOutcome({ ...base, pairing }, 0, 1300, 1000), 'waiting', 'waiting');
eq(hostOutcome({ ...base, paired: true, peers: [peer] }, 0, 1300, 1000), 'linked', 'linked');
eq(hostOutcome(base, 0, 1300, 1300), 'expired', 'expired');
eq(hostOutcome(base, 0, 1300, 1100), 'tries', 'withdrawn after wrong tries');
eq(hostOutcome({ ...base, paired: true, peers: [peer] }, 1, 1300, 1100), 'tries', 'an existing peer is not a new link');

console.log('ok');
