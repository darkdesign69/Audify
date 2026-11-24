// stream: chapters. The one rule a UI keeps for chapters: which one is playing. Everything else
// (sources, cleanup, ends and labels) comes from the core (`podcasts::chapters`).
import type { PodcastChapter } from './api';

/** Index of the chapter playing at `pos` seconds (the last one started), or -1 before the first. */
export function chapterAt(list: readonly PodcastChapter[], pos: number): number {
	for (let i = list.length - 1; i >= 0; i--) if (list[i].startSecs <= pos + 0.25) return i;
	return -1;
}
