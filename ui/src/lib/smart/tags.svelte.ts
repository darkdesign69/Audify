/**
 * Lane magic-d3: Liked Songs genre and mood chips (#7). Rust tags the likes from MusicBrainz (and
 * Last.fm / Deezer as fallbacks) in the background and answers with the chips worth showing;
 * `smart-tags-changed` brings them again as more tracks get tagged.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { LikedChips } from './chips';
export { filterByChip, keepChip, type LikedChips, type TagChip } from './chips';

export const likedTags = $state<LikedChips & { loaded: boolean; playing: string | null }>({
	chips: [],
	done: 0,
	total: 0,
	loaded: false,
	/** The chip Liked Songs was last played under (null: all of it). */
	playing: null
});

function apply(r: LikedChips | null | undefined) {
	if (!r) return;
	likedTags.chips = r.chips ?? [];
	likedTags.done = r.done ?? 0;
	likedTags.total = r.total ?? 0;
	likedTags.loaded = true;
}

let subscribed = false;
/** Ask for the chips (again: after a like or unlike, the counts move). */
export async function loadLikedTags() {
	if (!subscribed) {
		subscribed = true;
		void listen<LikedChips>('smart-tags-changed', (e) => apply(e.payload)).catch(() => {});
	}
	try {
		apply(await invoke<LikedChips>('smart_tags_liked'));
	} catch {
		/* older backend: no chips */
	}
}
