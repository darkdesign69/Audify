import type { Snippet } from 'svelte';
import type { BrowseItem, SongItem } from '$lib/api';
import type { IconComponent } from '../Icon.svelte';

/** One entry of a data-driven menu (context menus, "…" menus, sort menus). */
export type MenuEntry =
	| {
			type?: 'item';
			label: string;
			icon?: IconComponent;
			/** Spotify's checked item: accent text + accent check at the right. */
			checked?: boolean;
			disabled?: boolean;
			/** Keyboard hint shown at the right, e.g. "⌥⇧B". */
			hint?: string;
			onSelect: () => void;
	  }
	| {
			type: 'sub';
			label: string;
			icon?: IconComponent;
			items: MenuEntry[] | (() => MenuEntry[]);
			/** Submenu width in px (Spotify: share 200-ish, add-to-playlist 350). */
			width?: number;
	  }
	/** "Add to playlist ▸": the submenu with a search box, "New playlist" and the user's playlists. */
	| { type: 'playlist-picker'; label?: string; songs: SongItem[] }
	/** The accent liked-check's "Saved in" picker (Liked Songs + playlists, Cancel / Done). */
	| { type: 'saved-in'; song: SongItem }
	| { type: 'separator' }
	| { type: 'heading'; label: string }
	/** Arbitrary content (e.g. the sidebar sort menu's "View as" icon row). Receives `close`. */
	| { type: 'custom'; snippet: Snippet<[() => void]> };

/** Where a track menu was opened, which decides context items ("Remove from this playlist" …). */
export type TrackMenuContext = {
	/** The playlist the rows belong to (enables "Remove from this playlist" when own). */
	playlistId?: string;
	/** Override ownership detection for playlistId. */
	own?: boolean;
	/** Row is a queue row at this backend index (enables "Remove from queue"). */
	queueIndex?: number;
	/** Called after a removal so the host list can drop the rows (optimistic). */
	onRemove?: (songs: SongItem[]) => void;
	/** Album page: hide "Go to album". */
	albumId?: string;
	/** Artist page: hide "Go to artist" for this artist. */
	artistId?: string;
};

export type ItemMenuContext = {
	/** Opened from the library sidebar (adds Pin/Unpin, Create playlist/folder, Move to folder). */
	sidebar?: boolean;
	/** For a folder-aware sidebar: folder actions. */
	onRename?: () => void;
};

export type MenuTarget = MouseEvent | HTMLElement | { x: number; y: number };
export type { BrowseItem, SongItem };
