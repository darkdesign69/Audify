// Lane magic-d2 self-check: where a drop on the queue panel lands (`dnd.ts` queueDropRange /
// queueDropAt). Mirrors state.rs `splice_queued_at`'s clamp: only inside "Next in queue".
//
//     node --experimental-strip-types ui/src/lib/dndqueue.check.ts
import { queueDropAt, queueDropRange } from './dnd.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

const row = (video_id: string, extra: Record<string, boolean> = {}) =>
	({ video_id, title: video_id, artists: '', ...extra }) as never;

// now, two Play next, one Add to queue, then the playlist and autoplay.
const items = [
	row('now'),
	row('n1', { queued: true }),
	row('n2', { queued: true }),
	row('x1', { queued_end: true }),
	row('p1'),
	row('r1', { autoplay: true })
];
const range = queueDropRange(items, 0);
ok(range[0] === 1 && range[1] === 4, `range is the manual block, got ${range}`);
ok(queueDropAt(range, 2, true) === 2, 'upper half of n2: in front of it');
ok(queueDropAt(range, 2, false) === 3, 'lower half of n2: behind it');
ok(queueDropAt(range, 0, true) === 1, 'over the playing row: right after it');
ok(queueDropAt(range, 4, true) === 4, 'over the playlist: the end of Next in queue');
ok(queueDropAt(range, 5, false) === 4, 'over autoplay: the end of Next in queue');

// Nothing queued by hand: the only spot is right after the playing track.
const bare = queueDropRange([row('a'), row('b'), row('c')], 1);
ok(bare[0] === 2 && bare[1] === 2, `empty manual block, got ${bare}`);
ok(queueDropAt(bare, 0, true) === 2, 'history clamps forward');

// An empty queue: index 0, which the core turns into "start playing it".
const none = queueDropRange([], 0);
ok(none[0] === 0 && none[1] === 0, 'empty queue');

console.log('ok');
