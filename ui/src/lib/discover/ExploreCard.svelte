<script lang="ts">
	/** A BrowseItem / ExploreItem (search, charts, new releases, genre pages) as a kit Card. */
	import type { BrowseItem, ExploreItem } from '$lib/api';
	import Card from '$lib/kit/Card.svelte';
	import { openItemMenu } from '$lib/kit/menus';
	import { cardSubtitle } from './format';
	import { exploreHref, isPlaying, openExplore, playBrowse, toggle } from './play.svelte';

	let {
		item,
		subtitle,
		onOpen
	}: { item: BrowseItem | ExploreItem; subtitle?: string; onOpen?: () => void } = $props();
	const href = $derived(item.kind === 'song' ? undefined : exploreHref(item));
	const key = $derived(item.kind === 'song' ? `song:${item.id}` : item.id);
	const fallback: Record<string, string> = {
		artist: 'Artist',
		album: 'Album',
		playlist: 'Playlist',
		podcast: 'Podcast',
		episode: 'Episode',
		song: 'Song'
	};
</script>

<Card
	title={item.title}
	subtitle={subtitle ?? ((item.subtitle && cardSubtitle(item.kind, item.subtitle)) || fallback[item.kind])}
	image={item.thumbnail}
	kind={item.kind}
	{href}
	onclick={() => {
		onOpen?.();
		openExplore(item);
	}}
	playing={isPlaying(key)}
	onPlay={item.kind === 'episode' ? undefined : () => toggle(key, () => playBrowse(item))}
	oncontextmenu={item.kind === 'podcast' || item.kind === 'episode'
		? undefined
		: (e) => openItemMenu(e, item as BrowseItem)}
/>
