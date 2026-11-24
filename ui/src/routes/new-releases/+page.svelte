<script lang="ts">
	/**
	 * New releases (polish.md): header, YouTube's "New Release Mix", then the Albums & singles and
	 * Music videos shelves; "Show all" opens the full grid (`getBrowseGrid(moreBrowseId)`).
	 */
	import { onMount } from 'svelte';
	import type { NewReleasesPage } from '$lib/api';
	import { ErrorState, Shelf, Skeleton } from '$lib/kit';
	import BrowseHeader from '$lib/discover/BrowseHeader.svelte';
	import ExploreCard from '$lib/discover/ExploreCard.svelte';
	import { NEW_KEY, TILE, loadNewReleases, peek } from '$lib/discover/data';

	let data = $state<NewReleasesPage | null>(peek<NewReleasesPage>(NEW_KEY));
	let error = $state(false);
	async function load() {
		error = false;
		try {
			data = await loadNewReleases();
		} catch {
			if (!data) error = true;
		}
	}
	onMount(load);

	const listHref = (s: { title: string; moreBrowseId?: string; moreParams?: string }) =>
		s.moreBrowseId
			? `/list?id=${encodeURIComponent(s.moreBrowseId)}${s.moreParams ? `&params=${encodeURIComponent(s.moreParams)}` : ''}&title=${encodeURIComponent(s.title)}`
			: undefined;
</script>

<div class="relative isolate min-h-full pb-10">
	<BrowseHeader title="New Releases" color={TILE.new_releases} eyebrow="Browse" />
	<div class="flex flex-col gap-8 px-[var(--content-spacing)]">
		{#if error}
			<ErrorState message="Try reloading the page." onRetry={load} />
		{:else if !data}
			{#each Array(2) as _, s (s)}
				<section aria-hidden="true" class="flex flex-col gap-3">
					<Skeleton width="220px" height={20} />
					<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
				</section>
			{/each}
		{:else}
			{#if data.mix}
				<Shelf title="Made for new music" items={[data.mix]}>
					{#snippet card(i)}<ExploreCard item={i} />{/snippet}
				</Shelf>
			{/if}
			{#each data.sections as s, n (n + s.title)}
				<Shelf title={s.title} items={s.items} href={listHref(s)} rows={s.kind === 'albums' ? 2 : 1}>
					{#snippet card(i)}<ExploreCard item={i} />{/snippet}
				</Shelf>
			{/each}
		{/if}
	</div>
</div>
