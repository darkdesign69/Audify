// stream: tilefix — no browser broken-image glyph, anywhere.
//
// Kit components (Card, ShortcutTile, TrackRow, MediaCard…) swap a failed cover for their own
// placeholder tile. Dozens of other `<img>`s have no `onerror`, and a dead URL there paints
// WebKit's broken-image glyph (the blue "?"). This is the app-wide safety net: an image that fails
// is hidden (`data-img-broken`, styled `visibility: hidden` in layout.css) so its slot's own
// background shows through, and shown again as soon as anything loads into it (a component's
// retry with another URL, a new `src`). `error`/`load` don't bubble, so both are caught in the
// capture phase, which also runs before a component's own handler.

export const BROKEN_ATTR = 'data-img-broken';

type ImgLike = {
	tagName: string;
	setAttribute(n: string, v: string): void;
	removeAttribute(n: string): void;
};

function isImg(t: unknown): t is ImgLike {
	return !!t && typeof t === 'object' && (t as { tagName?: unknown }).tagName === 'IMG';
}

/** The capture-phase listener: hide on `error`, show again on `load`. */
export function onImageEvent(e: { type: string; target: unknown }) {
	if (!isImg(e.target)) return;
	if (e.type === 'error') e.target.setAttribute(BROKEN_ATTR, '');
	else if (e.type === 'load') e.target.removeAttribute(BROKEN_ATTR);
}

let installed = false;

/** Install once per window (idempotent). Also catches images that failed before it ran. */
export function installImageGuard(doc: Document = document) {
	if (installed) return;
	installed = true;
	doc.addEventListener('error', onImageEvent, true);
	doc.addEventListener('load', onImageEvent, true);
	for (const img of Array.from(doc.images)) {
		if (img.complete && img.naturalWidth === 0 && img.getAttribute('src')) {
			img.setAttribute(BROKEN_ATTR, '');
		}
	}
}
