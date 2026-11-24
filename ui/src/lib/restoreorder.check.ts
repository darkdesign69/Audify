// Self-check for restoreorder.ts (lane magic-d1, MAGIC-GAPS #17). `pnpm test` runs it.
import { matchFresh, restoreMoves } from './restoreorder.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

// Playlist a b c d e; b and c removed, then re-added as B2 C2 at the end.
const order = ['a', 'b', 'c', 'd', 'e'];
const removed = [
	{ setVideoId: 'b', videoId: 'vb' },
	{ setVideoId: 'c', videoId: 'vc' }
];
const fresh = matchFresh(removed, new Set(['a', 'd', 'e']), [
	{ set_video_id: 'a', video_id: 'va' },
	{ set_video_id: 'd', video_id: 'vd' },
	{ set_video_id: 'e', video_id: 've' },
	{ set_video_id: 'B2', video_id: 'vb' },
	{ set_video_id: 'C2', video_id: 'vc' }
]);
ok(fresh.get('b') === 'B2' && fresh.get('c') === 'C2', 'new copies are matched by song');
const moves = restoreMoves(order, removed, fresh);
// c goes before d first, then b before c's new copy.
ok(eq(moves, [{ setVideoId: 'C2', before: 'd' }, { setVideoId: 'B2', before: 'C2' }]), `two moves: ${JSON.stringify(moves)}`);

// Simulate the moves on the list and check the order comes back.
let list = ['a', 'd', 'e', 'B2', 'C2'];
for (const m of moves) {
	list = list.filter((x) => x !== m.setVideoId);
	const i = m.before ? list.indexOf(m.before) : list.length;
	list.splice(i, 0, m.setVideoId);
}
ok(eq(list, ['a', 'B2', 'C2', 'd', 'e']), `order restored: ${list}`);

// The last row needs no move; a row past the first page is left at the end.
ok(eq(restoreMoves(order, [{ setVideoId: 'e', videoId: 've' }], new Map([['e', 'E2']])), []), 'last row stays put');
ok(eq(restoreMoves(order, [{ setVideoId: 'zz', videoId: 'vz' }], new Map([['zz', 'Z2']])), []), 'unknown row stays at the end');

// The same song twice: each removed row gets its own new copy.
const dup = matchFresh(
	[
		{ setVideoId: 'x1', videoId: 'v' },
		{ setVideoId: 'x2', videoId: 'v' }
	],
	new Set(),
	[
		{ set_video_id: 'n1', video_id: 'v' },
		{ set_video_id: 'n2', video_id: 'v' }
	]
);
ok(dup.get('x1') === 'n1' && dup.get('x2') === 'n2', 'duplicates matched one to one');

console.log('ok');
