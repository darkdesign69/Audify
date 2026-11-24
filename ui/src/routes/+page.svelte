<script lang="ts">
	/**
	 * Home (spotify-spec/screens/home.md): tinted header wash that follows the hovered shortcut,
	 * sticky All / Music / Podcasts chips, the 8-tile shortcuts grid, then shelves from
	 * `smartHome()` (smart.md) as kit Shelves in fit mode. Greeting-less, like current Spotify.
	 */
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import type { PodcastShow, Shelf as HomeShelf, ShelfItem } from '$lib/api';
	import { Card, ErrorState, Shelf, ShortcutTile, Skeleton, NEUTRAL_TINT } from '$lib/kit';
	import { roving } from '$lib/kit/roving';
	// a11y: arrow keys move between the shortcut tiles (a 2-4 column grid); each stays a Tab stop.
	const TILE_KEYS = { items: '[data-card-primary]', orientation: 'grid', tabStop: false } as const;
	import { openItemMenu } from '$lib/kit/menus';
	import { showHref } from '$lib/kit/actions';
	import { playback } from '$lib/player.svelte';
	import { podcasts } from '$lib/podcasts.svelte';
	import ShelfCard from '$lib/discover/ShelfCard.svelte';
	import BrowseTile from '$lib/discover/BrowseTile.svelte';
	import DevicesHint from '$lib/settings/devices/DevicesHint.svelte';
	import {
		HOME_KEY,
		HOME_STORE,
		PODS_KEY,
		loadHome,
		loadPodcastHome,
		peek,
		peekHome,
		podcastCategoryHref
	} from '$lib/discover/data';
	import { titled } from '$lib/discover/format';
	import { putCached } from '$lib/pagecache';
	import { contextTile, mergeLocal, promoteShortcut } from '$lib/homelive';
	import {
		isCurrent,
		isPlaying,
		moodHref,
		openShelfItem,
		playBrowse,
		playShelfItem,
		shelfHref,
		shelfKey,
		shelfMenuItem,
		shelfImage,
		toggle
	} from '$lib/discover/play.svelte';
	import * as api from '$lib/api';
	import { audiobookHome } from '$lib/audiobooks.svelte'; // stream: audiobooks
	import AudiobookShelves from '$lib/audiobooks/AudiobookShelves.svelte';

	type Filter = 'all' | 'music' | 'podcasts' | 'audiobooks';
	let filter = $state<Filter>('all');

	const initial = peekHome();
	let shelves = $state<HomeShelf[] | null>(initial);
	let loading = $state(!initial);
	let error = $state<string | null>(null);

	async function load(fresh = false) {
		error = null;
		if (!shelves) loading = true;
		try {
			shelves = await loadHome(fresh);
		} catch (e) {
			if (!shelves) error = String(e);
		} finally {
			loading = false;
		}
	}
	// ── stream: magic-d4 ── Home moves the moment you play (MAGIC-GAPS #2).
	/** Keep the page cache and the next launch's head start in step with what's on screen. */
	function remember(v: HomeShelf[]) {
		const plain = $state.snapshot(v) as HomeShelf[];
		putCached(HOME_KEY, plain);
		try {
			localStorage.setItem(HOME_STORE, JSON.stringify(plain));
		} catch {
			/* storage unavailable: only the next launch's head start is lost */
		}
	}
	let localSeq = 0;
	/** The local shelves only (no network, a few ms): the grid, recently played, jump back in. */
	async function refreshLocal() {
		// Offline Home is the core's (downloads only, `offline_home`): rebuild it whole.
		if (notice) return void load(true);
		const seq = ++localSeq;
		try {
			const local = titled(await api.smartHomeLocal());
			if (seq !== localSeq || !shelves) return;
			shelves = mergeLocal(shelves, local);
			remember(shelves);
		} catch {
			/* the full load still revalidates */
		}
	}
	// stream: home-tiles-pods — the Podcasts chip: the core's podcast shelves (smart::podhome).
	let podShelves = $state<HomeShelf[] | null>(peek<HomeShelf[]>(PODS_KEY) ?? null);
	async function loadPods(fresh = false) {
		try {
			podShelves = await loadPodcastHome(fresh);
		} catch {
			/* keep what's on screen */
		}
	}
	$effect(() => {
		if (filter === 'podcasts') untrack(() => loadPods(!!podShelves));
	});

	// stream: audiobooks — the Audiobooks chip: the core's audiobook shelves.
	let bookShelves = $state<HomeShelf[] | null>(peek<HomeShelf[]>('audiobooks:home') ?? null);
	function loadBooks() {
		audiobookHome()
			.then((v) => {
				bookShelves = v;
				putCached('audiobooks:home', v);
			})
			.catch(() => (bookShelves ??= []));
	}
	$effect(() => {
		if (filter === 'audiobooks') untrack(loadBooks);
	});

	function onHomeChange(c: api.HomeChange) {
		if (filter === 'podcasts') void loadPods(true);
		if (c.reason === 'podcasts') return; // only the Podcasts chip's suggestions changed
		if (c.reason !== 'played') return void load(true);
		const tile = contextTile(c.context);
		if (tile && shelves) shelves = promoteShortcut(shelves, tile as ShelfItem);
		void refreshLocal();
	}
	// ── end stream: magic-d4 ──

	onMount(() => {
		// Cached: paint it, fix the local shelves at once (something may have played since), then
		// revalidate the whole Home quietly.
		if (shelves) void refreshLocal();
		load(!!shelves);
		// ── stream: smart2 ── Made for you rebuilt (schedule, import): repaint its shelf.
		let un: (() => void) | undefined;
		void api.onMadeForYouChanged(() => load(true)).then((u) => (un = u));
		// stream: perf — YouTube's shelves that missed Home's budget arrive later.
		let unHome: (() => void) | undefined;
		// stream: magic-d4 — and a context just started moves to the first tile (`played`).
		void api.onHomeChanged(onHomeChange).then((u) => (unHome = u));
		return () => {
			un?.();
			unHome?.();
		};
	});

	// ── header tint (follows the hovered shortcut, 1s) ──
	let tint = $state(NEUTRAL_TINT);

	// ── sticky chip bar: its tinted backing fades in once content scrolls under it ──
	let stuck = $state(false);
	function watchScroll(node: HTMLElement) {
		const el = node.closest<HTMLElement>('[data-main-scroll]');
		if (!el) return;
		const on = () => (stuck = el.scrollTop > 0);
		on();
		el.addEventListener('scroll', on, { passive: true });
		return () => el.removeEventListener('scroll', on);
	}

	// ── what each filter shows ──
	const isPodcastItem = (i: ShelfItem) => i.kind === 'podcast' || i.kind === 'episode';
	const shortcuts = $derived(shelves?.find((s) => s.layout === 'shortcuts')?.items.slice(0, 8) ?? []);
	/** Offline: the core's "You're offline" notice heads Home. */
	const notice = $derived(shelves?.find((s) => s.layout === 'notice'));
	const feed = $derived.by(() => {
		const list = (shelves ?? []).filter((s) => s.layout !== 'shortcuts' && s.items.length);
		if (filter === 'music')
			return list
				.map((s) => ({ ...s, items: s.items.filter((i) => !isPodcastItem(i)) }))
				.filter((s) => s.items.length);
		return list;
	});

	// Your shows, also on the All feed (podcasts store).
	const shows = $derived(podcasts.follows);

	/** Play an episode tile, with its show as the context (Recents and Home record the show). */
	const playEpisodeTile = (item: ShelfItem) => playShelfItem(item);

	// Made for you has its own page (the iPhone's Show all opens it too); the rest list the shelf.
	const showAllHref = (s: HomeShelf) =>
		s.id === 'made_for_you'
			? '/made-for-you'
			: `/list?home=${encodeURIComponent(s.id)}&title=${encodeURIComponent(s.title)}`;

	const showItem = (s: PodcastShow) => ({
		kind: 'podcast' as const,
		id: s.id,
		title: s.title,
		subtitle: s.author,
		thumbnail: s.thumbnail ?? undefined
	});
