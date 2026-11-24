<script lang="ts">
	/**
	 * Spotify card (Home, search, artist discography, "More by"). spec: home.md "Card".
	 * Padding 12, gap 8, radius 6, no bg until hover (#1f1f1f, 200ms). Image square, radius 6
	 * (artists: circle), card-image shadow; 48px accent play rises 8px + fades in on hover
	 * (bottom-right 8px), stays visible while `playing`.
	 *
	 * Props
	 *   title: string
	 *   subtitle?: string             14px #b3b3b3, 2-line clamp
	 *   subtitleSnippet?: Snippet     replaces subtitle (E badge, links, blue dot…)
	 *   image?: string | null         cover URL (sized via $lib/thumb to 300px)
	 *   round?: boolean               circular image (artist cards); default kind === 'artist'
	 *   kind?: string                 'artist' | 'album' | 'playlist' | 'song' | …
	 *   href?: string                 card click navigates here (title is a link too)
	 *   onclick?: (e) => void         alternative to href
	 *   playing?: boolean             this card's context is playing → pause glyph, button stays visible
	 *   onPlay?: () => void           omit to hide the play button
	 *   playLabel?: string
	 *   oncontextmenu?: (e: MouseEvent) => void   right-click (e.g. openItemMenu(e, item))
	 *   draggable?: boolean; ondragstart?: (e: DragEvent) => void
	 *   dragItem?: BrowseItem         drag payload (lane magic-d2). Without it, an album / playlist
	 *                                 card with an `/album/<id>` or `/playlist/<id>` href is a drag
	 *                                 source on its own: onto the queue, a sidebar playlist, a
	 *                                 playlist's track list or home's Shortcuts.
	 *   class?: string
	 *
	 * Usage
	 *   <Card title={i.title} subtitle={i.subtitle} image={i.thumbnail} kind={i.kind} href={hrefFor(i)}
	 *     onPlay={() => play(i)} oncontextmenu={(e) => openItemMenu(e, i)} />
	 */
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import PlayButton from './PlayButton.svelte';
	import Icon from './Icon.svelte';
	import MusicNotesIcon from 'phosphor-svelte/lib/MusicNotesIcon';
	import UserIcon from 'phosphor-svelte/lib/UserIcon';
	import { thumb } from '$lib/thumb';
	import type { BrowseItem } from '$lib/api';
	import { setDragItem } from '$lib/dnd';

	let {
		title,
		subtitle,
		subtitleSnippet,
		image,
		round,
		kind,
		href,
		onclick,
		playing = false,
		onPlay,
		playLabel,
		oncontextmenu,
		draggable = false,
		ondragstart,
		dragItem,
		class: className = ''
	}: {
		title: string;
		subtitle?: string | null;
		subtitleSnippet?: Snippet;
		image?: string | null;
		round?: boolean;
		kind?: string;
		href?: string;
		onclick?: (e: MouseEvent) => void;
		playing?: boolean;
		onPlay?: () => void;
		playLabel?: string;
		oncontextmenu?: (e: MouseEvent) => void;
		draggable?: boolean;
		ondragstart?: (e: DragEvent) => void;
		dragItem?: BrowseItem;
		class?: string;
	} = $props();

	const circle = $derived(round ?? kind === 'artist');
	// Keyed by URL rather than reset in an effect: a cached image can fire `load` before the
	// reset effect runs, which left it invisible.
	let loadedSrc = $state<string | null | undefined>(null);
	const loaded = $derived(!!image && loadedSrc === image);
	// stream: coverfix — a cover that fails to load shows the placeholder tile, not a broken image.
	let failedSrc = $state<string | null | undefined>(null);

	// Lane magic-d2: every album / playlist card drags, like on Spotify.
	const autoItem = $derived.by((): BrowseItem | undefined => {
		if (dragItem) return dragItem;
		const m = href?.match(/^\/(album|playlist)\/([^/?#]+)$/);
		if (!m) return undefined;
		return {
			kind: m[1] as BrowseItem['kind'],
			id: decodeURIComponent(m[2]),
			title,
			subtitle: subtitle ?? undefined,
			thumbnail: image ?? undefined
		} as BrowseItem;
	});
	const canDrag = $derived(draggable || !!autoItem);
	function dragStart(e: DragEvent) {
		if (ondragstart) ondragstart(e);
		else if (autoItem) setDragItem(e, autoItem);
	}

	function activate(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('a,button')) return;
		if (onclick) onclick(e);
		else if (href) goto(href);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group relative flex min-w-0 cursor-pointer flex-col gap-2 rounded-md p-3 transition-colors duration-200 ease-productive hover:bg-bg-highlight active:bg-black {className}"
	onclick={activate}
	oncontextmenu={oncontextmenu
		? (e) => {
				e.preventDefault();
				oncontextmenu(e);
			}
		: undefined}
	draggable={canDrag}
	ondragstart={canDrag ? dragStart : undefined}
>
	<div class="relative aspect-square w-full">
		<div
			class="size-full overflow-hidden bg-bg-menu shadow-card-image {circle
				? 'rounded-full'
				: 'rounded-md'}"
		>
			{#if image && failedSrc !== image}
				<img
					src={thumb(image, 300)}
					alt=""
					loading="lazy"
					draggable="false"
					class="size-full object-cover transition-opacity duration-300 {loaded
						? 'opacity-100'
						: 'opacity-0'}"
					onload={() => (loadedSrc = image)}
					onerror={() => (failedSrc = image)}
					{@attach (el: HTMLImageElement) => {
						const src = image;
						if (el.complete && el.naturalWidth) loadedSrc = src;
					}}
				/>
			{:else}
				<div class="grid size-full place-items-center text-text-subdued">
					<Icon icon={kind === 'artist' ? UserIcon : MusicNotesIcon} size={48} />
				</div>
			{/if}
		</div>
		{#if onPlay}
			<div
				class="absolute right-2 bottom-2 rounded-full shadow-play transition-[opacity,transform] duration-200 ease-out {playing
					? 'translate-y-0 opacity-100'
					: 'translate-y-2 opacity-0 group-hover:translate-y-0 group-hover:opacity-100 group-focus-within:translate-y-0 group-focus-within:opacity-100'}"
			>
				<PlayButton size="md" {playing} label={playLabel ?? `Play ${title}`} onclick={() => onPlay()} />
			</div>
		{/if}
	</div>
	<div class="min-w-0">
		{#if href}
			<a
				{href}
				data-card-primary
				class="line-clamp-2 text-base leading-[22px] text-white hover:underline"
				title={title}>{title}</a
			>
		{:else if onclick}
			<!-- a11y: an onclick-only card still needs a keyboard way in: the title is its button. -->
			<button
				type="button"
				data-card-primary
				class="line-clamp-2 text-left text-base leading-[22px] text-white hover:underline"
				title={title}
				onclick={(e) => onclick?.(e)}>{title}</button
			>
		{:else}
			<span class="line-clamp-2 text-base leading-[22px] text-white" title={title}>{title}</span>
		{/if}
		{#if subtitleSnippet}
			<div class="mt-0.5 line-clamp-2 text-sm leading-[19px] text-text-subdued">
				{@render subtitleSnippet()}
			</div>
		{:else if subtitle}
			<p class="mt-0.5 line-clamp-2 text-sm leading-[19px] text-text-subdued">{subtitle}</p>
		{/if}
	</div>
</div>
