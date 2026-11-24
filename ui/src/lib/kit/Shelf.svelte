<script lang="ts" generics="T">
	/**
	 * A titled row of cards (spec: home.md "Shelves"). The heading is 24/700 (link when `href`),
	 * "Show all" sits at the right (14/700 #b3b3b3 → white underline).
	 *
	 * Two layouts:
	 *  - `scroll` (default; spec home.md "Shelves", artist.md "Discography"): Spotify's carousel.
	 *    N = floor((width + 24) / minCard) full cards plus a 0.2-card peek, so the pitch is
	 *    (width + 24) / (N + 0.2) — 178px (154px image) at an 803px main view with the 40px Home
	 *    gutter. The rail runs out to the main panel's right edge, where the peeking card fades.
	 *    Round 32px prev/next arrows appear on shelf hover, centred on the images, and scroll by
	 *    a page; scrolling snaps to card starts.
	 *  - `fit`: as many columns as fit the width, exactly that many items (and `rows` rows) shown.
	 *    Used automatically when `rows` > 1.
	 * The first column is pulled left 12px so the first image lines up with the page gutter.
	 *
	 * Props
	 *   title: string; overline?: string; href?: string (heading link + "Show all")
	 *   showAll?: boolean (default: !!href); showAllLabel?: string ("Show all")
	 *   onShowAll?: () => void (instead of href)
	 *   items: T[]; key?: (item: T) => string
	 *   card: Snippet<[T, number]>     render one card (usually <Card …/>)
	 *   controls?: Snippet              extra row under the heading (artist Discography's chips)
	 *   mode?: 'fit' | 'scroll'; minCard?: number; rows?: number (fit mode, default 1)
	 *   columns?: number               bindable out: the computed column count
	 *
	 * Usage
	 *   <Shelf title="Recently played" href="/section/recent" items={list}>
	 *     {#snippet card(i)}<Card title={i.title} … />{/snippet}
	 *   </Shelf>
	 */
	import { scrollBehavior } from './motion';
	import type { Snippet } from 'svelte';
	import { roving } from './roving';
	import Icon from './Icon.svelte';
	import CaretLeftIcon from 'phosphor-svelte/lib/CaretLeftIcon';
	import CaretRightIcon from 'phosphor-svelte/lib/CaretRightIcon';

	let {
		title,
		overline,
		href,
		showAll,
		showAllLabel = 'Show all',
		onShowAll,
		items,
		key,
		card,
		controls,
		mode: modeProp = 'scroll',
		minCard = 180,
		rows = 1,
		columns = $bindable(5)
	}: {
		title: string;
		overline?: string;
		href?: string;
		showAll?: boolean;
		showAllLabel?: string;
		onShowAll?: () => void;
		items: T[];
		key?: (item: T) => string;
		card: Snippet<[T, number]>;
		controls?: Snippet;
		mode?: 'fit' | 'scroll';
		minCard?: number;
		rows?: number;
		columns?: number;
	} = $props();

	const mode = $derived(rows > 1 ? 'fit' : modeProp);
	let width = $state(0);
	let section = $state<HTMLElement | null>(null);
	// Row is widened by 24px (12px each side) so the card padding hangs into the gutter.
	$effect(() => {
		columns = Math.max(2, Math.floor((width + 24) / minCard));
	});
	const colWidth = $derived(
		width ? (width + 24) / (mode === 'scroll' ? columns + 0.2 : columns) : minCard
	);
	// Distance from the shelf's right edge to the main panel's right edge: the carousel bleeds
	// into it so the next card is cut by the panel, as on Spotify.
	let bleed = $state(0);
	$effect(() => {
		void width;
		if (!section) return;
		const panel = section.closest('[data-main-scroll]');
		bleed = panel
			? Math.max(0, Math.round(panel.getBoundingClientRect().right - section.getBoundingClientRect().right))
			: 0;
	});
	const shown = $derived(mode === 'fit' ? items.slice(0, columns * rows) : items);
	const withShowAll = $derived(showAll ?? !!(href || onShowAll));

	let rail = $state<HTMLElement | null>(null);
	let atStart = $state(true);
	let atEnd = $state(false);
	function edges() {
		if (!rail) return;
		atStart = rail.scrollLeft <= 1;
		atEnd = rail.scrollLeft + rail.clientWidth >= rail.scrollWidth - 1;
	}
	$effect(() => {
		void shown.length;
		void width;
		edges();
	});
	function page(dir: 1 | -1) {
		if (!rail) return;
		const step = Math.max(1, columns) * colWidth;
		rail.scrollBy({ left: dir * step, behavior: scrollBehavior() });
	}
	// a11y: ←/→ (Home/End) move between the cards' links like Spotify's shelf grids; each card
	// stays its own Tab stop (Spotify's are too).
	const CARD_KEYS = { items: '[data-card-primary]', orientation: 'horizontal', tabStop: false } as const;
