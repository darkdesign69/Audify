// Telling "the network is down" apart from "YouTube refused this" in the one place the UI turns a
// Rust error into words. Everything below is string matching because that is all that survives the
// trip: every Tauri command returns `Result<_, String>`, so the typed error (`reqwest::Error`,
// `ResolveError`) is already flattened by the time the webview sees it.
//
// Pure on purpose: no i18n import, so `neterr.check.ts` can run it under plain node. Callers pass
// the translated sentence in.

/**
 * What a dead connection looks like by the time it reaches here.
 *
 * reqwest's `Display` is the whole message: DNS failure, refused connection, dead proxy, captive
 * portal and timeout all print `error sending request for url (…)` and nothing finer, because the
 * cause lives in `source()` which `to_string()` never walks. That is fine, since all five want the
 * same sentence from us. The rest of the alternatives cover the layers that do pass a cause
 * through: rustypipe, the loopback proxy, mpv, and the orchestrator's own `Unreachable`.
 *
 * The optional `http: ` / `json: ` prefix is `innertube::transport::Error`'s, and matching it here
 * is what makes a bare transport failure replace cleanly instead of leaving `http:` dangling.
 */
const UNREACHABLE =
	/(?:\b(?:http|json): )?(?:error sending request|request or response body error|error trying to connect|dns error|failed to lookup address|connection (?:refused|reset|closed)|network is unreachable|no route to host|operation timed out|could not reach YouTube)/i;

/** Is this error the network rather than the request? */
export function isUnreachable(raw: string): boolean {
	return UNREACHABLE.test(raw);
}

/**
 * Swap the transport's raw wording for `friendly`, keeping whatever context the caller wrapped it
 * in. `"Couldn't sort this playlist: http: error sending request for url (…)"` keeps its first
 * half and loses the second; a bare transport error becomes `friendly` outright.
 *
 * Anything that isn't a network failure comes back untouched: a raw message the user can copy into
 * an issue beats a vague "something went wrong".
 */
export function friendlyNetError(raw: string, friendly: string): string {
	const m = UNREACHABLE.exec(raw);
	if (!m) return raw;
	const head = raw.slice(0, m.index).trim();
	return head ? `${head} ${friendly}` : friendly;
}

// ── lane magic-d1 ── Friendly error toasts (MAGIC-GAPS #21) ──────────────────────────────────
//
// Spotify never shows an exception: "Couldn't start radio. Try again." The raw text still matters
// for a bug report, so the toast layer logs it (errlog → limusic.log) and shows one of these.

/** Things only a program says. Any of them in a message means it was not written for a person. */
const TECHNICAL =
	/(?:\b[A-Z][a-zA-Z]*Error\b|\bError:|\berror:|\bexception\b|\bpanicked\b|::|=>|\{|\}|\[object |\bundefined\b|\bnull\b|\bNaN\b|\bhttps?:\/\/|\bstatus(?: code)? \d{3}\b|\bHTTP \d{3}\b|\b[45]\d\d\b|\bjson\b|\bserde\b|\binvalid type\b|\bmissing field\b|\bunknown variant\b|\bexpected\b.*\bat line\b|\bstructuredClone\b|\bfailed to (?:execute|parse|deserialize|decode)\b|\bcommand \w+ not found\b|\bPOST\b|\bGET\b)/;

const RATE_LIMITED = /\b429\b|too many requests|rate.?limit/i;
const NEEDS_SIGN_IN = /\b401\b|sign.?in required|login required|not signed in|unauthori[sz]ed/i;
const FORBIDDEN = /\b403\b|forbidden|permission denied/i;
const NOT_FOUND = /\b404\b|not found|no longer available|unavailable/i;

/** Plain-language wording for each failure family. Callers can translate them. */
export type ErrorWords = {
	unreachable: string;
	rateLimited: string;
	signIn: string;
	forbidden: string;
	notFound: string;
	/** The fallback when nothing better fits. A caller's own `context` beats it. */
	generic: string;
};

export const DEFAULT_ERROR_WORDS: ErrorWords = {
	unreachable: "Can't reach YouTube Music. Check your connection.",
	rateLimited: 'YouTube is busy right now. Try again in a moment.',
	signIn: 'Sign in to do that.',
	forbidden: "YouTube didn't allow that.",
	notFound: "That isn't available any more.",
	generic: 'Something went wrong. Try again.'
};

/** Already a sentence for a person: short, starts with a capital, ends in a full stop, no code. */
export function looksFriendly(msg: string): boolean {
	const m = msg.trim();
	return (
		m.length > 0 &&
		m.length <= 140 &&
		/^[A-Z“"'‘]/.test(m) &&
		/[.!?…)”"']$/.test(m) &&
		!TECHNICAL.test(m)
	);
}

/** Any thrown value as text: an Error's message, a Tauri command's rejected string, anything. */
export function rawError(e: unknown): string {
	if (e instanceof Error) return `${e.name}: ${e.message}`;
	if (typeof e === 'string') return e;
	try {
		return JSON.stringify(e) ?? String(e);
	} catch {
		return String(e);
	}
}

/**
 * The sentence to show for `e`. A message that is already friendly passes through (Rust's own
 * "This album has no radio." is), a known failure family gets its sentence, and anything else
 * becomes `context` ("Couldn't start radio.") plus "Try again.", or the generic sentence.
 */
export function friendlyError(
	e: unknown,
	context?: string,
	words: ErrorWords = DEFAULT_ERROR_WORDS
): string {
	const raw = rawError(e);
	if (isUnreachable(raw)) return words.unreachable;
	if (looksFriendly(raw)) return raw.trim();
	if (RATE_LIMITED.test(raw)) return words.rateLimited;
	if (NEEDS_SIGN_IN.test(raw)) return words.signIn;
	if (FORBIDDEN.test(raw)) return words.forbidden;
	if (NOT_FOUND.test(raw) && !context) return words.notFound;
	if (context) {
		const c = context.trim().replace(/[.!]$/, '');
		return `${c}. Try again.`;
	}
	return words.generic;
}
