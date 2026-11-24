<script lang="ts">
	import { untrack } from 'svelte';
	/**
	 * Charts (polish.md "Charts"): header + country picker, featured chart playlists
	 * (Top songs / Top videos / Trending), then YouTube's chart shelves: video charts and
	 * per-genre Top 50s as cards, Top artists as a ranked list with trend arrows, Top podcasts.
	 */
	import type { ChartEntry, ChartsPage, ExploreItem } from '$lib/api';
	import { ErrorState, Shelf, Skeleton, Pill, Card } from '$lib/kit';
	import Icon from '$lib/kit/Icon.svelte';
	import { openMenu } from '$lib/kit/menus';
	import { thumb } from '$lib/thumb';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';
	import CaretUpIcon from 'phosphor-svelte/lib/CaretUpIcon';
	import GlobeIcon from 'phosphor-svelte/lib/GlobeIcon';
	import MinusIcon from 'phosphor-svelte/lib/MinusIcon';
	import BrowseHeader from '$lib/discover/BrowseHeader.svelte';
	import ExploreCard from '$lib/discover/ExploreCard.svelte';
	import { TILE, chartsKey, loadCharts, peek } from '$lib/discover/data';
	import { exploreHref, isPlaying, playBrowse, toggle } from '$lib/discover/play.svelte';

	let country = $state<string | undefined>(undefined);
	let data = $state<ChartsPage | null>(peek<ChartsPage>(chartsKey()));
	let error = $state(false);
	let switching = $state(false);

	async function load(c?: string) {
		error = false;
		switching = !!data;
		try {
			const d = await loadCharts(c);
			if (c === country) data = d;
		} catch {
			if (c === country && !data) error = true;
		} finally {
			if (c === country) switching = false;
		}
	}
	$effect(() => {
		const c = country;
		untrack(() => load(c));
	});

	function pickCountry(e: MouseEvent) {
		if (!data) return;
		openMenu(
			e,
			data.countries.map((c) => ({
				label: c.name,
				checked: c.code === data?.country?.code,
				onSelect: () => (country = c.code)
			})),
			{ width: 240 }
		);
	}

	const featured = $derived(
		data
			? ([
					data.highlights.topSongs,
					data.highlights.topVideos,
					data.highlights.trending
				].filter(Boolean) as ExploreItem[])
			: []
	);
	const ranked = $derived(data?.sections.filter((s) => s.kind === 'top_artists') ?? []);
	const shelves = $derived(data?.sections.filter((s) => s.kind !== 'top_artists') ?? []);
</script>

{#snippet trend(t: ChartEntry['trend'])}
	{#if t === 'up'}<Icon icon={CaretUpIcon} size={16} fill class="text-text-positive" label="Up" />
	{:else if t === 'down'}<Icon icon={CaretDownIcon} size={16} fill class="text-text-negative" label="Down" />
	{:else if t === 'new'}<span class="text-[10px] font-bold text-accent-text">NEW</span>
	{:else}<Icon icon={MinusIcon} size={12} class="text-text-subdued" label="No change" />{/if}
{/snippet}

<div class="relative isolate min-h-full pb-10">
	<BrowseHeader title="Charts" color={TILE.charts} eyebrow="Browse">
		{#if data?.countries.length}
			<Pill variant="outline" icon={GlobeIcon} onclick={pickCountry}>{data.country?.name ?? 'Global'}</Pill>
		{/if}
	</BrowseHeader>
	<div class="flex flex-col gap-8 px-[var(--content-spacing)] transition-opacity {switching ? 'opacity-60' : ''}">
		{#if error}
			<ErrorState message="Try reloading the page." onRetry={() => load(country)} />
		{:else if !data}
			{#each Array(3) as _, s (s)}
				<section aria-hidden="true" class="flex flex-col gap-3">
					<Skeleton width="220px" height={20} />
					<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
				</section>
			{/each}
		{:else}
			{#if featured.length}
				<Shelf title="Featured charts" items={featured}>
					{#snippet card(i)}<ExploreCard item={i} />{/snippet}
				</Shelf>
			{/if}
			{#each ranked as sec, r (r)}
				<section>
					<h2 class="mb-2 font-title text-xl font-bold text-white">{sec.title}</h2>
					<div class="ranked -mx-2 grid gap-x-6">
						{#each sec.items.slice(0, 20) as a, i (i + ':' + a.id)}
							{@const href = exploreHref(a)}
							<a
								{href}
								class="group flex h-16 items-center gap-3 rounded-sm px-2 hover:bg-bg-tinted"
							>
								<span class="w-6 shrink-0 text-right text-base text-text-subdued tabular-nums">{a.rank ?? i + 1}</span>
								<span class="grid w-6 shrink-0 place-items-center">{@render trend(a.trend)}</span>
								<img src={thumb(a.thumbnail, 96)} alt="" class="size-12 shrink-0 rounded-full bg-bg-menu object-cover" />
								<span class="min-w-0 flex-1">
									<span class="block truncate text-base text-white">{a.title}</span>
									{#if a.subtitle}<span class="block truncate text-sm text-text-subdued">{a.subtitle}</span>{/if}
								</span>
							</a>
						{/each}
					</div>
				</section>
			{/each}
			{#each shelves as sec, n (n + sec.title)}
				<Shelf title={sec.title} items={sec.items}>
					{#snippet card(i)}
						{#if sec.kind === 'top_podcasts'}
							<Card
								title={i.title}
								subtitle={`#${i.rank ?? ''} ${i.subtitle ?? ''}`.trim()}
								image={i.thumbnail}
								kind="podcast"
								href={exploreHref(i)}
								playing={isPlaying(i.id)}
								onPlay={() => toggle(i.id, () => playBrowse(i))}
							/>
						{:else}
							<ExploreCard item={i} />
						{/if}
					{/snippet}
				</Shelf>
			{/each}
		{/if}
	</div>
</div>

<style>
	.ranked {
		grid-template-columns: repeat(2, minmax(0, 1fr));
		grid-template-rows: repeat(10, auto);
		grid-auto-flow: column;
	}
	@container (max-width: 640px) {
		.ranked {
			grid-template-columns: minmax(0, 1fr);
			grid-template-rows: none;
			grid-auto-flow: row;
		}
	}
</style>
