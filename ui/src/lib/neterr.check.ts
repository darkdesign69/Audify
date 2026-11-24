// Self-check for the network-error classifier in `neterr.ts`. There is no test runner in `ui/` and
// this doesn't warrant adding one, node 22 runs TypeScript directly:
//
//     node --experimental-strip-types ui/src/lib/neterr.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant. Not imported by the app.
import { isUnreachable, friendlyNetError, friendlyError, looksFriendly, rawError, DEFAULT_ERROR_WORDS as W } from './neterr.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

const OFFLINE = "Can't reach YouTube Music. Check your connection.";
const swap = (raw: string) => friendlyNetError(raw, OFFLINE);

// --- what Rust actually hands the webview -------------------------------------------------------
// reqwest 0.12 Display: the kind, then " for url (…)". Same string for DNS, refused, TLS, timeout.
const REQWEST = 'http: error sending request for url (https://music.youtube.com/youtubei/v1/browse)';
ok(isUnreachable(REQWEST), 'a bare reqwest transport failure is the network');
ok(swap(REQWEST) === OFFLINE, 'and it is replaced whole, with no dangling "http:"');

ok(
	swap('http: request or response body error for url (https://music.youtube.com/)') === OFFLINE,
	'a connection dropped mid-response is the network too'
);
ok(
	swap('could not reach YouTube. Check your connection and try again (no client answered)') ===
		OFFLINE,
	"the orchestrator's own Unreachable is translated rather than passed through"
);

// --- a message that wrapped the error keeps its own half ----------------------------------------
ok(
	swap('Couldn’t sort this playlist: http: error sending request for url (https://x/)') ===
		`Couldn’t sort this playlist: ${OFFLINE}`,
	'context before the transport error survives'
);

// --- everything else is left alone --------------------------------------------------------------
for (const other of [
	'Your YouTube Music session expired, open the account menu and sign in again.',
	'This track is already in the playlist.',
	'http: HTTP status client error (404 Not Found) for url (https://music.youtube.com/)',
	'json: invalid type: null, expected a string at line 1 column 8',
	"Couldn't load this track. The stream link was refused.",
	'this file is no longer on your disk: /home/me/gone.flac'
]) {
	ok(!isUnreachable(other), `not the network: ${other}`);
	ok(swap(other) === other, `passed through untouched: ${other}`);
}

// --- lane magic-d1: friendly error toasts (MAGIC-GAPS #21) --------------------------------------
// Already a sentence: passes through.
for (const fine of [
	'This album has no radio.',
	'YouTube has no radio for this.',
	"Couldn't load this track. The stream link was refused.",
	'Your YouTube Music session expired, open the account menu and sign in again.'
]) {
	ok(looksFriendly(fine), `friendly already: ${fine}`);
	ok(friendlyError(fine, "Couldn't start radio") === fine, `kept as is: ${fine}`);
}
// Raw text never reaches the toast.
const RAW = [
	'http: HTTP status client error (404 Not Found) for url (https://music.youtube.com/)',
	'json: invalid type: null, expected a string at line 1 column 8',
	"DataCloneError: Failed to execute 'structuredClone' on 'Window': #<Object> could not be cloned.",
	'no metadata client',
	'unknown radio kind: banana',
	'[object Object]',
	'TypeError: Cannot read properties of undefined (reading \'id\')'
];
for (const raw of RAW) {
	ok(!looksFriendly(raw), `not friendly: ${raw}`);
	const out = friendlyError(raw, "Couldn't start radio");
	ok(out === "Couldn't start radio. Try again.", `context sentence for ${raw}: got ${out}`);
	ok(!friendlyError(raw).includes(':'), `no raw text without a context either: ${raw}`);
}
ok(friendlyError(REQWEST, "Couldn't start radio") === W.unreachable, 'the network wins over the context');
ok(friendlyError('HTTP 429 Too Many Requests') === W.rateLimited, 'rate limit');
ok(friendlyError('http: HTTP status client error (401 Unauthorized)') === W.signIn, 'needs sign in');
ok(friendlyError('http: HTTP status client error (404 Not Found) for url (x)') === W.notFound, '404 without context');
ok(friendlyError(new Error('boom')) === W.generic, 'an Error with no context is the generic sentence');
ok(rawError(new TypeError('x')) === 'TypeError: x', 'rawError keeps the class for the log');
ok(rawError({ a: 1 }) === '{"a":1}', 'rawError serialises objects');
ok(friendlyError('oops', "Couldn't save.") === "Couldn't save. Try again.", 'no double full stop');

console.log('ok');
