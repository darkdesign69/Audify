// Self-check for Spotify's English date formats (parity4). Same shape as `timecode.check.ts`:
//
//     node --experimental-strip-types ui/src/lib/dates.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant. Not imported by the app.
import { formatAdded } from './kit/trackTable.ts';
import { episodeDate } from './discover/format.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

const now = new Date(2026, 10, 15, 12, 0, 0).getTime(); // Nov 15, 2026
const at = (y: number, m: number, d: number) => new Date(y, m, d, 12, 0, 0).getTime() / 1000;

// Date added: relative under 30 days, then "Aug 14, 2026" (Spotify English, measured).
ok(formatAdded(at(2026, 10, 14), now) === '1 day ago', 'yesterday is relative');
ok(formatAdded(at(2026, 9, 20), now) === '3 weeks ago', 'weeks under a month');
ok(formatAdded(at(2026, 7, 14), now) === 'Aug 14, 2026', `month-first absolute date, got ${formatAdded(at(2026, 7, 14), now)}`);
ok(formatAdded(at(2023, 8, 29), now) === 'Sep 29, 2023', 'older year');
ok(formatAdded(undefined, now) === '', 'no date');

// Episode dates: "Nov 13" this year, "Nov 13, 2024" before.
const y = new Date().getFullYear();
ok(episodeDate(at(y, 10, 13)) === 'Nov 13', `this year, got ${episodeDate(at(y, 10, 13))}`);
ok(episodeDate(at(2024, 10, 13)) === 'Nov 13, 2024', 'earlier year');

console.log('ok');
