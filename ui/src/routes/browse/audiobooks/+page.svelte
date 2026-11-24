<script lang="ts">
	/**
	 * stream: audiobooks — Browse ▸ Audiobooks: popular and genre shelves (`audiobook_home`), or with
	 * `?genre=<subject>&title=<label>` that genre's books as a grid (`audiobook_genre`).
	 */
	import { page } from '$app/state';
	import type { Shelf as HomeShelf } from '$lib/api';
	import { EmptyState, ErrorState, Skeleton } from '$lib/kit';
	import BrowseHeader from '$lib/discover/BrowseHeader.svelte';
	import { TILE } from '$lib/discover/data';
	import { getCached, putCached } from '$lib/pagecache';
	import { audiobookGenre, audiobookHome, type Audiobook } from '$lib/audiobooks.svelte';
	import AudiobookShelves from '$lib/audiobooks/AudiobookShelves.svelte';
	import BookCard from '$lib/audiobooks/BookCard.svelte';

	const genre = $derived(page.url.searchParams.get('genre'));
	const title = $derived(page.url.searchParams.get('title') ?? 'Audiobooks');

	let shelves = $state.raw<HomeShelf[] | null>(getCached<HomeShelf[]>('audiobooks:home'));
	let grid = $state.raw<Audiobook[] | null>(null);
	let error = $state(false);

	async function load(g: string | null) {
		error = false;
		try {
			if (g) {
				grid = getCached<Audiobook[]>(`audiobooks:genre:${g}`);
				const fresh = await audiobookGenre(g);
				if (g !== genre) return;
				grid = fresh;
				putCached(`audiobooks:genre:${g}`, fresh);
			} else {
				const fresh = await audiobookHome();
				shelves = fresh;
				putCached('audiobooks:home', fresh);
			}
		} catch {
			error = true;
		}
	}
	$effect(() => {
		load(genre);
	});
</script>

<svelte:head><title>{title}</title></svelte:head>

<div class="relative isolate min-h-full pb-10">
	<BrowseHeader {title} color={TILE.audiobooks} eyebrow={genre ? 'Audiobooks' : 'Browse'} />
	<div class="flex flex-col gap-8 px-[var(--content-spacing)]">
		{#if genre}
			{#if grid?.length}
				<div class="-mx-3 grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))]">
					{#each grid as b (b.id)}<BookCard book={b} />{/each}
				</div>
			{:else if grid}
				<EmptyState title="No audiobooks here yet" body="Try another genre." />
			{:else if error}
				<ErrorState message="Couldn't load audiobooks. Check your connection and try again." onRetry={() => load(genre)} />
			{:else}
				<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={10} /></div>
			{/if}
		{:else if shelves}
			<AudiobookShelves {shelves} />
		{:else if error}
			<ErrorState message="Couldn't load audiobooks. Check your connection and try again." onRetry={() => load(null)} />
		{:else}
			{#each Array(3) as _, s (s)}
				<section aria-hidden="true" class="flex flex-col gap-3">
					<Skeleton width="220px" height={20} />
					<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
				</section>
			{/each}
		{/if}
	</div>
</div>
