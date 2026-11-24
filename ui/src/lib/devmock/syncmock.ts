// DEV ONLY (device sync UI): a stateful stand-in for the core's sync commands, so Settings ›
// Devices and the linking dialog run in the browser preview. Presets by URL:
//   ?sync=linked | pending | error     start linked (synced 2 min ago / 3 waiting / relay down)
//   ?pair=intro|show|enter|success     open the dialog at that step (devices.svelte.ts)
//   ?joincode=000000 …                  type a code in "Enter code": 000000 wrong (2 tries left),
//                                       111111 expired, 222222 can't see each other,
//                                       333333 Local Network denied, anything else links
//   ?devpeer                            the code shown is entered on the "iPhone" after 3 s
/* eslint-disable @typescript-eslint/no-explicit-any */
import type { SyncStatus } from '$lib/api';

const PEER = { id: 'b7e1', name: "Alex's iPhone", platform: 'ios', lastSeen: 0, lan: true };
const now = () => Math.floor(Date.now() / 1000);

export function syncHandlers(emit: (event: string, payload: unknown) => void) {
	const q = new URLSearchParams(location.search);
	const preset = q.get('sync');
	const st: SyncStatus = {
		paired: !!preset,
		deviceId: 'a1c4',
		deviceName: "Alex's MacBook Pro",
		platform: 'mac',
		pairId: preset ? 'p1' : null,
		peers: preset ? [{ ...PEER, lastSeen: now() - 120 }] : [],
		pending: preset === 'pending' ? 3 : preset === 'error' ? 12 : 0,
		lastSync: preset ? now() - 120 : null,
		lastError: preset === 'error' ? 'relay: HTTP 503' : null,
		relay: { enabled: true, ok: preset === 'error' ? false : true },
		lanPort: preset ? 47615 : null,
		pairing: null,
		syncing: false,
		active: true
	};
	const changed = () => emit('sync-changed', { status: { ...st }, entities: [] });
	const status = () => ({ ...st });
	return {
		sync_status: status,
		sync_set_active: (a: any) => ((st.active = !!a.active), status()),
		sync_start_pairing: () => {
			const code = '482913';
			const qr = `spotless://pair?v=1&code=${code}&addr=192.168.1.20%3A47615&dev=${st.deviceId}&name=Alex%27s%20MacBook%20Pro`;
			st.pairing = { code, qr, expiresAt: now() + 300 };
			st.lanPort = 47615;
			changed();
			if (q.has('devpeer'))
				setTimeout(() => {
					if (!st.pairing) return;
					st.pairing = null;
					st.paired = true;
					st.peers = [{ ...PEER, lastSeen: now() }];
					st.lastSync = now();
					changed();
				}, 3000);
			return { ...st.pairing, port: 47615, addrs: ['192.168.1.20'] };
		},
		sync_cancel_pairing: () => ((st.pairing = null), changed(), status()),
		sync_join: async (a: any) => {
			await new Promise((r) => setTimeout(r, 700));
			const code = a.code ?? /code=(\d+)/.exec(a.qr ?? '')?.[1];
			if (code === '000000') throw 'wrong_code:2';
			if (code === '111111') throw 'not_pairing';
			if (code === '222222') throw 'no_host';
			if (code === '333333') throw 'No route to host (os error 65)';
			Object.assign(st, { paired: true, pairId: 'p1', peers: [{ ...PEER, lastSeen: now() }], lastSync: now(), lastError: null });
			changed();
			return status();
		},
		sync_unpair: () => {
			Object.assign(st, { paired: false, pairId: null, peers: [], pending: 0, lastSync: null, lastError: null });
			changed();
			return status();
		},
		sync_now: async () => {
			st.syncing = true;
			changed();
			await new Promise((r) => setTimeout(r, 600));
			Object.assign(st, { syncing: false, lastSync: now(), pending: 0, lastError: null });
			changed();
			return status();
		}
	};
}
