// a11y: modal focus handling for overlays that aren't bits-ui dialogs (fullscreen player…).
// On attach: remember what had focus and move focus into the node; Tab / ⇧Tab cycle inside it;
// on detach: give focus back. bits-ui Dialog/AlertDialog/menus already do this themselves.
import type { Attachment } from 'svelte/attachments';

const FOCUSABLE =
	'a[href],button:not([disabled]),input:not([disabled]):not([type=hidden]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"]),[contenteditable=true]';

export function trapFocus(): Attachment<HTMLElement> {
	return (node) => {
		const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
		if (!node.contains(document.activeElement)) node.focus({ preventScroll: true });
		const items = () =>
			[...node.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
				(el) => el.getClientRects().length > 0 && !el.closest('[inert]')
			);
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== 'Tab' || e.defaultPrevented) return;
			const els = items();
			if (!els.length) {
				e.preventDefault();
				return;
			}
			const first = els[0];
			const last = els[els.length - 1];
			const a = document.activeElement;
			if (e.shiftKey && (a === first || a === node || !node.contains(a))) {
				e.preventDefault();
				last.focus();
			} else if (!e.shiftKey && (a === last || !node.contains(a))) {
				e.preventDefault();
				first.focus();
			}
		};
		node.addEventListener('keydown', onKey);
		return () => {
			node.removeEventListener('keydown', onKey);
			if (before?.isConnected) before.focus({ preventScroll: true });
		};
	};
}
