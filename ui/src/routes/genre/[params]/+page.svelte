<script lang="ts">
	/**
	 * Genre / mood page (search-and-browse.md §5 "Click → genre/category page"): header in the
	 * tile's colour, then one shelf per YouTube section (`getBrowseSections`, polish.md).
	 * URL: /genre/<params>?title=<tile title>&color=<#hex>.
	 */
	import { untrack } from 'svelte';
	import { page } from '$app/state';
	import type { SectionsPage } from '$lib/api';
	import { ErrorState, Shelf, Skeleton, EmptyState } from '$lib/kit';
	import BrowseHeader from '$lib/discover/BrowseHeader.svelte';
	import ExploreCard from '$lib/discover/ExploreCard.svelte';
	import { loadGenre, peek, sectionsKey } from '$lib/discover/data';

	const params = $derived(decodeURIComponent(page.params.params ?? ''));
	const color = $derived(page.url.searchParams.get('color'));
	let data = $state<SectionsPage | null>(null);
	let error = $state(false);
	const title = $derived(page.url.searchParams.get('title') ?? data?.title ?? 'Browse');

	async function load(p: string) {
		data = peek<SectionsPage>(sectionsKey(p));
		error = false;
		try {
			const d = await loadGenre(p);
			if (p === params) data = d;
		} catch {
			if (p === params && !data) error = true;
		}
	}
	$effect(() => {
		const p = params;
		if (p) untrack(() => load(p));
	});

	const listHref = (s: { title: string; moreBrowseId?: string; moreParams?: string }) =>
		s.moreBrowseId
			? `/list?id=${encodeURIComponent(s.moreBrowseId)}${s.moreParams ? `&params=${encodeURIComponent(s.moreParams)}` : ''}&title=${encodeURIComponent(s.title)}`
			: `/list?genre=${encodeURIComponent(params)}&section=${encodeURIComponent(s.title)}&title=${encodeURIComponent(s.title)}`;
</script>

<div class="relative isolate min-h-full pb-10">
	<BrowseHeader {title} {color} hero />
	<div class="flex flex-col gap-8 px-[var(--content-spacing)] pt-[76px]">
		{#if error}
			<ErrorState message="Try reloading the page." onRetry={() => load(params)} />
		{:else if !data}
			{#each Array(3) as _, s (s)}
				<section aria-hidden="true" class="flex flex-col gap-3">
					<Skeleton width="220px" height={20} />
					<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
				</section>
			{/each}
		{:else if !data.sections.length}
			<EmptyState title="Nothing here right now" body="Try another category from Browse all." />
		{:else}
			{#each data.sections as s, i (i + s.title)}
				<Shelf title={s.title} items={s.items} href={listHref(s)}>
					{#snippet card(x)}<ExploreCard item={x} />{/snippet}
				</Shelf>
			{/each}
		{/if}
	</div>
</div>
