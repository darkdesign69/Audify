// ── lane magic-d1 ── The Credits dialog (MAGIC-GAPS #6), opened from "Show credits" in a track
// menu. Mounted into <body> on first use, so no layout has to host it; later opens reuse it.
import { mount, tick } from 'svelte';
import type { SongItem } from '$lib/api';
import CreditsDialog from './CreditsDialog.svelte';

export const credits = $state<{ song: SongItem | null; open: boolean }>({ song: null, open: false });

let mounted = false;

export async function openCredits(song: SongItem) {
	if (!mounted) {
		const host = document.createElement('div');
		host.dataset.creditsHost = '';
		document.body.appendChild(host);
		mount(CreditsDialog, { target: host });
		mounted = true;
		// The dialog only opens on a change it sees: let the fresh mount settle closed first.
		await tick();
	}
	credits.song = song;
	credits.open = true;
}
