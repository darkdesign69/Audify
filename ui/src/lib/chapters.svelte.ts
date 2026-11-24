// stream: chapters. Chapters of the playing episode (Now Playing, player bar, full screen) and a
// per-episode cache for the episode page. Loaded beside playback, never in its way.
import { untrack } from 'svelte';
import * as api from './api';
import { playback, seekTo } from './player.svelte';
import { chapterAt } from './chapters';

const cache = new Map<string, Promise<api.PodcastChapter[]>>();

/** An episode's chapters (empty when it has none or the lookup failed). */
export function loadChapters(id: string, durationSecs?: number): Promise<api.PodcastChapter[]> {
	let p = cache.get(id);
	if (!p) {
		p = api
			.podcastChapters(id, durationSecs)
			.then((c) => c.chapters ?? [])
			.catch(() => {
				cache.delete(id);
				return [];
			});
		cache.set(id, p);
	}
	return p;
}

/** The playing episode's chapters. */
export const nowChapters = $state<{ id: string | null; list: api.PodcastChapter[] }>({ id: null, list: [] });

let started = false;
/** Follow the playing episode (call once, from the always-mounted player bar). */
export function trackNowChapters() {
	if (started) return;
	started = true;
	$effect.root(() => {
		$effect(() => {
			const now = playback.now;
			const id = now?.isEpisode ? now.videoId : null;
			if (id === untrack(() => nowChapters.id)) return;
			nowChapters.id = id;
			nowChapters.list = [];
			if (!id) return;
			loadChapters(id, untrack(() => playback.duration)).then((list) => {
				if (nowChapters.id === id) nowChapters.list = list;
			});
		});
	});
}

/** Index of the playing chapter of the playing episode, -1 when none. */
export function nowChapterIndex(): number {
	return chapterAt(nowChapters.list, playback.position);
}

/** Jump to a chapter of the playing episode. */
export function seekChapter(c: api.PodcastChapter) {
	seekTo(c.startSecs);
}
