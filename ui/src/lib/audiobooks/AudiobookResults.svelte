<script lang="ts">
	/**
	 * stream: audiobooks — search results for books (`audiobook_search`): the All view's
	 * "Audiobooks" shelf (`shelf`), or the Audiobooks chip's grid. Empty renders nothing in a shelf.
	 */
	import { EmptyState, Shelf, Skeleton } from '$lib/kit';
	import { getCached, putCached } from '$lib/pagecache';
	import { audiobookSearch, type Audiobook } from '$lib/audiobooks.svelte';
	import BookCard from './BookCard.svelte';

	let { query, shelf = false, onShowAll }: { query: string; shelf?: boolean; onShowAll?: () => void } = $props();

	let rows = $state.raw<Audiobook[] | null>(null);
	let failed = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	$effect(() => {
		const q = query.trim();
		clearTimeout(timer);
		failed = false;
		rows = getCached<Audiobook[]>(`search:books:${q}`);
		if (!q || rows) return;
		timer = setTimeout(async () => {
			try {
				const r = await audiobookSearch(q);
				if (q !== query.trim()) return;
				rows = r;
				putCached(`search:books:${q}`, r);
			} catch {
				if (q === query.trim()) failed = true;
			}
		}, 300);
		return () => clearTimeout(timer);
	});
</script>

{#if shelf}
	{#if rows?.length}
		<Shelf title="Audiobooks" items={rows} {onShowAll}>
			{#snippet card(b)}<BookCard book={b} />{/snippet}
		</Shelf>
	{/if}
{:else if rows?.length}
	<div class="-mx-3 grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))]">
		{#each rows as b (b.id)}<BookCard book={b} />{/each}
	</div>
{:else if rows || failed}
	<EmptyState
		title={failed ? "Couldn't search audiobooks" : `No audiobooks found for "${query}"`}
		body={failed ? 'Check your connection and try again.' : 'Audiobooks here are free public-domain recordings from LibriVox. Try a classic title or author.'}
	/>
{:else}
	<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={10} /></div>
{/if}
