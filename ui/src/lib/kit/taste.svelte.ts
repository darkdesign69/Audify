// ── lane magic-d1 ── "Exclude from your taste profile" (MAGIC-GAPS #16). The excluded ids, for
// the menus' label, plus the toggle with its Undo toast. Rust: crates/core/src/smart/taste.rs.
import type { SongItem } from '$lib/api';
import * as tapi from '$lib/trackapi';
import { toast, toastUndo } from './toast';

/** Ids excluded from the taste profile. Reactive; loaded once, kept in step by the toggles. */
export const taste = $state({ excluded: [] as string[], loaded: false });

let loading: Promise<void> | null = null;
export function loadTasteExcluded(): Promise<void> {
	loading ??= tapi
		.tasteExcluded()
		.then((ids) => {
			taste.excluded = ids;
			taste.loaded = true;
		})
		.catch(() => {
			/* older backend: every item reads as included */
		});
	return loading;
}

/** Excluded? The first ask loads the list; menus re-read it when it lands. */
export function isTasteExcluded(id: string): boolean {
	if (!taste.loaded) void loadTasteExcluded();
	return taste.excluded.includes(id);
}

/** Exclude a song or playlist from the taste profile, or include it again, with Undo. */
export async function toggleTasteExcluded(
	item: { kind: 'song' | 'playlist'; id: string; title?: string },
	tracks: () => Promise<SongItem[]> | SongItem[] = () => []
) {
	const on = !isTasteExcluded(item.id);
	const set = async (excluded: boolean) => {
		const ids = excluded ? (await tracks()).map((s) => s.video_id) : [];
		await tapi.tasteSetExcluded(item.id, item.kind, item.title, ids, excluded);
		taste.excluded = excluded
			? [item.id, ...taste.excluded.filter((x) => x !== item.id)]
			: taste.excluded.filter((x) => x !== item.id);
	};
	try {
		await set(on);
		toastUndo(on ? 'Excluded from your taste profile' : 'Included in your taste profile', () => set(!on));
	} catch (e) {
		toast.fail(e, "Couldn't update your taste profile");
	}
}
