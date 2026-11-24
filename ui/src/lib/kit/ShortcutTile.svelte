<script lang="ts">
	/**
	 * Home's top grid tile (spec: home.md §3). 48px tall, radius 4, bg white/10 → white/14 on hover,
	 * 48x48 art at the left (square, no radius), title 14/700 2-line clamp, 32px accent play fades in
	 * at right 8px on hover; the equaliser shows instead while `playing` (not paused) and not hovered.
	 * Hovering resolves the art's tint and calls `onHoverTint(color)` (Home tints its header with it,
	 * `transition: background 1s`).
	 *
	 * Props
	 *   title: string; image?: string | null; href?: string; onclick?: (e) => void
	 *   playing?: boolean; paused?: boolean (no equaliser while paused, like Spotify)
	 *   onPlay?: () => void
	 *   onHoverTint?: (color: string) => void
	 *   oncontextmenu?: (e: MouseEvent) => void
	 *   progress?: number (0..1, podcast episodes: thin bar under the title); unplayed?: boolean (blue dot)
	 *
	 * Usage: grid of 8 — `<div class="grid grid-cols-4 gap-2">{#each picks as p}<ShortcutTile …/>{/each}</div>`
	 */
	import { goto } from '$app/navigation';
	import PlayButton from './PlayButton.svelte';
	import Equalizer from './Equalizer.svelte';
	import Icon from './Icon.svelte';
	import MusicNotesIcon from 'phosphor-svelte/lib/MusicNotesIcon';
	import { thumb } from '$lib/thumb';
	import { tintFor } from './tint.svelte';

	let {
		title,
		image,
		href,
		onclick,
		playing = false,
		paused = false,
		onPlay,
		onHoverTint,
		oncontextmenu,
		progress,
		unplayed = false
	}: {
		title: string;
		image?: string | null;
		href?: string;
		onclick?: (e: MouseEvent) => void;
		playing?: boolean;
		paused?: boolean;
		onPlay?: () => void;
		onHoverTint?: (color: string) => void;
		oncontextmenu?: (e: MouseEvent) => void;
		progress?: number;
		unplayed?: boolean;
	} = $props();

	let hover = $state(false);
	// stream: tilefix — art that fails to load (a dead signed URL, a 404) shows the placeholder,
	// never the browser's broken-image glyph. Keyed by URL so a new image gets its own try.
	let failedSrc = $state<string | null | undefined>(null);
	function enter() {
		hover = true;
		if (onHoverTint) tintFor(image).then((t) => hover && onHoverTint?.(t.color));
	}
	function activate(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('a,button')) return;
		if (onclick) onclick(e);
		else if (href) goto(href);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group relative flex h-12 min-w-0 cursor-pointer items-center overflow-hidden rounded-sm bg-bg-tinted transition-colors duration-100 hover:bg-bg-tinted-highlight"
	onmouseenter={enter}
	onmouseleave={() => (hover = false)}
	onclick={activate}
	oncontextmenu={oncontextmenu
		? (e) => {
				e.preventDefault();
				oncontextmenu(e);
			}
		: undefined}
>
	<div class="size-12 shrink-0 bg-white/20 shadow-[0_8px_24px_rgb(0_0_0/0.5)]">
		{#if image && failedSrc !== image}
			<img
				src={thumb(image, 96)}
				alt=""
				draggable="false"
				class="size-full object-cover"
				onerror={() => (failedSrc = image)}
			/>
		{:else}
			<div class="grid size-full place-items-center text-text-subdued">
				<Icon icon={MusicNotesIcon} size={24} />
			</div>
		{/if}
	</div>
	<div class="flex min-w-0 flex-1 flex-col justify-center gap-1 pr-12 pl-2">
		<!-- a11y: the title is the tile's keyboard entry point (open), like Card's. -->
		{#if href}
			<a {href} data-card-primary class="line-clamp-2 text-sm leading-4 font-bold text-white focus-visible:outline-offset-2">{title}</a>
		{:else if onclick}
			<button
				type="button"
				data-card-primary
				class="line-clamp-2 text-left text-sm leading-4 font-bold text-white"
				onclick={(e) => onclick?.(e)}>{title}</button
			>
		{:else}
			<span class="line-clamp-2 text-sm leading-4 font-bold text-white">{title}</span>
		{/if}
		{#if progress !== undefined}
			<span class="block h-[3px] w-full max-w-[100px] overflow-hidden rounded-full bg-white/30">
				<span class="block h-full bg-white" style="width:{Math.round(progress * 100)}%"></span>
			</span>
		{/if}
	</div>
	<div class="absolute right-2 grid size-8 place-items-center">
		{#if onPlay}
			<div
				class="col-start-1 row-start-1 transition-opacity duration-200 {hover
					? 'opacity-100'
					: 'pointer-events-none opacity-0'} group-focus-within:pointer-events-auto group-focus-within:opacity-100"
			>
				<PlayButton size="sm" playing={playing && !paused} onclick={() => onPlay()} />
			</div>
		{/if}
		{#if playing && !paused && !hover}
			<span class="col-start-1 row-start-1"><Equalizer /></span>
		{:else if unplayed && !hover}
			<span class="col-start-1 row-start-1 size-2 rounded-full bg-announcement"></span>
		{/if}
	</div>
</div>
