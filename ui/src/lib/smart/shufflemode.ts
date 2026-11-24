// Smart Shuffle's button cycle (stream smart2), pure so `smartshuffle.check.ts` can run it in node.

export type ShuffleMode = 'off' | 'shuffle' | 'smart';

/** Own playlists (this Mac's) and Liked Songs (`VLLM`). */
export const smartEligible = (sourceId: string | null | undefined) =>
	!!sourceId && (sourceId.startsWith('LOCALPLAYLIST:') || sourceId === 'VLLM');

/** off → shuffle → smart shuffle → off; outside own playlists off ↔ shuffle. */
export function nextMode(mode: ShuffleMode, eligible: boolean): ShuffleMode {
	if (mode === 'off') return 'shuffle';
	if (mode === 'shuffle') return eligible ? 'smart' : 'off';
	return 'off';
}

/** The button's tooltip: what pressing it does next (Spotify's wording). */
export function shuffleLabel(mode: ShuffleMode, eligible: boolean): string {
	if (mode === 'off') return 'Enable shuffle';
	if (mode === 'shuffle') return eligible ? 'Enable Smart Shuffle' : 'Disable shuffle';
	return 'Disable Smart Shuffle';
}
