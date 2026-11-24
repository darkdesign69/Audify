// Podcast UI helpers (stream p-artist-pods): Spotify's episode wording and the one play path.
import * as api from '$lib/api';
import type { PodcastEpisode, PodcastShow } from '$lib/api';
import { openPlayer, playback } from '$lib/player.svelte';
import { lib } from '$lib/library.svelte';

export const episodeHref = (id: string) => `/episode/${encodeURIComponent(id)}`;

/** "1 hr 23 min" / "45 min" / "52 sec" (Spotify's episode duration wording). */
export function spokenDuration(secs: number | null | undefined): string {
	const s = Math.max(0, Math.round(secs ?? 0));
	if (!s) return '';
	const h = Math.floor(s / 3600);
	const m = Math.floor((s % 3600) / 60);
	if (h) return m ? `${h} hr ${m} min` : `${h} hr`;
	if (m) return `${m} min`;
	return `${s} sec`;
}

const MONTH = new Intl.DateTimeFormat('en-US', { day: 'numeric', month: 'short' });
const MONTH_YEAR = new Intl.DateTimeFormat('en-US', { day: 'numeric', month: 'short', year: 'numeric' });

/** Spotify English: "Nov 13" this year, "Nov 13, 2024" before; "Today" for today (Spotify shows
 *  yesterday's episode as "Nov 14", not "Yesterday"). */
export function episodeDate(unix: number | null | undefined, now = Date.now()): string {
	if (!unix) return '';
	const d = new Date(unix * 1000);
	const today = new Date(now);
	today.setHours(0, 0, 0, 0);
	const day = new Date(d);
	day.setHours(0, 0, 0, 0);
	const diff = Math.round((today.getTime() - day.getTime()) / 86_400_000);
	if (diff === 0) return 'Today';
	return (d.getFullYear() === today.getFullYear() ? MONTH : MONTH_YEAR).format(d);
}

/** Show notes are HTML; rows show them as one plain paragraph. */
export function plainText(html: string | null | undefined): string {
	if (!html) return '';
	if (typeof DOMParser !== 'undefined') {
		const doc = new DOMParser().parseFromString(html.replace(/<(br|\/p|\/li)\s*\/?>/gi, ' $&'), 'text/html');
		return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim();
	}
	return html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
}

export const isEpisodePlaying = (id: string) => playback.now?.videoId === id && !playback.paused;
export const isEpisodeCurrent = (id: string) => playback.now?.videoId === id;

/**
 * Play `episodes` from `start` as one queue, recorded under the show (or Your Episodes) context.
 * No `sourceId` / continuation: those are YouTube-playlist concepts (podcasts.md); resume, played
 * state and SponsorBlock are handled in Rust. The current episode toggles pause instead.
 */
export function playEpisodes(
	episodes: PodcastEpisode[],
	start: number,
	context: { id: string; title: string; subtitle?: string; thumbnail?: string | null; kind?: 'podcast' | 'playlist' }
) {
	const ep = episodes[start];
	if (!ep) return;
	if (isEpisodeCurrent(ep.id)) return api.togglePause();
	lib.notePlayed({ kind: 'playlist', id: context.id, title: context.title, subtitle: context.subtitle, thumbnail: context.thumbnail ?? undefined });
	openPlayer();
	return api.playPlaylistFrom(episodes.map(api.episodeSong), start, {
		kind: context.kind ?? 'podcast',
		id: context.id,
		title: context.title,
		subtitle: context.subtitle,
		thumbnail: context.thumbnail ?? undefined
	});
}

export const showContext = (s: PodcastShow) => ({
	id: s.id,
	title: s.title,
	subtitle: s.author,
	thumbnail: s.thumbnail
});

// ── Episode menu ("…" and right-click) ───────────────────────────────────────────────────────
import type { MenuEntry } from '$lib/kit/menus';
import { episodeDownloadEntry, openMenu } from '$lib/kit/menus';
import { toast } from '$lib/kit/toast';
import { copyText } from '$lib/clipboard';
import { enqueue } from '$lib/player.svelte';
import { goto } from '$app/navigation';
import { isSaved, progressOf, setPlayed, toggleSaved } from '$lib/podcasts.svelte';
import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
import QueueIcon from 'phosphor-svelte/lib/QueueIcon';
import ArrowBendDownRightIcon from 'phosphor-svelte/lib/ArrowBendDownRightIcon';
import MicrophoneIcon from 'phosphor-svelte/lib/MicrophoneIcon';
import LinkIcon from 'phosphor-svelte/lib/LinkIcon';
import ShareFatIcon from 'phosphor-svelte/lib/ShareFatIcon';
import CheckIcon from 'phosphor-svelte/lib/CheckIcon';

export const episodeShareUrl = (ep: PodcastEpisode): string | null =>
	ep.videoId
		? `https://music.youtube.com/watch?v=${ep.videoId}`
		: (ep.enclosureUrl ?? (ep.id.startsWith('PODURL:') ? ep.id.slice(7) : null));

export async function toggleSavedToast(ep: PodcastEpisode) {
	try {
		const saved = await toggleSaved(ep);
		toast(saved ? 'Added to Your Episodes' : 'Removed from Your Episodes');
	} catch (e) {
		toast.error(String(e));
	}
}

export async function setPlayedSafe(ep: PodcastEpisode, played: boolean) {
	try {
		await setPlayed(ep, played);
	} catch (e) {
		toast.error(String(e));
	}
}

export async function shareEpisode(ep: PodcastEpisode) {
	const url = episodeShareUrl(ep);
	if (!url) return;
	try {
		await copyText(url);
		toast('Link copied to clipboard');
	} catch (e) {
		toast.error(String(e));
	}
}

export function episodeMenu(ep: PodcastEpisode, opts: { showLink?: boolean } = {}): MenuEntry[] {
	const played = progressOf(ep).played;
	const saved = isSaved(ep.id);
	const song = api.episodeSong(ep);
	const entries: MenuEntry[] = [
		{
			label: saved ? 'Remove from Your Episodes' : 'Add to Your Episodes',
			icon: saved ? CheckCircleIcon : PlusCircleIcon,
			onSelect: () => toggleSavedToast(ep)
		},
		{
			label: played ? 'Mark as unplayed' : 'Mark as played',
			icon: CheckIcon,
			onSelect: () => setPlayedSafe(ep, !played)
		},
		episodeDownloadEntry(song),
		{ type: 'separator' },
		{ label: 'Add to queue', icon: QueueIcon, onSelect: () => enqueue([song], false) },
		{ label: 'Play next', icon: ArrowBendDownRightIcon, onSelect: () => enqueue([song], true) }
	];
	if (opts.showLink !== false && ep.showId) {
		entries.push({ type: 'separator' }, { label: 'Go to show', icon: MicrophoneIcon, onSelect: () => goto(`/show/${encodeURIComponent(ep.showId!)}`) });
	}
	if (episodeShareUrl(ep)) {
		entries.push(
			{ type: 'separator' },
			{ type: 'sub', label: 'Share', icon: ShareFatIcon, items: [{ label: 'Copy link to episode', icon: LinkIcon, onSelect: () => shareEpisode(ep) }] }
		);
	}
	return entries;
}

export const openEpisodeMenu = (target: MouseEvent | HTMLElement, ep: PodcastEpisode, opts?: { showLink?: boolean }) =>
	openMenu(target, episodeMenu(ep, opts));
