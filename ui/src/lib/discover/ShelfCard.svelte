<script lang="ts">
	/** One `smartHome()` shelf item as a kit Card (home.md "Card"). Plays from the accent button,
	 *  opens on click (mixes open the mix page; songs play), right-click = the item's menu. */
	import type { ShelfItem } from '$lib/api';
	import Card from '$lib/kit/Card.svelte';
	import { openItemMenu } from '$lib/kit/menus';
	import { cardSubtitle } from './format';
	import {
		isPlaying,
		openShelfItem,
		playShelfItem,
		shelfHref,
		shelfKey,
		shelfMenuItem,
		shelfImage,
		toggle
	} from './play.svelte';

	let { item }: { item: ShelfItem } = $props();
	const key = $derived(shelfKey(item));
	const menuItem = $derived(shelfMenuItem(item));
	const href = $derived(item.kind === 'song' ? undefined : shelfHref(item));
	const subtitle = $derived(
		item.subtitle
			? cardSubtitle(item.kind, item.subtitle) || item.subtitle
			: item.kind === 'artist' ? 'Artist' : item.kind === 'album' ? 'Album' : item.kind === 'liked' ? 'Playlist' : ''
	);
</script>

<Card
	title={item.title}
	{subtitle}
	image={shelfImage(item)}
	kind={item.kind}
	{href}
	onclick={href ? undefined : () => openShelfItem(item)}
	playing={isPlaying(key, item.title)}
	onPlay={item.kind === 'mood' ? undefined : () => toggle(key, () => playShelfItem(item), item.title)}
	oncontextmenu={menuItem ? (e) => openItemMenu(e, menuItem) : undefined}
/>
