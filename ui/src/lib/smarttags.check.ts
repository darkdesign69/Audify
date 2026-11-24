// Self-check for Liked Songs chips and Enhance (lane magic-d3):
//     node --experimental-strip-types ui/src/lib/smarttags.check.ts
import { enhanceEligible, enhanceSlots, filterByChip, interleave, keepChip, type TagChip } from './smart/chips.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const eq = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

const rows = ['a', 'b', 'c', 'd', 'e'].map((video_id) => ({ video_id }));
const jazz: TagChip = { label: 'Jazz', kind: 'genre', videoIds: ['d', 'b', 'zz'] };

// A chip keeps the list's own order and only its tracks; no chip keeps everything.
ok(eq(filterByChip(rows, jazz).map((r) => r.video_id), ['b', 'd']), 'chip filters in list order');
ok(filterByChip(rows, null).length === 5, 'no chip, no filter');
ok(keepChip('Jazz', [jazz]) === 'Jazz', 'a chip still offered stays picked');
ok(keepChip('Rock', [jazz]) === null, 'a chip that went away is dropped');

// Enhance: one suggestion after every 2 songs, never after the last (mirrors smartshuffle.rs).
ok(eq(interleave([1, 2, 3, 4, 5, 6, 7], [100, 200, 300, 400]), [1, 2, 100, 3, 4, 200, 5, 6, 300, 7]), 'every 2');
ok(eq(interleave([1, 2, 3, 4, 5], [100]), [1, 2, 100, 3, 4, 5]), 'runs out of suggestions');
ok(eq(interleave([1, 2], [9]), [1, 2]), 'none after the last song');
ok(enhanceSlots(7, 10) === 3 && enhanceSlots(0, 5) === 0 && enhanceSlots(9, 2) === 2, 'slots');
ok(enhanceEligible('LOCALPLAYLIST:2') && enhanceEligible('VLLM') && !enhanceEligible('PLx'), 'eligible');

console.log('ok');
