import { convertFileSrc } from '@tauri-apps/api/core';

// Rewrite a Google image URL to (about) the pixel size a slot actually renders, so WebKitGTK
// doesn't decode a 544px (or 1080p) image for a 40px row. Only lh3/yt3 googleusercontent-style
// URLs carry the size in the URL (`=w544-h544` / `=s576` suffixes). A video's i.ytimg thumb has its
// sizes as separate files that can 404, so only the big player swaps one (see below); anything else
// is returned unchanged.
export function thumb(url: string | undefined | null, px: number): string | undefined {
	if (!url) return undefined;
	// stream: downloads — offline, a downloaded item's artwork comes from its saved file.
	const saved = artResolver?.(url);
	if (saved) return convertFileSrc(saved);
	// Local library artwork is a path on this machine, not a URL. The webview can't open a bare
	// path, so hand it through Tauri's asset protocol. Kept here rather than at the command
	// boundary so what gets stored (queue, Shortcuts) stays the real path — which is also what
	// MPRIS needs.
	if (url.startsWith('/') || /^[A-Za-z]:[\\/]/.test(url)) {
		const { path, query } = localPath(url);
		return convertFileSrc(path) + query;
	}
	// Every InnerTube reply, /player included, tops a video's thumbs out at 426x240, which the big
	// player crops square and stretches (#425). Ask for the 1280x720 file instead; not every video
	// has one, so the caller steps down to the original on a miss (`isMissingThumb`).
	const video = /^https:\/\/i\.ytimg\.com\/vi\/([\w-]{11})\//.exec(url);
	if (video && px >= 720) return `https://i.ytimg.com/vi/${video[1]}/maxresdefault.jpg`;
	if (/=w\d+-h\d+/.test(url)) return url.replace(/=w\d+-h\d+/, `=w${px}-h${px}`);
	if (/=s\d+/.test(url)) return url.replace(/=s\d+/, `=s${px}`);
	return url;
}

// ── stream: coverfix ──
/** A local file reference split for the asset protocol: the path to convert, and a `?v=…`
 *  cache-buster to append after it (Made for you covers are overwritten in place and carry their
 *  mtime there; the asset protocol ignores the query, `convertFileSrc` would encode it into the
 *  path). A pre-coverfix dated Made for you cover (`covers/mfy/MFY_X-<secs>.svg`, still in a Home
 *  pre-painted from `localStorage` or the page cache) maps to its stable name `MFY_X.svg`, which
 *  the app migrates the file to at startup, so the old URL never shows a broken image. */
export function localPath(url: string): { path: string; query: string } {
	// Only our own `?v=<digits>` suffix: a local file may have a `?` in its name.
	const m = /\?v=\d+$/.exec(url);
	let path = m ? url.slice(0, m.index) : url;
	const query = m ? m[0] : '';
	path = path.replace(/([\\/]covers[\\/]mfy[\\/][A-Za-z0-9_]+)-\d+\.svg$/, '$1.svg');
	return { path, query };
}
// ── end stream: coverfix ──

// i.ytimg answers a size it doesn't have with a 404 *and* a 120x90 grey picture, which a webview
// may load without firing `error`. Read that picture as the miss it is.
export function isMissingThumb(e: Event): boolean {
	const img = e.currentTarget as HTMLImageElement;
	return img.naturalWidth === 120 && img.naturalHeight === 90;
}

// ── stream: downloads ──
let artResolver: ((url: string) => string | undefined) | null = null;
/** Installed by `downloads.svelte.ts`: a thumbnail URL → a local artwork path, or undefined. It
 *  reads reactive state, so slots re-render when the app goes offline or online. */
export function setArtResolver(f: (url: string) => string | undefined) {
	artResolver = f;
}
// ── end stream: downloads ──
