<script lang="ts">
	/**
	 * "Show all" grid page (home.md shelves → full section). Sources, by query param:
	 *  - id (+params): a YouTube browse grid (`getBrowseGrid`) — new releases, artist "See all"…
	 *  - home=<shelf id>: a `smartHome()` shelf (`pod_*`: a `podcastHome()` shelf)
	 *  - podcat=<genre>: a podcast category's shows (`podcastCategory`)
	 *  - genre=<params>&section=<title>: one shelf of a genre page
	 *  - podcasts=shows|new|continue: the podcasts store lists
	 * Always: `title`.
	 */
	import { untrack } from 'svelte';
	import { page } from '$app/state';
	import * as api from '$lib/api';
	import type { BrowseItem, PodcastEpisode, PodcastShow, ShelfItem } from '$lib/api';
	import { Card, EmptyState, ErrorState, Skeleton } from '$lib/kit';
	import { showHref } from '$lib/kit/actions';
	import { podcasts } from '$lib/podcasts.svelte';
	import { getCached, putCached } from '$lib/pagecache';
	import { reveal } from '$lib/reveal.svelte';
	import ExploreCard from '$lib/discover/ExploreCard.svelte';
	import ShelfCard from '$lib/discover/ShelfCard.svelte';
	import EpisodeCard from '$lib/discover/EpisodeCard.svelte';
	import { loadGenre, loadHome, loadPodcastCategory, loadPodcastHome } from '$lib/discover/data';
	import { isPlaying, playBrowse, toggle } from '$lib/discover/play.svelte';

	type Data =
		| { kind: 'browse'; items: BrowseItem[] }
		| { kind: 'shelf'; items: ShelfItem[] }
		| { kind: 'shows'; items: PodcastShow[] }
		| { kind: 'episodes'; items: PodcastEpisode[] };

	const sp = $derived(page.url.searchParams);
	const title = $derived(sp.get('title') ?? 'Show all');
	const pod = $derived(sp.get('podcasts'));
	let data = $state<Data | null>(null);
	let error = $state(false);
	const rv = reveal();

	// Podcast lists come straight from the live store.
	const podData = $derived<Data | null>(
		pod === 'shows'
			? { kind: 'shows', items: podcasts.follows }
			: pod === 'new'
				? { kind: 'episodes', items: podcasts.newEpisodes }
				: pod === 'continue'
					? { kind: 'episodes', items: podcasts.continueListening }
					: null
	);
	const shown = $derived(podData ?? data);

	async function load(url: URL) {
		const p = url.searchParams;
		error = false;
		data = null;
		rv.reset();
		const stale = () => url.href !== page.url.href;
		try {
			if (p.get('podcat')) {
				const items = await loadPodcastCategory(p.get('podcat')!);
				if (!stale()) data = { kind: 'shelf', items };
			} else if (p.get('home')) {
				const id = p.get('home')!;
				const shelves = id.startsWith('pod_') ? await loadPodcastHome() : await loadHome();
				const s = shelves.find((x) => x.id === p.get('home'));
				if (!stale()) data = { kind: 'shelf', items: s?.items ?? [] };
			} else if (p.get('genre')) {
				const g = await loadGenre(p.get('genre')!);
				const s = g.sections.find((x) => x.title === p.get('section'));
				if (!stale()) data = { kind: 'browse', items: (s?.items ?? []) as BrowseItem[] };
			} else if (p.get('id')) {
				const id = p.get('id')!;
				const params = p.get('params') ?? undefined;
				const key = `list:${id}:${params ?? ''}`;
				const items = getCached<BrowseItem[]>(key) ?? (await api.getBrowseGrid(id, params));
				putCached(key, items);
				if (!stale()) data = { kind: 'browse', items };
			}
		} catch {
			if (!stale()) error = true;
		}
	}
	$effect(() => {
		const url = page.url;
		if (!pod) untrack(() => load(url));
	});
</script>

<div class="px-[var(--content-spacing)] pt-6 pb-10">
	<h1 class="mb-4 font-title text-xl font-bold text-white">{title}</h1>
	{#if error}
		<ErrorState message="Try reloading the page." onRetry={() => load(page.url)} />
	{:else if !shown}
		<div class="card-grid -mx-3 grid"><Skeleton variant="card" count={12} /></div>
	{:else if !shown.items.length}
		<EmptyState title="Nothing here yet" body="Check back later." />
	{:else}
		<div class="card-grid -mx-3 grid">
			{#if shown.kind === 'browse'}
				{#each shown.items.slice(0, rv.count(shown.items.length)) as i, __i (__i + ':' + i.kind + i.id)}<ExploreCard item={i} />{/each}
			{:else if shown.kind === 'shelf'}
				{#each shown.items as i, __i (__i + ':' + i.kind + i.id)}<ShelfCard item={i} />{/each}
			{:else if shown.kind === 'shows'}
				{#each shown.items as s, __i (__i + ':' + s.id)}
					{@const it = { kind: 'podcast' as const, id: s.id, title: s.title, subtitle: s.author, thumbnail: s.thumbnail ?? undefined }}
					<Card title={s.title} subtitle={s.author} image={s.thumbnail} kind="podcast" href={showHref(s.id)}
						playing={isPlaying(s.id)} onPlay={() => toggle(s.id, () => playBrowse(it))} />
				{/each}
			{:else}
				{#each shown.items as e, __i (__i + ':' + e.id)}<EpisodeCard episode={e} list={shown.items} />{/each}
			{/if}
		</div>
		{#if shown.kind === 'browse' && rv.more(shown.items.length)}<div {@attach rv.sentinel}></div>{/if}
	{/if}
</div>

<style>
	.card-grid {
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
	}
</style>