</script>

<div class="relative isolate min-h-full pb-10" {@attach watchScroll}>
	<h1 class="visually-hidden">Home</h1>
	<!-- 1. Header colour wash: tint + gradient to #121212 + noise. -->
	<div
		class="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[256px] rounded-t-lg transition-[background-color] duration-1000 ease-productive"
		style="background-color:{tint}"
	>
		<div class="absolute inset-0 bg-[linear-gradient(rgba(0,0,0,.6)_0%,#121212_100%)]"></div>
		<div class="noise absolute inset-0"></div>
	</div>

	<!-- 2. Sticky chips -->
	<div class="sticky top-0 z-[2] h-16 px-[var(--spacing-home-gutter)] pt-3">
		<div
			class="absolute inset-0 transition-opacity duration-200 {stuck ? 'opacity-100' : 'opacity-0'}"
			style="background-color:{tint}"
		>
			<div class="absolute inset-0 bg-black/60"></div>
		</div>
		<div class="relative flex h-12 items-center gap-2 overflow-x-auto [scrollbar-width:none]" role="group" aria-label="Filter Home">
			<!-- Every chip always shows; the picked one is filled, picking it again (or All) is All. -->
			<button type="button" class="chip" aria-pressed={filter === 'all'} onclick={() => (filter = 'all')}>All</button>
			<button type="button" class="chip" aria-pressed={filter === 'music'} onclick={() => (filter = filter === 'music' ? 'all' : 'music')}>Music</button>
			<button type="button" class="chip" aria-pressed={filter === 'podcasts'} onclick={() => (filter = filter === 'podcasts' ? 'all' : 'podcasts')}>Podcasts</button>
			<button type="button" class="chip" aria-pressed={filter === 'audiobooks'} onclick={() => (filter = filter === 'audiobooks' ? 'all' : 'audiobooks')}>Audiobooks</button>
		</div>
	</div>

	<div class="flex flex-col gap-8 px-[var(--spacing-home-gutter)] pt-1">
		<DevicesHint />
		{#if filter === 'audiobooks'}
			<!-- ── stream: audiobooks — Audiobooks filter: continue, your books, popular, genres ── -->
			{#if !bookShelves}
				{#each Array(3) as _, s (s)}
					<section aria-hidden="true" class="flex flex-col gap-3">
						<Skeleton width="200px" height={20} />
						<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
					</section>
				{/each}
			{:else if !bookShelves.length}
				<ErrorState message="Couldn't load audiobooks. Check your connection and try again." onRetry={() => ((bookShelves = null), loadBooks())} />
			{:else}
				<AudiobookShelves shelves={bookShelves} />
			{/if}
		{:else if filter === 'podcasts'}
			<!-- ── Podcasts filter: the core's shelves (shortcuts, new episodes, suggestions, categories) ── -->
			{#if !podShelves}
				{#each Array(3) as _, s (s)}
					<section aria-hidden="true" class="flex flex-col gap-3">
						<Skeleton width="200px" height={20} />
						<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
					</section>
				{/each}
			{:else}
				{#each podShelves as s, idx (idx + ':' + s.id)}
					{#if s.layout === 'shortcuts'}
						<div class="shortcuts mb-[19px] grid gap-2" {@attach roving(TILE_KEYS)}>
							{#each s.items as item, __i (__i + ':' + item.id)}{@render episodeTile(item)}{/each}
						</div>
					{:else}
						{@render homeShelf(s)}
					{/if}
				{/each}
			{/if}
		{:else if loading}
			<!-- ── loading skeletons ── -->
			<div class="shortcuts mb-[19px] grid gap-2" aria-hidden="true">
				{#each Array(8) as _, i (i)}
					<div class="sk h-12 rounded-sm bg-bg-tinted"></div>
				{/each}
			</div>
			{#each Array(3) as _, s (s)}
				<section aria-hidden="true" class="flex flex-col gap-3">
					<Skeleton width="200px" height={20} />
					<div class="-mx-3 grid grid-cols-5 overflow-hidden">
						<Skeleton variant="card" count={5} />
					</div>
				</section>
			{/each}
		{:else if error}
			<ErrorState message="Try reloading the page." onRetry={() => load(true)} />
		{:else}
			{#if notice}
				<div class="flex flex-col gap-1" data-testid="home-offline">
					<h2 class="font-title text-base font-bold text-white">{notice.title}</h2>
					{#if notice.subtitle}<p class="text-sm text-text-subdued">{notice.subtitle}</p>{/if}
				</div>
			{/if}
			<!-- ── 3. shortcuts ── -->
			{#if shortcuts.length && filter === 'all'}
				<div class="shortcuts mb-[19px] grid gap-2" {@attach roving(TILE_KEYS)}>
					{#each shortcuts as item, __i (__i + ':' + item.kind + item.id)}
						{@const key = shelfKey(item)}
						{@const mi = shelfMenuItem(item)}
						{#if item.kind === 'episode'}
							{@render episodeTile(item)}
						{:else}
						<ShortcutTile
							title={item.title}
							image={shelfImage(item)}
							href={shelfHref(item)}
							onclick={item.kind === 'song' ? () => openShelfItem(item) : undefined}
							playing={isCurrent(key, item.title)}
							paused={playback.paused}
							onPlay={() => toggle(key, () => playShelfItem(item), item.title)}
							onHoverTint={(c) => (tint = c)}
							oncontextmenu={mi ? (e) => openItemMenu(e, mi) : undefined}
						/>
						{/if}
					{/each}
				</div>
			{/if}

			<!-- ── 4. shelves ── -->
			{#each feed as s, idx (idx + ':' + s.id)}
				{@render homeShelf(s)}
				{#if idx === 1 && filter === 'all' && shows.length}
					<Shelf title="Your shows" items={shows} href="/list?podcasts=shows&title=Your%20shows">
						{#snippet card(x)}
							{@const it = showItem(x)}
							<Card
								title={x.title}
								subtitle={x.author}
								image={x.thumbnail}
								kind="podcast"
								href={showHref(x.id)}
								playing={isPlaying(x.id)}
								onPlay={() => toggle(x.id, () => playBrowse(it))}
							/>
						{/snippet}
					</Shelf>
				{/if}
			{/each}

			{#if !feed.length && !shortcuts.length && !notice}
				<div class="flex flex-col items-center gap-4 py-16 text-center">
					<h2 class="font-title text-xl font-bold text-white">Nothing here yet</h2>
					<p class="max-w-[400px] text-base text-text-subdued">
						Play something and Home fills up with your mixes, recents and new releases.
					</p>
					<button type="button" class="btn-white" onclick={() => goto('/search')}>Browse all</button>
				</div>
			{/if}
		{/if}
	</div>
</div>

{#snippet episodeTile(item: ShelfItem)}
	<!-- stream: magic-d4 — an episode tile: progress bar or new-episode dot (#11). -->
	{@const p = podcasts.progress[item.id]}
	{@const frac = p ? (p.played ? undefined : p.positionSecs > 0 ? p.fraction : undefined) : item.progress}
	<ShortcutTile
		title={item.title}
		image={item.thumbnail}
		href={`/episode/${encodeURIComponent(item.id)}`}
		playing={playback.now?.videoId === item.id}
		paused={playback.paused}
		progress={frac}
		unplayed={frac === undefined && !!item.newEpisode && !p?.played}
		onPlay={() => (playback.now?.videoId === item.id ? api.togglePause() : playEpisodeTile(item))}
		onHoverTint={(c) => (tint = c)}
	/>
{/snippet}

{#snippet homeShelf(s: HomeShelf)}
	{#if s.layout === 'moods'}
		<!-- Moods & genres / podcast categories: the Search page's Browse tiles (colour, title, tilted cover). -->
		<Shelf title={s.title} overline={s.subtitle} items={s.items} href={s.id === 'pod_categories' ? '/browse/podcasts' : '/search'} showAllLabel="Show all" minCard={220}>
			{#snippet card(i)}
				<div class="p-3">
					<BrowseTile
						title={i.title}
						color={i.color ?? '#535353'}
						image={i.thumbnail}
						href={!i.params
							? '/search'
							: i.kind === 'podcast_category'
								? podcastCategoryHref(i.title, i.params)
								: moodHref(i.title, i.params, i.color)}
					/>
				</div>
			{/snippet}
		</Shelf>
	{:else}
		<Shelf title={s.title} overline={s.subtitle} items={s.items} href={showAllHref(s)}>
			{#snippet card(i)}<ShelfCard item={i} />{/snippet}
		</Shelf>
	{/if}
{/snippet}

<style>
	/* 4 columns, 2 when four would squeeze a tile under 160px (home.md §3: 174.75px tiles at an
	   803px main view). 744 = 4 × 160 + 3 × 8 gap + 2 × 40 gutter. The old 600px breakpoint let
	   tiles shrink to ~125px, a 48px cover plus a sliver of title. Columns never depend on how many
	   tiles there are, so two recents still get full-size tiles. */
	.shortcuts {
		grid-template-columns: repeat(4, minmax(0, 1fr));
	}
	@container (max-width: 743px) {
		.shortcuts {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
