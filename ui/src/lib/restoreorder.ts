// ── lane magic-d1 ── Undo for "Remove from this playlist" on a YouTube playlist (MAGIC-GAPS #17).
// YouTube has no "put it back": the undo re-adds each song (it lands at the end with a new row id)
// and then moves it in front of the row that followed it. Pure, so it runs under node
// (restoreorder.check.ts).

/** One removed row: its old row id (`setVideoId`) and the song. */
export type RemovedRow = { setVideoId: string; videoId: string };

/**
 * The moves that put removed rows back where they were. `order` is the playlist's row ids before
 * the removal; `fresh` maps each removed row's old id to the id its re-added copy got. Rows are
 * placed last to first, so a row whose old successor was also removed goes in front of that
 * successor's new copy, which is already in place. A row missing from `order` (past the first
 * page) or without a new copy is left at the end. `before: null` means "at the end".
 */
export function restoreMoves(
	order: string[],
	removed: RemovedRow[],
	fresh: Map<string, string>
): { setVideoId: string; before: string | null }[] {
	const gone = new Set(removed.map((r) => r.setVideoId));
	const at = new Map(order.map((id, i) => [id, i]));
	const placed = removed
		.filter((r) => at.has(r.setVideoId) && fresh.has(r.setVideoId))
		.sort((a, b) => at.get(b.setVideoId)! - at.get(a.setVideoId)!);
	const moves: { setVideoId: string; before: string | null }[] = [];
	for (const r of placed) {
		const next = order[at.get(r.setVideoId)! + 1];
		let before: string | null = null;
		if (next !== undefined) before = gone.has(next) ? (fresh.get(next) ?? null) : next;
		// Already last and nothing follows: the re-add put it at the end, where it belongs.
		if (before === null && next === undefined) continue;
		moves.push({ setVideoId: fresh.get(r.setVideoId)!, before });
	}
	return moves;
}

/** Match re-added rows to the removed ones: the new rows (not in `before`) by song, in order. */
export function matchFresh(
	removed: RemovedRow[],
	beforeIds: Set<string>,
	now: { set_video_id?: string | null; video_id: string }[]
): Map<string, string> {
	const pool = now.filter((s) => s.set_video_id && !beforeIds.has(s.set_video_id));
	const out = new Map<string, string>();
	for (const r of removed) {
		const i = pool.findIndex((s) => s.video_id === r.videoId);
		if (i < 0) continue;
		out.set(r.setVideoId, pool[i].set_video_id!);
		pool.splice(i, 1);
	}
	return out;
}
