// Native menu bar → UI. Rust (`src-tauri/src/menu.rs`) emits `menu://<id>` for the items the page
// owns (core.md). One listener map, wired once by the root layout.
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ui } from '$lib/player.svelte';
import { zoomBy, setZoom } from '$lib/zoom.svelte';
import { createFolder, createPlaylist, toggleLike } from '$lib/kit/actions';
import { closeMenu } from '$lib/kit/menus';
import { nowPlayingSong } from './nowplaying';
import { focusSearch } from './search.svelte';
import { openSettings } from './settings';
import { toggleLeft, toggleRight } from './layout.svelte';
import { requestFind } from './find.svelte';
import { goSettings } from '$lib/settings/actions';
import { checkForUpdatesInteractive } from '$lib/updater.svelte';

export const MENU_ACTIONS: Record<string, () => void> = {
	// Settings › About's row shows the outcome (and Install), as a Mac app's About/Updates pane would.
	'check-updates': () => {
		goSettings('updates');
		void checkForUpdatesInteractive();
	},
	preferences: openSettings,
	'new-playlist': () => createPlaylist(),
	'new-folder': () => createFolder(),
	find: requestFind,
	search: focusSearch,
	back: () => history.back(),
	forward: () => history.forward(),
	'toggle-sidebar': toggleLeft,
	'toggle-now-playing': () => toggleRight('now-playing'),
	'toggle-queue': () => toggleRight('queue'),
	'zoom-in': () => zoomBy(1),
	'zoom-out': () => zoomBy(-1),
	'zoom-reset': () => setZoom(1),
	like: () => {
		const s = nowPlayingSong();
		if (s) toggleLike(s);
	},
	shortcuts: () => (ui.shortcutsOpen = !ui.shortcutsOpen)
};

export function initMenubar(): () => void {
	const offs: Promise<UnlistenFn>[] = Object.entries(MENU_ACTIONS).map(([id, run]) =>
		listen(`menu://${id}`, () => {
			closeMenu();
			run();
		}).catch(() => () => {})
	);
	return () => offs.forEach((p) => p.then((f) => f()));
}
