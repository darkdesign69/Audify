// Self-check for the dropdown's stale-response rule (`lastwins.ts`, stream: search-dropdown).
import { lastWins, sharedByKey } from './lastwins.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const later = <T>(ms: number, v: T) => new Promise<T>((r) => setTimeout(() => r(v), ms));

// Typing "que" then "queen": the slow "que" answer arrives last and must be dropped.
const req = lastWins();
let shown = '';
async function search(q: string, ms: number) {
	const t = req.take();
	const answer = await later(ms, `results for ${q}`);
	if (t.current()) shown = answer;
}
await Promise.all([search('que', 40), search('queen', 5)]);
ok(shown === 'results for queen', `last query wins, got "${shown}"`);

// Emptying the field drops whatever was in flight.
shown = '';
const p = search('abba', 5);
req.drop();
await p;
ok(shown === '', 'dropped after the field is emptied');

// The dropdown and the results page asking for the same query share one request.
let calls = 0;
const get = sharedByKey(async (q) => {
	calls++;
	return later(5, q.toUpperCase());
});
const [a, b] = await Promise.all([get('queen'), get('queen')]);
ok(a === 'QUEEN' && b === 'QUEEN' && calls === 1, `one request for two callers, got ${calls}`);
await get('queen');
ok(calls === 2, 'a finished request is not held (the page cache keeps answers)');
console.log('ok');
