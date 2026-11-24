<script lang="ts">
	/**
	 * stream: audiobooks — the core's audiobook shelves (`audiobook_home`): Continue listening,
	 * Your audiobooks, Popular, genre tiles, genre shelves. Home's Audiobooks chip and Browse ▸
	 * Audiobooks both render this.
	 */
	import type { Shelf as HomeShelf } from '$lib/api';
	import { Shelf } from '$lib/kit';
	import ShelfCard from '$lib/discover/ShelfCard.svelte';
	import BrowseTile from '$lib/discover/BrowseTile.svelte';
	import { genreHref } from '$lib/audiobooks.svelte';

	let { shelves }: { shelves: HomeShelf[] } = $props();

	const allHref = (s: HomeShelf) =>
		s.id === 'ab_popular' ? genreHref('popular', s.title) : s.id.startsWith('ab_genre:') ? genreHref(s.id.slice(9), s.title) : undefined;
</script>

{#each shelves as s (s.id)}
	{#if s.layout === 'moods'}
		<Shelf title={s.title} items={s.items} minCard={220}>
			{#snippet card(i)}
				<div class="p-3"><BrowseTile title={i.title} color={i.color ?? '#535353'} image={i.thumbnail} href={genreHref(i.params ?? i.id, i.title)} /></div>
			{/snippet}
		</Shelf>
	{:else}
		<Shelf title={s.title} overline={s.subtitle} items={s.items} href={allHref(s)}>
			{#snippet card(i)}<ShelfCard item={i} />{/snippet}
		</Shelf>
	{/if}
{/each}
