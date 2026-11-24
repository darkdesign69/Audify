<script lang="ts">
	/**
	 * The takeover of the main view (lyrics-and-expanded-player.md "Lyrics" + "Expanded Now
	 * Playing"): one rounded panel in the track's lyrics colour. Toolbar top-right (32px white .7 → 1
	 * icon buttons, 8px gaps): Artwork · Video (music videos only) · Artist image · Lyrics |
	 * "Switch to video" pill · … (lyrics source / timing) · Full screen · Minimise.
	 * Modes: lyrics (LyricLines), artwork (big cover + title 32/700 + artists over a cover-colour
	 * gradient), artist (full-bleed artist photo with the name), video (16:9 on black).
	 * The tab title reads "Track • Artist" while open.
	 */
	import { offersLyrics } from '$lib/nowlyrics.svelte';
	import { onDestroy } from 'svelte';
	import { playback, ui } from '$lib/player.svelte';
	import { thumb } from '$lib/thumb';
	import Icon from '$lib/kit/Icon.svelte';
	import Tooltip from '$lib/kit/Tooltip.svelte';
	import { albumHref, artistHref } from '$lib/kit/actions';
	import { canSwitch, canVideo, pickVersion, showVideo, video, videoSide } from '$lib/video.svelte';
	import { nowPlayingSong } from '$lib/shell/nowplaying';
	import LyricLines from './LyricLines.svelte';
	import VideoBox from './VideoBox.svelte';
	import { LyricsTint } from './palette.svelte';
	import { closeExpanded, npui, type ExpandedMode } from './state.svelte';
	import { loadLyricsProviders, openLyricsMenu } from './menus';
	import { artistInfo } from './info';
	import type * as api from '$lib/api';
	import ImageIcon from 'phosphor-svelte/lib/ImageIcon';
	import MonitorPlayIcon from 'phosphor-svelte/lib/MonitorPlayIcon';
	import UserCircleIcon from 'phosphor-svelte/lib/UserCircleIcon';
	import MicrophoneStageIcon from 'phosphor-svelte/lib/MicrophoneStageIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import CornersOutIcon from 'phosphor-svelte/lib/CornersOutIcon';
	import ArrowsInSimpleIcon from 'phosphor-svelte/lib/ArrowsInSimpleIcon';

	const now = $derived(playback.now);
	const tint = new LyricsTint(() => thumb(playback.now?.thumbnail, 120));
	const song = $derived(now ? nowPlayingSong() : null);
	const artistId = $derived(now?.artistId ?? now?.artistRuns?.find((r) => r.id)?.id);

	loadLyricsProviders();

	// Video mode only while there is a picture to show; otherwise the artwork stands in.
	const mode = $derived<ExpandedMode>(
		(npui.mode === 'video' && !showVideo()) || (npui.mode === 'lyrics' && !offersLyrics(now)) ? 'artwork' : npui.mode
	);

	let artist = $state<api.ArtistPage | null>(null);
	$effect(() => {
		const id = mode === 'artist' ? artistId : undefined;
		artist = null;
		if (!id) return;
		let stale = false;
		artistInfo(id)
			.then((a) => !stale && (artist = a))
			.catch(() => {});
		return () => (stale = true);
	});

	// "Track • Artist" in the tab/window title while open.
	const before = typeof document !== 'undefined' ? document.title : '';
	$effect(() => {
		if (now) document.title = `${now.title} • ${now.artists}`;
	});
	onDestroy(() => {
		if (typeof document !== 'undefined') document.title = before;
	});

	const modes = $derived([
		{ id: 'artwork' as const, label: 'Show artwork', icon: ImageIcon, show: true },
		{ id: 'video' as const, label: 'Show video', icon: MonitorPlayIcon, show: canVideo() },
		{ id: 'artist' as const, label: 'Show artist image', icon: UserCircleIcon, show: !!artistId },
		{ id: 'lyrics' as const, label: 'Show lyrics', icon: MicrophoneStageIcon, show: offersLyrics(now) }
	]);
</script>

