// Self-check for Smart Shuffle's button cycle and What's New grouping (stream smart2):
//     node --experimental-strip-types ui/src/lib/smartshuffle.check.ts
import { nextMode, shuffleLabel, smartEligible } from './smart/shufflemode.ts';
import { feed, releaseWhen } from './smart/whatsnew.ts';
import type { WhatsNew } from './api.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

// off → shuffle → smart → off in own playlists; off ↔ shuffle elsewhere.
ok(nextMode('off', true) === 'shuffle', 'off goes to shuffle');
ok(nextMode('shuffle', true) === 'smart', 'shuffle goes to smart in own playlists');
ok(nextMode('smart', true) === 'off', 'smart goes to off');
ok(nextMode('shuffle', false) === 'off', 'no smart outside own playlists');
ok(smartEligible('LOCALPLAYLIST:4') && smartEligible('VLLM'), 'own playlists and Liked Songs');
ok(!smartEligible('PLabc') && !smartEligible(null), 'not YouTube playlists');
ok(shuffleLabel('shuffle', true) === 'Enable Smart Shuffle', 'tooltip names the next state');
ok(shuffleLabel('smart', true) === 'Disable Smart Shuffle', 'smart tooltip');

const now = 1_800_000_000;
const rel = (id: string, firstSeen: number, isNew: boolean) => ({
	id, title: id, artist: 'A', artistId: 'UCa', year: 2026, firstSeen, followed: true, ytNew: false,
	position: 0, releaseType: 'Single', isNew
});
const w = {
	music: [rel('old', 0, false), rel('fresh', now - 100, true), rel('week', now - 8 * 86400, false)],
	podcasts: [{ id: 'ep', publishedAt: now - 50, isNew: true }],
	seenAt: now - 1000,
	newCount: 2
} as unknown as WhatsNew;
const g = feed(w, null);
ok(g.fresh.map((i) => i.key).join() === 'e:ep,r:fresh', `new group newest first: ${g.fresh.map((i) => i.key)}`);
ok(g.earlier.map((i) => i.key).join() === 'r:week,r:old', 'earlier keeps dated before undated');
ok(feed(w, 'music').fresh.length === 1 && feed(w, 'podcasts').earlier.length === 0, 'chips filter');
ok(releaseWhen({ firstSeen: now - 2 * 86400, year: 2026 }, now) === '2 days ago', 'days ago');
ok(releaseWhen({ firstSeen: 0, year: 2025 }, now) === '2025', 'undated shows the year');
ok(releaseWhen({ firstSeen: now - 10, year: 2026 }, now) === 'Today', 'today');
console.log('ok');
