import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChild<T> = T extends { child?: any } ? Omit<T, "child"> : T;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type WithoutChildren<T> = T extends { children?: any } ? Omit<T, "children"> : T;
export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
export type WithElementRef<T, U extends HTMLElement = HTMLElement> = T & { ref?: U | null };

/** The titlebar sits above dialog backdrops (#427). A press on its drag region or a window button
 *  moves or resizes the window, it isn't a click on the page behind, so a dialog's
 *  `onInteractOutside` should leave the dialog open. The bar's app buttons still dismiss it. */
export function onWindowChrome(e: Event) {
	const el = e.target as Element;
	return !!(el.matches?.("[data-tauri-drag-region]") || el.closest?.("[data-window-control]") || isWindowDrag(el));
}

/** Controls (and the search field and its dropdown) inside a `data-window-drag` strip. */
const INTERACTIVE =
	'button, input, textarea, select, a, label, form, [role="button"], [role="option"], [role="listbox"], [role="menuitem"], [contenteditable="true"], [tabindex]:not([tabindex="-1"])';

/** True when a press lands on the empty part of a `data-window-drag` strip (the top bar), so it
 *  moves the window. Unlike Tauri's `data-tauri-drag-region`, this covers the strip's empty child
 *  wrappers too, and leaves every control clickable. */
export function isWindowDrag(el: Element | null) {
	return !!(el?.closest?.("[data-window-drag]") && !el.closest(INTERACTIVE));
}
