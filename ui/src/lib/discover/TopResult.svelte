<script lang="ts">
	/**
	 * Search "Top result" banner (search-and-browse.md §4.2, the current horizontal layout): full
	 * width, 104px tall, padding 16, gap 24, radius 6, white/10 → white/14 on hover; 72px image
	 * (circle for artists), name 24/700 white, type 14 #b3b3b3; at the right an outline Follow pill
	 * (artists) and the 48px accent play.
	 */
	import type { BrowseItem, ExploreItem } from '$lib/api';
	import { Pill, PlayButton, actions } from '$lib/kit';
	import { openItemMenu } from '$lib/kit/menus';
	import { thumb } from '$lib/thumb';
	import { creatorOf } from './format';
	import { exploreHref, isPlaying, openExplore, playBrowse, toggle } from './play.svelte';

	let { item, onOpen }: { item: BrowseItem | ExploreItem; onOpen?: () => void } = $props();
	const key = $derived(item.kind === 'song' ? `song:${item.id}` : item.id);
	const playing = $derived(isPlaying(key));
	const kindLabel = $derived(
		({ song: 'Song', artist: 'Artist', album: 'Album', playlist: 'Playlist', podcast: 'Podcast', episode: 'Episode' } as Record<string, string>)[item.kind] ?? ''
	);
	const type = $derived(
		item.kind === 'artist' ? kindLabel : [kindLabel, creatorOf(item.subtitle)].filter(Boolean).join(' • ')
	);
	const href = $derived(item.kind === 'song' ? undefined : exploreHref(item));
	// A show (stream: podtop) has no item menu or library toggle here; music cards do.
	const music = $derived(item.kind === 'podcast' || item.kind === 'episode' ? null : (item as BrowseItem));
	const followed = $derived(item.kind === 'artist' && !!music && actions.isInLibrary(music));
	// stream: podtop — a show's line is "Podcast • <author>"; its subtitle is the bare author.
	const line = $derived(item.kind === 'podcast' ? ['Podcast', item.subtitle].filter(Boolean).join(' • ') : type);
	function open(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('a,button')) return;
		onOpen?.();
		openExplore(item);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group flex h-[104px] cursor-pointer items-center gap-6 rounded-md bg-bg-tinted p-4 transition-colors duration-200 ease-productive hover:bg-bg-tinted-highlight"
	onclick={open}
	oncontextmenu={(e) => {
		if (!music) return;
		e.preventDefault();
		openItemMenu(e, music);
	}}
>
	<img
		src={thumb(item.thumbnail, 160)}
		alt=""
		class="size-[72px] shrink-0 bg-bg-menu object-cover shadow-card-image {item.kind === 'artist'
			? 'rounded-full'
			: 'rounded-sm'}"
	/>
	<div class="min-w-0 flex-1">
		{#if href}
			<a {href} class="block truncate font-title text-xl font-bold text-white hover:underline" onclick={() => onOpen?.()}
				>{item.title}</a
			>
		{:else}
			<span class="block truncate font-title text-xl font-bold text-white">{item.title}</span>
		{/if}
		<div class="truncate text-sm text-text-subdued">{line}</div>
	</div>
	<div class="flex shrink-0 items-center gap-6">
		{#if item.kind === 'artist'}
			<Pill variant="outline" onclick={() => music && actions.toggleInLibrary(music)}>{followed ? 'Following' : 'Follow'}</Pill>
		{/if}
		<PlayButton size="md" {playing} label={`Play ${item.title}`} onclick={() => toggle(key, () => playBrowse(item))} />
	</div>
</div>
