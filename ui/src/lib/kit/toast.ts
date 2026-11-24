// Spotify-style toasts on svelte-sonner. One visible at a time (the newest replaces the current),
// bottom-centre above the player bar, white pill with black text, optional action ("Undo", "View").
// Mount <Toaster/> from `$lib/kit` once (the root layout does). Every legacy `toast()` from
// `$lib/player.svelte` is routed here too, so the whole app shares one look.
//
// ── lane magic-d1 ── Errors are worded for people (MAGIC-GAPS #21): every error toast goes through
// `friendlyError`, so raw exception text never reaches the screen, and the raw text is logged
// (limusic.log via errlog) for the bug report. `toast.fail(e, "Couldn't start radio")` is the call
// for a caught error: it words the failure with what the user was doing.
import { toast as sonner } from 'svelte-sonner';
import { logUi } from '$lib/errlog';
import { DEFAULT_ERROR_WORDS, friendlyError, rawError, type ErrorWords } from '$lib/neterr';

export type ToastAction = { label: string; onClick: () => void };
export type ToastOpts = {
	action?: ToastAction;
	/** A second, quieter action left of `action` ("Undo" beside "Change"). */
	secondary?: ToastAction;
	/** ms; default 3000, 6000 when there is an action. */
	duration?: number;
	/** `fail` only: when the error fits no friendly wording, show it after `context` ("Couldn't play
	 *  this: <the error>") instead of "… Try again.". */
	detail?: boolean;
};

// The sentences, translatable: the app swaps in its locale's words at startup.
let words: () => ErrorWords = () => DEFAULT_ERROR_WORDS;
export function setErrorWords(get: () => Partial<ErrorWords>) {
	words = () => ({ ...DEFAULT_ERROR_WORDS, ...get() });
}

function logRaw(raw: string, shown: string) {
	if (raw === shown) return;
	console.warn('[toast] error:', raw);
	logUi('warn', `toast error: ${raw} (shown as: ${shown})`);
}

const BUTTON = 'shrink-0 text-base font-bold text-black hover:scale-[1.04] transition-transform duration-50';

function show(kind: 'info' | 'success' | 'error', msg: string, opts: ToastOpts = {}) {
	sonner.dismiss();
	if (kind === 'error') {
		const shown = friendlyError(msg, undefined, words());
		logRaw(msg, shown);
		msg = shown;
	}
	const duration =
		opts.duration ?? (opts.action || opts.secondary ? 6000 : kind === 'error' ? 5000 : 3000);
	return sonner(msg, {
		duration,
		class: kind === 'error' ? 'encore-toast-error' : undefined,
		action: opts.action
			? { label: opts.action.label, onClick: () => opts.action!.onClick() }
			: undefined,
		cancel: opts.secondary
			? { label: opts.secondary.label, onClick: () => opts.secondary!.onClick() }
			: undefined,
		// With two buttons the first one takes the free space (`ml-auto`), the second sits beside it.
		classes: opts.secondary
			? { cancelButton: `ml-auto ${BUTTON}`, actionButton: '!ml-0' }
			: undefined
	});
}

/** A caught error as a toast: friendly words for the user (`context` says what failed), the raw
 *  text to the log. */
function fail(e: unknown, context?: string, opts?: ToastOpts) {
	const raw = rawError(e);
	let shown = friendlyError(raw, context, words());
	const what = context?.trim().replace(/[.!]$/, '');
	// The fallback sentence says nothing about why; the error itself does.
	if (opts?.detail && what && shown === `${what}. Try again.`)
		shown = `${what}: ${raw.replace(/^Error: /, '').slice(0, 200)}`;
	logRaw(raw, shown);
	sonner.dismiss();
	return sonner(shown, {
		duration: opts?.duration ?? 5000,
		class: 'encore-toast-error',
		action: opts?.action ? { label: opts.action.label, onClick: () => opts.action!.onClick() } : undefined
	});
}

/** Neutral toast: `toast('Added to queue')`. `.success` / `.error` / `.info` for intent. */
export const toast = Object.assign((msg: string, opts?: ToastOpts) => show('info', msg, opts), {
	info: (msg: string, opts?: ToastOpts) => show('info', msg, opts),
	success: (msg: string, opts?: ToastOpts) => show('success', msg, opts),
	error: (msg: string, opts?: ToastOpts) => show('error', msg, opts),
	fail,
	dismiss: () => sonner.dismiss()
});

/** A toast with an "Undo" action. `undo` may be async; a failure shows an error toast. */
export function toastUndo(msg: string, undo: () => unknown, label = 'Undo', opts: ToastOpts = {}) {
	return show('info', msg, {
		...opts,
		action: {
			label,
			onClick: async () => {
				try {
					await undo();
				} catch (e) {
					fail(e, "Couldn't undo that");
				}
			}
		}
	});
}