</script>

<section class="group/shelf relative min-w-0" bind:this={section} bind:clientWidth={width}>
	<div class="mb-2 flex items-end justify-between gap-4">
		<div class="min-w-0">
			{#if overline}<p class="text-sm text-text-subdued">{overline}</p>{/if}
			<h2 class="truncate font-title text-xl leading-[normal] font-bold text-white">
				{#if href}<a {href} class="hover:underline">{title}</a>{:else}{title}{/if}
			</h2>
		</div>
		{#if withShowAll}
			{#if href}
				<a
					{href}
					class="shrink-0 text-sm font-bold text-text-subdued hover:text-white hover:underline"
					>{showAllLabel}</a
				>
			{:else}
				<button
					type="button"
					class="shrink-0 text-sm font-bold text-text-subdued hover:text-white hover:underline"
					onclick={() => onShowAll?.()}>{showAllLabel}</button
				>
			{/if}
		{/if}
	</div>
	{#if controls}{@render controls()}{/if}

	{#if mode === 'fit'}
		<div
			class="-mx-3 grid"
			style="grid-template-columns:repeat({columns}, minmax(0, 1fr))"
			{@attach roving(CARD_KEYS)}
		>
			{#each shown as item, i (key ? key(item) : i)}
				{@render card(item, i)}
			{/each}
		</div>
	{:else}
		<div class="relative -ml-3" style="margin-right:{-bleed}px;--col:{colWidth}px">
			<div
				bind:this={rail}
				onscroll={edges}
				class="no-scrollbar grid snap-x snap-mandatory auto-cols-[var(--col)] grid-flow-col overflow-x-auto overscroll-x-contain"
				{@attach roving(CARD_KEYS)}
				style="mask-image:linear-gradient(to right, {atStart
					? '#000'
					: 'transparent'}, #000 {atStart ? 0 : 48}px, #000 calc(100% - {atEnd
					? 0
					: bleed + colWidth * 0.2 + 12}px), {atEnd ? '#000' : 'rgb(0 0 0 / 0.4)'})"
			>
				{#each shown as item, i (key ? key(item) : i)}
					<div class="min-w-0 snap-start">{@render card(item, i)}</div>
				{/each}
			</div>
			{#if !atStart}
				<button
					type="button"
					aria-label="Go back"
					class="absolute top-[calc(var(--col)/2-16px)] left-3 z-[5] grid size-8 place-items-center rounded-full bg-bg-highlight/90 text-white opacity-0 shadow-menu transition-opacity duration-200 group-hover/shelf:opacity-100 hover:scale-[1.04] focus-visible:opacity-100"
					onclick={() => page(-1)}
				>
					<Icon icon={CaretLeftIcon} size={16} weight="bold" />
				</button>
			{/if}
			{#if !atEnd}
				<button
					type="button"
					aria-label="Show more"
					class="absolute top-[calc(var(--col)/2-16px)] z-[5] grid size-8 place-items-center rounded-full bg-bg-highlight/90 text-white opacity-0 shadow-menu transition-opacity duration-200 group-hover/shelf:opacity-100 hover:scale-[1.04] focus-visible:opacity-100"
					style="right:{Math.max(12, bleed - 8)}px"
					onclick={() => page(1)}
				>
					<Icon icon={CaretRightIcon} size={16} weight="bold" />
				</button>
			{/if}
		</div>
	{/if}
</section>
