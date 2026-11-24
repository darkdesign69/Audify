// What's New helpers (stream smart2): pure, so `whatsnew.check.ts` can test them.
import type { WhatsNew, WhatsNewEpisode, WhatsNewRelease } from '$lib/api';

export type FeedFilter = 'music' | 'podcasts';

export type FeedItem =
	| { kind: 'release'; key: string; at: number; isNew: boolean; release: WhatsNewRelease }
	| { kind: 'episode'; key: string; at: number; isNew: boolean; episode: WhatsNewEpisode };

/** One list, newest first, split into Spotify's "New" and "Earlier" groups. */
export function feed(w: WhatsNew | null, filter: FeedFilter | null): { fresh: FeedItem[]; earlier: FeedItem[] } {
	if (!w) return { fresh: [], earlier: [] };
	const items: FeedItem[] = [];
	if (filter !== 'podcasts')
		for (const r of w.music)
			items.push({ kind: 'release', key: `r:${r.id}`, at: r.firstSeen, isNew: r.isNew, release: r });
	if (filter !== 'music')
		for (const e of w.podcasts)
			items.push({ kind: 'episode', key: `e:${e.id}`, at: e.publishedAt ?? 0, isNew: e.isNew, episode: e });
	// Stable: within equal times, the backend's order (music newest-first) stays.
	const ordered = items.map((it, i) => [it, i] as const).sort((a, b) => b[0].at - a[0].at || a[1] - b[1]).map(([it]) => it);
	return { fresh: ordered.filter((i) => i.isNew), earlier: ordered.filter((i) => !i.isNew) };
}

/** "Today", "2 days ago", "3 weeks ago", else the year (a release first filed at the first scan). */
export function releaseWhen(r: Pick<WhatsNewRelease, 'firstSeen' | 'year'>, now = Date.now() / 1000): string {
	if (!r.firstSeen) return String(r.year);
	const days = Math.floor((now - r.firstSeen) / 86400);
	if (days <= 0) return 'Today';
	if (days === 1) return 'Yesterday';
	if (days < 7) return `${days} days ago`;
	if (days < 30) return `${Math.floor(days / 7)} week${days < 14 ? '' : 's'} ago`;
	return String(r.year);
}
