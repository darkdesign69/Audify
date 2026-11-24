// Device sync on the Mac: the live status (refreshed by `sync-changed`), the window's focus
// telling the core to speed up or slow down, the linking dialog's open state, and the two
// one-time nudges (the first-run screen and Home's hint). Words and rules: devices.ts.
import { browser } from '$app/environment';
import * as api from './api';
import type { SyncStatus } from './api';
import { hintDue } from './devices';

export type PairStep = 'intro' | 'show' | 'enter' | 'success';

export const devices = $state({
	status: null as SyncStatus | null,
	/** The linking dialog. `qr`: a `spotless://pair` link to join with at once (deep link). */
	pair: { open: false, step: 'intro' as PairStep, qr: null as string | null, nonce: 0 },
	/** The first-run "Using Spotless on your iPhone too?" screen. */
	welcome: false,
	welcomeQueued: false,
	/** Home's one-time hint. */
	hint: false
});

export const linked = () => !!devices.status?.paired;

// ── per-device memory (localStorage; not synced) ──
type Memo = { firstUse: number | null; hintShown: boolean; welcomeShown: boolean };
const KEY = 'spotless:devices';
function memo(): Memo {
	const d: Memo = { firstUse: null, hintShown: false, welcomeShown: false };
	if (!browser) return d;
	try {
		return { ...d, ...JSON.parse(localStorage.getItem(KEY) ?? '{}') };
	} catch {
		return d;
	}
}
function remember(patch: Partial<Memo>) {
	if (!browser) return;
	try {
		localStorage.setItem(KEY, JSON.stringify({ ...memo(), ...patch }));
	} catch {
		/* private mode: it holds for this session */
	}
}
const nowSecs = () => Math.floor(Date.now() / 1000);

export async function refreshDevices() {
	try {
		devices.status = await api.syncStatus();
	} catch {
		/* the engine starts with the services; sync-changed brings it */
	}
	return devices.status;
}

/** Open the linking dialog: at a step, or joining with a QR link (`spotless://pair?…`). */
export function openPairing(opts: { step?: PairStep; qr?: string } = {}) {
	devices.pair = {
		open: true,
		step: opts.qr ? 'enter' : (opts.step ?? 'intro'),
		qr: opts.qr ?? null,
		nonce: devices.pair.nonce + 1
	};
	devices.hint = false;
}

/** The import step finished (or was skipped): the linking screen comes next, once. */
export function offerWelcome() {
	if (memo().welcomeShown || linked()) return;
	devices.welcomeQueued = true;
}
export function welcomeShown() {
	devices.welcome = false;
	devices.welcomeQueued = false;
	remember({ welcomeShown: true });
}

export function dismissHint() {
	devices.hint = false;
}

/** Mount once (root layout). Returns the cleanup. */
export function initDevices(): () => void {
	const m = memo();
	if (m.firstUse === null) remember({ firstUse: nowSecs() });
	const offs: (() => void)[] = [];
	const un = api.onSyncChanged((p) => (devices.status = p.status));
	offs.push(() => un.then((f) => f()).catch(() => {}));
	refreshDevices().then((s) => {
		const mm = memo();
		if (s && hintDue({ paired: s.paired, firstUse: mm.firstUse, shown: mm.hintShown, now: nowSecs() })) {
			devices.hint = true;
			remember({ hintShown: true }); // once: it stays up for this session until dismissed
		}
	});
	// Window focus: polls run fast in front, slow behind (docs/SYNC.md §5.3).
	import('@tauri-apps/api/window')
		.then(({ getCurrentWindow }) =>
			getCurrentWindow().onFocusChanged(({ payload }) => {
				api.syncSetActive(payload).catch(() => {});
			})
		)
		.then((f) => offs.push(f))
		.catch(() => {});
	if (import.meta.env.DEV) devPreview();
	return () => offs.forEach((f) => f());
}

/** `?pair=intro|show|enter|success`, `?devhint` and `?devlink` (the first-run screen) open those states in the browser preview. */
function devPreview() {
	const q = new URLSearchParams(location.search);
	const step = q.get('pair');
	if (step) setTimeout(() => openPairing({ step: step as PairStep, qr: q.get('qr') ?? undefined }), 300);
	if (q.has('devhint')) devices.hint = true;
	if (q.has('devlink')) devices.welcome = true;
}
