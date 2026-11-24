// DEV-ONLY timing probe (stream: perf). Loaded by the root layout only when the backend was started
// with `ENCORE_PERF=<mode>` (lib.rs injects `window.__ENCORE_PERF__`), so a normal launch never
// downloads or runs it. Every measurement goes to the Rust log as one `perf: <name> {json}` line;
// scripts/perf-run.sh reads them back and samples RSS / CPU of the app while the probe sits in its
// idle phases. Modes:
//   startup  → log the startup marks, then quit (cold / warm start runs)
//   idle     → startup, then 30 s idle on Home for CPU sampling, then quit
//   full     → startup marks, search, page navigation, first audio, idle phases, navigation soak,
//              then quit. Plays at volume 0 and pauses before quitting.
//   audio    → (stream: firstaudio) first audio only: AUDIO_SET cold, then each again with its
//              stream URL cached, then (when the backend offers it) each after a simulated hover.
//              Volume 0, paused before quitting. The backend logs `fa:*` stage marks alongside.
import { goto } from '$app/navigation';
import { exit } from '@tauri-apps/plugin-process';
import * as api from '$lib/api';
import type { SongItem } from '$lib/api';
import { logUi } from '$lib/errlog';
import { playback, playSong } from '$lib/player.svelte';

declare global {
	interface Window {
		__ENCORE_PERF__?: string;
	}
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const frames = (n = 2) =>
	new Promise<void>((r) => {
		const step = (k: number) => (k ? requestAnimationFrame(() => step(k - 1)) : r());
		step(n);
	});
const round = (n: number) => Math.round(n * 10) / 10;

function log(name: string, data: Record<string, unknown> = {}) {
	const line = `perf: ${name} ${JSON.stringify({ ...data, epoch: Date.now() })}`;
	console.info(line);
	logUi('info', line);
}

async function until(test: () => boolean, timeout = 20_000): Promise<boolean> {
	const end = performance.now() + timeout;
	while (performance.now() < end) {
		if (test()) return true;
		await sleep(5);
	}
	return false;
}

const main = () => document.querySelector<HTMLElement>('[data-app-main]');
const rows = () => main()?.querySelectorAll('[role=row]').length ?? 0;
const h1 = () => main()?.querySelector('h1')?.textContent?.trim() ?? '';

/** Home is interactive when its skeletons are gone and at least one shelf heading is on screen. */
function homeReady(): boolean {
	const m = main();
	if (!m || location.pathname !== '/') return false;
	return !m.querySelector('.sk') && m.querySelectorAll('h2').length >= 1;
}
/** …and visually complete once the shortcut / card artwork above the fold has decoded. */
function homeImagesReady(): boolean {
	const m = main();
	if (!m) return false;
	const imgs = [...m.querySelectorAll('img')].filter((i) => i.getBoundingClientRect().top < innerHeight);
	return imgs.length >= 8 && imgs.every((i) => i.complete);
}

async function timed(name: string, act: () => unknown, ready: () => boolean, extra = {}) {
	const t0 = performance.now();
	await act();
	const ok = await until(ready);
	const readyMs = performance.now() - t0;
	await frames();
	log(name, { ms: round(readyMs), paintMs: round(performance.now() - t0), ok, ...extra });
	return ok;
}

async function startupMarks(mountedAt: number) {
	const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming | undefined;
	const ok = await until(homeReady, 90_000);
	await frames();
	const homeInteractive = performance.now();
	const shelves = main()?.querySelectorAll('h2').length ?? 0;
	const imagesOk = await until(homeImagesReady, 15_000);
	log('startup', {
		ok,
		domContentLoaded: round(nav?.domContentLoadedEventEnd ?? 0),
		layoutMounted: round(mountedAt),
		homeInteractive: round(homeInteractive),
		homeImages: imagesOk ? round(performance.now()) : null,
		shelves,
		timeOrigin: Math.round(performance.timeOrigin),
		resources: performance.getEntriesByType('resource').length,
		jsResources: performance.getEntriesByType('resource').filter((r) => /\.js(\?|$)/.test(r.name)).length
	});
}

/** Results are in when the URL carries the query, the result shelves have headings, and nothing
 *  above the fold is still a skeleton (the podcast shelves further down load on their own). */
function searchReady(q: string, before: string): boolean {
	const m = main();
	if (!m || new URLSearchParams(location.search).get('q') !== q) return false;
	const top = m.getBoundingClientRect().top + 700;
	const busy = [...m.querySelectorAll('.sk')].some((e) => e.getBoundingClientRect().top < top);
	return !busy && m.querySelectorAll('h2').length >= 2 && sig() !== before;
}
const sig = () =>
	[...(main()?.querySelectorAll('img') ?? [])]
		.slice(0, 4)
		.map((i) => i.src)
		.join(',');

async function search(q: string) {
	const input = document.querySelector<HTMLInputElement>('input[placeholder="Songs, artists or podcasts"]');
	if (!input) return log('search', { q, error: 'no input' });
	const before = sig();
	await timed(
		'search',
		() => {
			input.focus();
			input.value = q;
			input.dispatchEvent(new Event('input', { bubbles: true }));
		},
		() => searchReady(q, before),
		{ q }
	);
}

/** `ms`: the page's title and first rows are on screen. `settledMs`: no skeleton left anywhere
 *  (Recommended, More by, …), which can wait on slow network sections below the fold. */
async function page(name: string, href: string, ready: () => boolean) {
	const t0 = performance.now();
	await goto(href);
	const here = () => decodeURIComponent(location.pathname) === decodeURIComponent(href.split('?')[0]);
	const ok = await until(() => here() && ready());
	const ms = performance.now() - t0;
	await frames();
	const paintMs = performance.now() - t0;
	const settled = await until(() => here() && !main()?.querySelector('.sk'), 15_000);
	log(name, { ms: round(ms), paintMs: round(paintMs), settledMs: settled ? round(performance.now() - t0) : null, ok, href, rows: rows(), title: h1().slice(0, 40) });
}

async function firstAudio(name: string, song: SongItem) {
	// stream: firstaudio. Not `timed`: the `play` command resolves only after the radio is hydrated
	// and the next track primed, which the user never waits for. Poll from the click instead.
	const t0 = performance.now();
	const done = Promise.resolve(playSong(song)).catch(() => {});
	// The outgoing track's position is still on screen until the new one's first tick resets it.
	let reset = false;
	// stream: instant. Tap to the player bar showing the tapped title (the UI's answer), apart from
	// tap to audible (`ms`).
	let uiMs: number | null = null;
	const bar = () => document.querySelector('[data-player-bar]')?.textContent ?? '';
	const ok = await until(() => {
		if (uiMs === null && bar().includes(song.title)) uiMs = performance.now() - t0;
		if (playback.now?.videoId !== song.video_id) return false;
		if (playback.position <= 0.05) reset = true;
		return reset && !playback.paused && playback.position > 0.05;
	});
	const ms = performance.now() - t0;
	log(name, { ms: round(ms), uiMs: uiMs === null ? null : round(uiMs), ok, videoId: song.video_id });
	await done;
}

const step = async (name: string, f: () => Promise<unknown>) => {
	try {
		await f();
	} catch (e) {
		log(`${name}:error`, { message: String(e).slice(0, 200) });
	}
};

/** Synthetic tracks for the big local playlists (real thumbnail, fake ids: never played). */
function fakeSongs(n: number, tag: string): SongItem[] {
	return Array.from({ length: n }, (_, i) => ({
		video_id: `pf${tag}${String(i).padStart(11 - 2 - tag.length, '0')}`,
		title: `Perf track ${i + 1}`,
		artists: `Artist ${i % 97}`,
		album: `Album ${i % 211}`,
		duration: `${2 + (i % 4)}:${String(i % 60).padStart(2, '0')}`,
		thumbnail: 'https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg'
	})) as unknown as SongItem[];
}

async function bigPlaylist(name: string, n: number, tag: string): Promise<string> {
	const have = (await api.libPlaylists()).find((p) => p.name === name);
	if (have) return have.id;
	const p = await api.libPlaylistCreate(name);
	const songs = fakeSongs(n, tag);
	for (let i = 0; i < songs.length; i += 500) await api.libPlaylistAdd(p.id, songs.slice(i, i + 500), true);
	return p.id;
}

/** Real tracks for the first-audio runs, so no search is needed. */
const AUDIO: SongItem[] = [
	{ video_id: 'FGBhQbmPwH8', title: 'One More Time', artists: 'Daft Punk', duration: '5:21' },
	{ video_id: 'kXYiU_JCYtU', title: 'Numb', artists: 'Linkin Park', duration: '3:07' }
] as unknown as SongItem[];

/** stream: firstaudio. Distinct real tracks for the `audio` mode (none of them in AUDIO). */
const AUDIO_SET: SongItem[] = [
	{ video_id: 'dQw4w9WgXcQ', title: 'Never Gonna Give You Up', artists: 'Rick Astley', duration: '3:33' },
	{ video_id: 'hTWKbfoikeg', title: 'Smells Like Teen Spirit', artists: 'Nirvana', duration: '5:01' },
	{ video_id: 'JGwWNGJdvx8', title: 'Shape of You', artists: 'Ed Sheeran', duration: '4:23' },
	{ video_id: 'fJ9rUzIMcZQ', title: 'Bohemian Rhapsody', artists: 'Queen', duration: '5:59' },
	{ video_id: 'kJQP7kiw5Fk', title: 'Despacito', artists: 'Luis Fonsi', duration: '4:41' },
	{ video_id: 'RgKAFK5djSk', title: 'See You Again', artists: 'Wiz Khalifa', duration: '3:57' }
] as unknown as SongItem[];

/** Tracks for the hint runs: never played before in the run, so each starts cold. */
const HOVER_SET: SongItem[] = [
	{ video_id: 'OPf0YbXqDm0', title: 'Uptown Funk', artists: 'Mark Ronson', duration: '4:30' },
	{ video_id: 'YQHsXMglC9A', title: 'Hello', artists: 'Adele', duration: '6:07' },
	{ video_id: '09R8_2nJtjg', title: 'Sugar', artists: 'Maroon 5', duration: '5:01' },
	{ video_id: 'CevxZvSJLk8', title: 'Roar', artists: 'Katy Perry', duration: '4:30' },
	{ video_id: 'nfWlot6h_JM', title: 'Shake It Off', artists: 'Taylor Swift', duration: '4:02' },
	{ video_id: '60ItHLz5WEA', title: 'Faded', artists: 'Alan Walker', duration: '3:32' }
] as unknown as SongItem[];
const PRESS_SET: SongItem[] = [
	{ video_id: 'pRpeEdMmmQ0', title: 'Waka Waka', artists: 'Shakira', duration: '3:31' },
	{ video_id: 'lp-EO5I60KA', title: 'Thinking Out Loud', artists: 'Ed Sheeran', duration: '4:57' },
	{ video_id: 'hT_nvWreIhg', title: 'Counting Stars', artists: 'OneRepublic', duration: '4:43' },
	{ video_id: 'YqeW9_5kURI', title: 'Lean On', artists: 'Major Lazer', duration: '2:59' },
	{ video_id: 'fKopy74weus', title: 'Thunder', artists: 'Imagine Dragons', duration: '3:24' },
	{ video_id: 'ru0K8uYEZWw', title: "Can't Stop the Feeling", artists: 'Justin Timberlake', duration: '4:46' }
] as unknown as SongItem[];

async function audioOnly() {
	// Let the launch's own network (cipher, PoToken, visitor data, Home) settle first.
	await sleep(8000);
	const vol = playback.volume;
	await api.setVolume(0);
	const hover = (api as unknown as Record<string, unknown>).hintPlay as
		| ((s: SongItem) => Promise<unknown>)
		| undefined;
	await step('audio', async () => {
		for (const [i, s] of AUDIO_SET.entries()) {
			await firstAudio(`audio:cold:${i}`, s);
			await sleep(3000);
		}
		for (const [i, s] of AUDIO_SET.entries()) {
			await firstAudio(`audio:again:${i}`, s);
			await sleep(3000);
		}
		// A row rested on (the UI hints after 250 ms) and clicked 350 ms after the hint, and one
		// rested on longer (clicked 3 s after). Without a hint command these are plain cold clicks.
		for (const [set, kind, gap] of [[HOVER_SET, 'hover', 350], [PRESS_SET, 'rest', 3000]] as const) {
			for (const [i, s] of set.entries()) {
				hover?.(s).catch(() => {});
				await sleep(gap);
				await firstAudio(`audio:${kind}:${i}`, s);
				await sleep(3000);
			}
		}
	});
	if (!playback.paused) await api.togglePause();
	await until(() => playback.paused, 5000);
	await api.setVolume(vol || 0.5);
}

async function full() {
	await sleep(3000);
	const tableReady = () => !!h1() && rows() >= 5;
	const pl = (id: string) => `/playlist/${encodeURIComponent(id)}`;
	// ── search (keystroke → results), three fresh queries, then a repeat ──
	for (const q of ['daft punk', 'radiohead', 'billie eilish', 'the beatles']) {
		await step('search', () => search(q));
		await sleep(1500);
	}
	// ── local pages: 1,000 and 5,000-track playlists (no network) ──
	let p1k = '';
	let p5k = '';
	await step('seed', async () => {
		const t0 = performance.now();
		p1k = await bigPlaylist('Perf 1k', 1000, 'a');
		p5k = await bigPlaylist('Perf 5k', 5000, 'b');
		log('seed', { ms: Math.round(performance.now() - t0) });
	});
	// ── YouTube pages (need the network; ids from search, else well-known ones) ──
	let album = 'MPREb_K8qWMWVqXGi';
	let artist = 'UCRr1xG_2WIDs18a6cIiCxeA';
	await step('ids', async () => {
		const flat = JSON.stringify(await api.searchAll('daft punk'));
		album = /"(MPREb_[\w-]+)"/.exec(flat)?.[1] ?? album;
		artist = /"(UC[\w-]{22})"/.exec(flat)?.[1] ?? artist;
	});
	for (const pass of ['cold', 'warm']) {
		if (p1k) await step('nav', () => page(`nav:playlist-1k:${pass}`, pl(p1k), tableReady));
		await sleep(800);
		if (p5k) await step('nav', () => page(`nav:playlist-5k:${pass}`, pl(p5k), tableReady));
		await sleep(800);
		await step('nav', () => page(`nav:album:${pass}`, `/album/${album}`, tableReady));
		await sleep(800);
		await step('nav', () => page(`nav:artist:${pass}`, `/artist/${artist}`, () => !!h1()));
		await sleep(800);
		await step('nav', () => page(`nav:home:${pass}`, '/', homeReady));
		await sleep(800);
	}
	// ── scrolling: the 5k playlist end to end, then Home ──
	if (p5k)
		await step('scroll', async () => {
			await goto(pl(p5k));
			await until(tableReady);
			await sleep(1500);
			await scrollFrames('scroll:playlist-5k', 6000, 60);
		});
	await step('scroll', async () => {
		await goto('/');
		await until(homeReady);
		await sleep(1500);
		await scrollFrames('scroll:home');
	});

	// ── first audio, at volume 0 ──
	const vol = playback.volume;
	await api.setVolume(0);
	await step('audio', async () => {
		await firstAudio('audio:cold', AUDIO[0]);
		await sleep(2500);
		await firstAudio('audio:cold-2', AUDIO[1]);
		await sleep(2500);
		await firstAudio('audio:again', AUDIO[0]);
		await sleep(1000);
	});
	// ── idle phases: the runner samples CPU between start and end ──
	if (!playback.paused) {
		log('phase:playing-idle:start');
		await sleep(30_000);
		log('phase:playing-idle:end');
		await api.togglePause();
		await until(() => playback.paused, 5000);
	}
	await sleep(2000);
	log('phase:paused-idle:start');
	await sleep(30_000);
	log('phase:paused-idle:end');
	// ── soak: ~30 min of browsing compressed (150 page visits) ──
	log('phase:soak:start');
	const route = ['/', pl(p1k || p5k), '/search?q=radiohead', `/album/${album}`, pl(p5k || p1k), '/history', `/artist/${artist}`];
	const t0 = performance.now();
	for (let i = 0; i < 150; i++) {
		await step('soak', () => goto(route[i % route.length]));
		await sleep(400);
	}
	await goto('/');
	await until(homeReady);
	log('phase:soak:end', { ms: Math.round(performance.now() - t0) });
	await sleep(8000);
	log('phase:after-soak-idle:end');
	if (!playback.paused) await api.togglePause();
	await api.setVolume(vol || 0.5);
}

async function scrollFrames(name: string, ms = 5000, px = 40) {
	const el = main();
	if (!el) return;
	el.scrollTop = 0;
	await sleep(300);
	const deltas: number[] = [];
	let last = performance.now();
	const t0 = last;
	let dir = 1;
	await new Promise<void>((done) => {
		const f = (t: number) => {
			deltas.push(t - last);
			last = t;
			if (el.scrollTop + el.clientHeight >= el.scrollHeight - 2) dir = -1;
			else if (el.scrollTop <= 0) dir = 1;
			el.scrollTop += dir * px;
			if (t - t0 < ms) requestAnimationFrame(f);
			else done();
		};
		requestAnimationFrame(f);
	});
	deltas.shift();
	const s = [...deltas].sort((x, y) => x - y);
	const q = (p: number) => round(s[Math.min(s.length - 1, Math.floor(p * s.length))] ?? 0);
	const avg = deltas.reduce((x, y) => x + y, 0) / Math.max(1, deltas.length);
	log(name, {
		frames: deltas.length,
		avgMs: round(avg),
		p50: q(0.5),
		p95: q(0.95),
		p99: q(0.99),
		max: q(1),
		janky: deltas.filter((d) => d > avg * 1.5 + 4).length,
		scrollHeight: el.scrollHeight
	});
}

/** Called once from the root layout's onMount. */
export async function runProbe(mode: string, mountedAt: number) {
	try {
		await startupMarks(mountedAt);
		if (mode === 'full') await full();
		if (mode === 'audio') await audioOnly(); // stream: firstaudio
		if (mode === 'idle') {
			// Nothing played: what the app costs sitting on Home.
			await sleep(5000);
			log('phase:paused-idle:start');
			await sleep(30_000);
			log('phase:paused-idle:end');
		}
	} catch (e) {
		log('error', { message: String(e) });
	}
	log('done');
	await sleep(500);
	await exit(0);
}
