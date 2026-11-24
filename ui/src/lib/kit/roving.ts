// Arrow-key navigation for composite widgets (a11y stream). One attachment for every list-like
// thing Spotify drives with the arrows: Your Library (treegrid: ↑/↓, grid view ←/→ too), the
// library filter chips (listbox: ←/→), card shelves/carousels (←/→ between cards).
//
//   <div {@attach roving({ items: '[data-lib-row]', orientation: 'vertical' })}>
//
// - `items`: selector for the focusable items (matched inside the container).
// - `orientation`: 'vertical' (↑/↓), 'horizontal' (←/→), 'grid' (←/→ by one, ↑/↓ by a row; the
//   column count is read from the layout: items stepping rightwards before the first wrap).
// - `tabStop` (default true): a roving tabindex — only the current item is in the Tab order
//   (tabindex 0), the rest are -1, so the widget is one Tab stop like Spotify's. With false the
//   items keep their own tab order and only the arrows are added (cards: one stop per card).
// - Home/End jump to the first/last item. Keys with ⌥/⌘/Ctrl are left alone (app shortcuts).
// - Only handles keys whose target is an item itself, so buttons inside an item keep their keys.
import type { Attachment } from 'svelte/attachments';

export type RovingOptions = {
	items: string;
	orientation?: 'vertical' | 'horizontal' | 'grid';
	tabStop?: boolean;
};

/** Columns in a wrapped layout: how many leading items step rightwards before the first wrap
 *  (by left edge — tops differ when titles are vertically centred over 1–2 lines). */
export function columnsOf(els: HTMLElement[]): number {
	if (els.length < 2) return 1;
	let n = 1;
	let prev = els[0].getBoundingClientRect().left;
	for (let i = 1; i < els.length; i++) {
		const left = els[i].getBoundingClientRect().left;
		if (left <= prev + 1) break;
		prev = left;
		n++;
	}
	return n;
}

/** Pure: the index an arrow key moves to (or -1 when the key isn't ours). */
export function nextIndex(
	key: string,
	i: number,
	count: number,
	orientation: 'vertical' | 'horizontal' | 'grid',
	cols = 1
): number {
	let n: number;
	const vert = orientation !== 'horizontal';
	const horiz = orientation !== 'vertical';
	const row = orientation === 'grid' ? cols : 1;
	switch (key) {
		case 'ArrowDown':
			if (!vert) return -1;
			n = i + row;
			break;
		case 'ArrowUp':
			if (!vert) return -1;
			n = i - row;
			break;
		case 'ArrowRight':
			if (!horiz) return -1;
			n = i + 1;
			break;
		case 'ArrowLeft':
			if (!horiz) return -1;
			n = i - 1;
			break;
		case 'Home':
			n = 0;
			break;
		case 'End':
			n = count - 1;
			break;
		default:
			return -1;
	}
	return Math.max(0, Math.min(count - 1, n));
}

export function roving(opts: RovingOptions): Attachment<HTMLElement> {
	const orientation = opts.orientation ?? 'vertical';
	const tabStop = opts.tabStop ?? true;
	return (node) => {
		let current: HTMLElement | null = null;
		const all = () => [...node.querySelectorAll<HTMLElement>(opts.items)];

		let queued = false;
		const sync = () => {
			queued = false;
			if (!tabStop) return;
			const els = all();
			if (!els.length) return;
			if (!current || !els.includes(current)) {
				current =
					els.find((e) => e.matches('[aria-selected="true"],[aria-current="page"],[data-active]')) ??
					els[0];
			}
			for (const el of els) {
				const want = el === current ? 0 : -1;
				if (el.tabIndex !== want) el.tabIndex = want;
			}
		};
		const queue = () => {
			if (queued) return;
			queued = true;
			requestAnimationFrame(sync);
		};

		const onFocusIn = (e: FocusEvent) => {
			const el = (e.target as HTMLElement).closest<HTMLElement>(opts.items);
			if (!el || !node.contains(el) || el === current) return;
			current = el;
			sync();
		};

		const onKeyDown = (e: KeyboardEvent) => {
			if (e.altKey || e.metaKey || e.ctrlKey || e.defaultPrevented) return;
			const el = e.target as HTMLElement;
			if (!el.matches?.(opts.items) || !node.contains(el)) return;
			// Skip items that aren't rendered (display:none) — they can't take focus.
			const els = all().filter((x) => x.getClientRects().length > 0);
			const i = els.indexOf(el);
			if (i < 0) return;
			const n = nextIndex(e.key, i, els.length, orientation, orientation === 'grid' ? columnsOf(els) : 1);
			if (n < 0) return;
			e.preventDefault();
			e.stopPropagation();
			if (n === i) return;
			current = els[n];
			sync();
			current.focus({ preventScroll: true });
			current.scrollIntoView({ block: 'nearest', inline: 'nearest' });
		};

		const mo = new MutationObserver(queue);
		mo.observe(node, { childList: true, subtree: true });
		sync();
		node.addEventListener('focusin', onFocusIn);
		node.addEventListener('keydown', onKeyDown);
		return () => {
			mo.disconnect();
			node.removeEventListener('focusin', onFocusIn);
			node.removeEventListener('keydown', onKeyDown);
		};
	};
}