{#snippet tool(label: string, icon: typeof ImageIcon, on: boolean, run: (e: MouseEvent) => void)}
	<Tooltip text={label} side="bottom">
		{#snippet trigger(props)}
			<button
				{...props}
				type="button"
				aria-label={label}
				aria-pressed={on}
				class="grid size-8 place-items-center rounded-full transition-[color,transform] duration-100 hover:scale-[1.04] hover:text-white active:scale-100 {on
					? 'text-white'
					: 'text-white/70'}"
				onclick={run}
			>
				<Icon {icon} size={16} fill={on} />
			</button>
		{/snippet}
	</Tooltip>
{/snippet}

<section
	class="relative h-full overflow-hidden rounded-lg transition-colors duration-300"
	style="background-color:{mode === 'video' ? '#000' : tint.bg}"
	aria-label={mode === 'lyrics' ? 'Lyrics' : 'Now playing'}
>
	{#if now}
		{#if mode === 'lyrics'}
			<LyricLines line={tint.line} fadeTop />
		{:else if mode === 'artwork'}
			<div
				class="flex h-full flex-col items-center justify-center gap-6 px-16 py-20"
				style="background:linear-gradient(to bottom,{tint.bg},{tint.deep})"
			>
				<img
					src={thumb(now.thumbnail, 720)}
					alt=""
					class="aspect-square h-[60%] max-h-[640px] max-w-full rounded-lg object-cover shadow-[0_8px_40px_rgba(0,0,0,.6)]"
				/>
				<div class="max-w-full text-center">
					<div class="truncate text-2xl font-bold text-white">
						{#if song?.album_id}<a class="hover:underline" href={albumHref(song.album_id)}>{now.title}</a>{:else}{now.title}{/if}
					</div>
					<div class="mt-1 truncate text-base text-white/70">
						{#if artistId}<a class="hover:text-white hover:underline" href={artistHref(artistId)}>{now.artists}</a>{:else}{now.artists}{/if}
					</div>
				</div>
			</div>
		{:else if mode === 'artist'}
			<div class="relative h-full">
				{#if artist?.thumbnail || artist?.avatar}
					<img src={thumb(artist.thumbnail ?? artist.avatar, 1200)} alt="" class="absolute inset-0 size-full object-cover" />
				{/if}
				<div class="absolute inset-0 bg-gradient-to-t from-black/70 via-transparent to-black/30"></div>
				<div class="absolute bottom-10 left-16 right-16">
					<a href={artistId ? artistHref(artistId) : undefined} class="text-5xl leading-none font-extrabold tracking-[-0.04em] text-white hover:underline">
						{artist?.name ?? now.artists}
					</a>
				</div>
			</div>
		{:else}
			<div class="flex h-full items-center justify-center p-16">
				<VideoBox fit="contain" class="aspect-video max-h-full w-full rounded-lg" />
			</div>
		{/if}
	{/if}

	{#if playback.queue.sourceName}
		<!-- Spotify: the context playing from, top-left (parity3) -->
		<p class="pointer-events-none absolute top-4 left-6 max-w-[40%] truncate text-base leading-8 font-bold text-white">
			{playback.queue.sourceName}
		</p>
	{/if}
	<div class="absolute top-4 right-4 flex items-center gap-2">
		{#each modes.filter((m) => m.show) as m (m.id)}
			{@render tool(m.label, m.icon, mode === m.id, () => (npui.mode = m.id))}
		{/each}
		<div class="mx-2 h-4 w-px bg-white/30"></div>
		{#if canSwitch()}
			<button
				type="button"
				class="flex h-8 items-center gap-2 rounded-full bg-white/10 px-3 text-sm font-bold text-white transition-transform hover:scale-[1.04] hover:bg-white/20 active:scale-100 disabled:opacity-50"
				disabled={video.switching !== null}
				onclick={() => {
					const on = !videoSide();
					pickVersion(on);
					if (on) npui.mode = 'video';
				}}
			>
				<Icon icon={MonitorPlayIcon} size={16} />
				{videoSide() ? 'Switch to audio' : 'Switch to video'}
			</button>
		{/if}
		{#if mode === 'lyrics'}
			{@render tool('More options', DotsThreeIcon, false, (e) => openLyricsMenu(e))}
		{/if}
		{@render tool('Enter Full screen', CornersOutIcon, false, () => (ui.theaterOpen = true))}
		{@render tool('Minimize Now Playing view', ArrowsInSimpleIcon, false, closeExpanded)}
	</div>
</section>
