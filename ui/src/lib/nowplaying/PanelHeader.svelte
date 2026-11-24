<script lang="ts">
	/**
	 * Right-panel header (now-playing-panel.md "Header", queue-panel.md, Connect): 64px row, title
	 * 16/700 white. Now Playing: the context name (a link to whatever the queue came from: playlist,
	 * album, artist, Liked Songs, mix, radio page or search), then the
	 * sleep-timer countdown chip (when set), "…" (track menu + Sleep timer ▸) and "Hide Now Playing
	 * view". Queue / Connect: title + X. Gets the panel bg's shadow once the panel is scrolled.
	 */
	import { playback, playingFrom } from '$lib/player.svelte';
	import * as api from '$lib/api';
	import { cardMatches, contextHref, songRadioHref, type ContextCard } from '$lib/contextlink';
	import { closeRight, shell } from '$lib/shell/layout.svelte';
	import IconButton from '$lib/kit/IconButton.svelte';
	import Icon from '$lib/kit/Icon.svelte';
	import Tooltip from '$lib/kit/Tooltip.svelte';
	import { playlistHref } from '$lib/kit/actions';
	import { audio, formatSleepLeft } from '$lib/audio.svelte';
	import { openNowPlayingMenu, openSleepMenu } from './menus';
	import XIcon from 'phosphor-svelte/lib/XIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import SidebarSimpleIcon from 'phosphor-svelte/lib/SidebarSimpleIcon';
	import MoonIcon from 'phosphor-svelte/lib/MoonIcon';
	import ArrowsOutSimpleIcon from 'phosphor-svelte/lib/ArrowsOutSimpleIcon';
	import { openExpanded } from './state.svelte';

	const reveal = 'group-hover/rp:opacity-100 group-focus-within/rp:opacity-100';

	const view = $derived(shell.rightView);
	// ── lane magic-d1 ── Spotify's wording: "Playing from search" over the query for a search
	// pick, the radio page for a radio played from its page, the context otherwise.
	const from = $derived(playingFrom(playlistHref));
	const contextName = $derived(from.title);
	// ── lane magic-d6 ── every context links (MAGIC-GAPS #20): the card the core records when
	// something starts (`home-changed` "played", or the latest `recent_play_contexts` after a
	// restart) knows an album, artist, Liked Songs, mix or radio the queue's bare `sourceId` can't.
	let card = $state<ContextCard | null>(null);
	$effect(() => {
		let live = true;
		api
			.recentPlayContexts(1)
			.then((r) => live && !card && r[0] && (card = r[0]))
			.catch(() => {});
		const off = api.onHomeChanged((c) => {
			if (c.reason === 'played' && c.context) card = c.context;
		});
		return () => {
			live = false;
			off.then((f) => f());
		};
	});
	const contextHrefLink = $derived.by(() => {
		if (from.label) return from.href ?? null; // "Playing from search" (or a bare single pick)
		const q = playback.queue;
		const matched = cardMatches(card, q.sourceName) ? contextHref(card!) : null;
		// A playlist or radio page's own link is exact. An album or artist played from its page has
		// its audio playlist as `sourceId`, but Spotify links the album or artist: the card wins.
		const page = matched && (card?.kind === 'album' || card?.kind === 'artist');
		if (from.href && !page) return from.href;
		return matched ?? from.href ?? songRadioHref(q.sourceName, q.items);
	});
	const sleepMode = $derived(audio.sleep.mode);

	let scrolled = $state(false);
	let sc: HTMLElement | null = null;
	// Each view opens at its top (the panel's scroller is shared between them).
	$effect(() => {
		view;
		if (sc) sc.scrollTop = 0;
	});
	function watch(el: HTMLElement) {
		sc = el.parentElement?.querySelector<HTMLElement>('[data-right-scroll]') ?? null;
		if (!sc) return;
		const s = sc;
		const on = () => (scrolled = s.scrollTop > 0);
		on();
		s.addEventListener('scroll', on, { passive: true });
		return () => s.removeEventListener('scroll', on);
	}
</script>

<div
	class="relative z-10 flex h-16 shrink-0 items-center justify-between gap-2 pr-4 pl-4 transition-shadow duration-200 {scrolled
		? 'shadow-[0_6px_10px_rgba(0,0,0,.6)]'
		: ''}"
	{@attach watch}
>
	{#if view === 'now-playing' || !view}
		<!-- Spotify (2026): "Hide Now Playing view" sits left of the title and, with More and
		     "Expand Now Playing view", only shows while the panel is hovered (or focused). -->
		<div class="-ml-2 w-0 shrink-0 overflow-hidden opacity-0 transition-[width,opacity] duration-200 {reveal} group-hover/rp:w-8 group-focus-within/rp:w-8">
			<IconButton icon={SidebarSimpleIcon} label="Hide Now Playing view" onclick={closeRight} />
		</div>
		<h2 class="min-w-0 flex-1 truncate text-base font-bold text-white">
			{#if from.label}
				<span class="block truncate text-[11px] leading-4 font-normal text-text-subdued" data-testid="playing-from-label">{from.label}</span>
			{/if}
			{#if contextHrefLink}
				<a href={contextHrefLink} class="hover:underline">{contextName}</a>
			{:else}
				{contextName}
			{/if}
		</h2>
		<div class="flex shrink-0 items-center">
			{#if sleepMode}
				<Tooltip text="Sleep timer">
					{#snippet trigger(props)}
						<button
							{...props}
							type="button"
							class="mr-1 flex h-6 items-center gap-1 rounded-full px-2 text-xs font-bold text-accent-text tabular-nums hover:bg-bg-tinted"
							onclick={(e) => openSleepMenu(e)}
						>
							<Icon icon={MoonIcon} size={12} fill />
							{sleepMode.kind === 'endOfTrack' ? 'End of track' : formatSleepLeft(audio.sleepLeft ?? 0)}
						</button>
					{/snippet}
				</Tooltip>
			{/if}
			{#if playback.now}
				<div class="flex items-center opacity-0 transition-opacity duration-200 {reveal}">
					<IconButton
						icon={DotsThreeIcon}
						label="More options for {playback.now.title}"
						tooltip="More options"
						onclick={(e) => openNowPlayingMenu(e)}
					/>
					<IconButton icon={ArrowsOutSimpleIcon} label="Expand Now Playing view" onclick={() => openExpanded('artwork')} />
				</div>
			{/if}
		</div>
	{:else}
		<h2 class="min-w-0 truncate text-base font-bold text-white">
			{view === 'queue' ? 'Queue' : 'Connect'}
		</h2>
		<IconButton icon={XIcon} label="Close" onclick={closeRight} />
	{/if}
</div>
