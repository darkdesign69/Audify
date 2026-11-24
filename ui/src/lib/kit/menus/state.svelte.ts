// One menu open at a time, app-wide (Spotify behaviour). `openMenu` is called by row/card
// right-clicks and "…" buttons; <MenuHost/> (mounted once in the root layout) renders it.
import type { MenuEntry, MenuTarget } from './types';

type Measurable = { getBoundingClientRect: () => DOMRect };

export const menu = $state({
	open: false,
	entries: [] as MenuEntry[],
	anchor: null as HTMLElement | Measurable | null,
	/** Pointer menus open down-right from the point; button menus align to the button's end. */
	align: 'start' as 'start' | 'end' | 'center',
	side: 'bottom' as 'bottom' | 'top' | 'right' | 'left',
	width: undefined as number | undefined,
	/** Bumped per open so the host remounts (fresh focus/submenu state). */
	seq: 0,
	/** The element that opened it, which gets `data-menu-open` while open (row stays highlighted). */
	source: null as HTMLElement | null,
	/** a11y: where focus goes back to when the menu closes (the opener, or what had focus). */
	returnTo: null as HTMLElement | null
});

function point(x: number, y: number): Measurable {
	return { getBoundingClientRect: () => new DOMRect(x, y, 0, 0) };
}

/**
 * Open a menu. `target`: a MouseEvent (right-click → at the pointer; a click → under the clicked
 * element), an element (anchored to it, end-aligned like Spotify's "…" menus) or a point.
 */
export function openMenu(
	target: MenuTarget,
	entries: MenuEntry[],
	opts: { align?: 'start' | 'end' | 'center'; side?: 'bottom' | 'top' | 'right' | 'left'; width?: number } = {}
) {
	let anchor: HTMLElement | Measurable;
	let align = opts.align;
	let side = opts.side;
	let source: HTMLElement | null = null;
	if (target instanceof MouseEvent) {
		target.preventDefault();
		target.stopPropagation();
		const el = target.currentTarget instanceof HTMLElement ? target.currentTarget : null;
		if (target.type === 'contextmenu' || !el) {
			anchor = point(target.clientX, target.clientY);
			align ??= 'start';
			side ??= 'right';
		} else {
			anchor = el;
			align ??= 'end';
		}
		source = el;
	} else if (target instanceof HTMLElement) {
		anchor = target;
		align ??= 'end';
		source = target;
	} else {
		anchor = point(target.x, target.y);
		align ??= 'start';
		side ??= 'right';
	}
	menu.source?.removeAttribute('data-menu-open');
	source?.setAttribute('data-menu-open', '');
	menu.entries = entries;
	menu.anchor = anchor;
	menu.align = align;
	// A pointer menu opens to the right of the point, top-aligned: then floating-ui *shifts* it up
	// just enough when there is no room below (Spotify) instead of flipping it above the pointer,
	// and flips it to the left of the pointer when there is no room on the right.
	menu.side = side ?? 'bottom';
	menu.width = opts.width;
	menu.source = source;
	const active = typeof document !== 'undefined' ? document.activeElement : null;
	menu.returnTo =
		active instanceof HTMLElement && active !== document.body && !active.closest('[role=menu]')
			? active
			: source;
	menu.seq++;
	menu.open = true;
}

export function closeMenu() {
	menu.open = false;
	menu.source?.removeAttribute('data-menu-open');
	menu.source = null;
}

/** a11y: put focus back on the opener after the menu closes (Spotify does; bits-ui's own return is
 *  off because our menus are opened programmatically, not by a bits-ui trigger). Skipped when focus
 *  has already moved somewhere real (an item opened a dialog or focused a field). */
export function restoreMenuFocus() {
	const el = menu.returnTo;
	menu.returnTo = null;
	if (!el || !el.isConnected) return;
	const a = document.activeElement;
	if (a && a !== document.body && !a.closest('[data-menu-content],[role=menu]')) return;
	el.focus({ preventScroll: true });
}
