<script lang="ts">
	/**
	 * Search page (spotify-spec/screens/search-and-browse.md). The field and its typeahead live in
	 * the shell's top bar; this route shows:
	 *  - empty query → Recent searches (removable cards) + "Browse all" category tiles;
	 *  - a query → sticky filter chips; All = Spotify's current layout: a full-width Top result
	 *    banner, "Featuring <artist>" playlists, a mixed result list (type badges), Music videos,
	 *    more mixed rows, then Artists, Albums, Podcasts, Episodes shelves; each chip has its full
	 *    list view.
	 * URL: /search?q=<query>&type=<filter>.
	 */
	import { untrack } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api';
	import type { BrowseItem, ExploreItem, PodcastEpisode, PodcastSearchResults, PodcastShow, SearchOverview, SearchResults, SongItem } from '$lib/api';
	import { Card, ErrorState, Shelf, Skeleton, TrackTable } from '$lib/kit';
	import IconButton from '$lib/kit/IconButton.svelte';
	import XIcon from 'phosphor-svelte/lib/XIcon';
	import { showHref } from '$lib/kit/actions';
	import { getCached, putCached } from '$lib/pagecache';
	import { overview } from '$lib/discover/overview'; // stream: search-dropdown — shared with the top bar
	import { playback } from '$lib/player.svelte';
	import ExploreCard from '$lib/discover/ExploreCard.svelte';
	import { cardSubtitle } from '$lib/discover/format'; // parity3
	import { podcasts } from '$lib/podcasts.svelte';
	import AudiobookResults from '$lib/audiobooks/AudiobookResults.svelte'; // stream: audiobooks // parity3: Podcasts tile art
	import EpisodeCard from '$lib/discover/EpisodeCard.svelte';
	import EpisodeRow from '$lib/discover/EpisodeRow.svelte';
	import TopResult from '$lib/discover/TopResult.svelte';
	import SearchResultRow from '$lib/discover/SearchResultRow.svelte';
	import VideoCard from '$lib/artistpods/VideoCard.svelte';
	import { actions, openTrackMenu } from '$lib/kit';
	import BrowseTile from '$lib/discover/BrowseTile.svelte';
	import { addRecent, clearRecents, recentSearches, removeRecent, type RecentItem } from '$lib/discover/recents.svelte';
	import { isPlaying, playBrowse, toggle, moodHref } from '$lib/discover/play.svelte';
	import { TILE, TILE_COLORS, loadCharts, loadMoodArt, loadMoods, loadNewReleases, markSkips } from '$lib/discover/data';

	type Filter = 'all' | api.SearchChip | 'audiobooks'; // audiobooks: stream audiobooks
	// Spotify's chip order (search-and-browse.md §29); Episodes and Videos are ours, after it.
	const FILTERS: { id: Filter; label: string }[] = [
		{ id: 'all', label: 'All' },
		{ id: 'songs', label: 'Songs' },
		{ id: 'artists', label: 'Artists' },
		{ id: 'playlists', label: 'Playlists' },
		{ id: 'albums', label: 'Albums' },
		{ id: 'podcasts', label: 'Podcasts & Shows' },
		{ id: 'audiobooks', label: 'Audiobooks' }, // stream: audiobooks
		{ id: 'profiles', label: 'Profiles' },
		{ id: 'episodes', label: 'Episodes' },
		{ id: 'videos', label: 'Videos' }
	];

	const q = $derived((page.url.searchParams.get('q') ?? '').trim());
	const filter = $derived<Filter>(
		(FILTERS.find((f) => f.id === page.url.searchParams.get('type'))?.id ?? 'all') as Filter
	);
	function setFilter(f: Filter) {
		const p = new URLSearchParams(page.url.searchParams);
		if (f === 'all') p.delete('type');
		else p.set('type', f);
		goto(`/search?${p.toString()}`, { replaceState: true, keepFocus: true, noScroll: false });
	}

	// ─────────────────────────── results ───────────────────────────
	// ── stream: search2 ── one `searchOverview` call answers the All view: YouTube's mixed search,
	// the Songs chip's first page (artist-page blend included), the podcast directories (a person's
	// name finds their show) and the Top result across types. It never comes back empty: a
	// correction, the library or "Try these" stand in (crates/core/src/search.rs).
	let ov = $state<SearchOverview | null>(null);
	const res = $derived<SearchResults | null>(ov?.results ?? null);
	const songs = $derived<SongItem[]>(ov?.songs ?? []);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let pods = $state<PodcastSearchResults | null>(null);
	let podsLoading = $state(false);
	// The All tab's Music videos shelf.
	let videosShelf = $state<SongItem[] | null>(null);

	let seq = 0;
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		const query = q;
		clearTimeout(timer);
		if (!query) {
			ov = null;
			pods = null;
			return;
		}
		const hit = getCached<SearchOverview>(`search:v3:${query}`);
		ov = hit ?? null;
		if (!hit) loading = true;
		error = null;
		videosShelf = getCached<SongItem[]>(`search:videos:${query}`);
		pods = getCached<PodcastSearchResults>(`search:pods:${query}`);
		// The top bar already waits 250ms for typing to settle before it navigates here, so a second
		// wait only added a quarter second to every search (stream: perf). `seq` drops a stale answer.
		timer = setTimeout(() => run(query, !!hit), 0);
		return () => clearTimeout(timer);
	});

	async function run(query: string, quiet: boolean) {
		const my = ++seq;
		interacted = false;
		lyricTop = null;
		// Songs by their words (lyricsearch): alongside, never in front. The core answers [] at once
		// unless the query reads like lyrics; a revisit reuses the answer (iTunes allows ~20/min).
		const cachedLyrics = getCached<api.LyricMatch[]>(`search:lyr:${query}`);
		const lyrP = cachedLyrics ? Promise.resolve(cachedLyrics) : api.searchLyrics(query).catch(() => [] as api.LyricMatch[]);
		try {
			const o = await overview(query);
			if (my !== seq) return;
			const marked = await markSkips(o.songs);
			if (my !== seq) return;
			ov = { ...o, songs: marked };
			putCached(`search:v3:${query}`, ov);
			void lyrP.then((l) => applyLyrics(query, my, l, o.songs));
			if (!o.podsPending) {
				pods = { shows: o.shows, episodes: o.episodes };
				putCached(`search:pods:${query}`, pods);
			}
		} catch (e) {
			if (my === seq && !quiet) error = String(e);
		} finally {
			if (my === seq) loading = false;
		}
		if (my !== seq) return;
		if (!videosShelf)
			api
				.searchVideos(ov?.query ?? query)
				.then(markSkips)
				.then((v) => {
					if (query === q) {
						videosShelf = v;
						putCached(`search:videos:${query}`, v);
					}
				})
				.catch(() => {});
		// The directories were slow: ask them on their own (iTunes allows ~20 calls a minute).
		if (ov?.podsPending && !pods) loadPods(query, my);
	}

	// ── lyrics blend (lyricsearch.md "How stream/search2 should blend it") ──
	/** The user scrolled, clicked or typed in the results: late lyric matches only append. */
	let interacted = false;
	let lyricTop = $state<{ q: string; song: SongItem } | null>(null);
	async function applyLyrics(query: string, my: number, lyric: api.LyricMatch[], base: SongItem[]) {
		putCached(`search:lyr:${query}`, lyric);
		if (!lyric.length || my !== seq || !ov) return;
		const card = ov.topShow != null ? ov.shows[ov.topShow]?.title : ov.results.top[0]?.kind !== 'song' ? ov.results.top[0]?.title : undefined;
		const b = await api.searchBlendLyrics(query, base, lyric, interacted, card).catch(() => null);
		if (!b || my !== seq || !ov || !b.added.length) return;
		const marked = await markSkips(b.songs);
		if (my !== seq || !ov) return;
		ov = { ...ov, songs: marked };
		if (b.leadTop && marked[0]) lyricTop = { q: query, song: marked[0] };
	}
	function touch() {
		if (q && filter === 'all') interacted = true;
	}

	let podTimer: ReturnType<typeof setTimeout> | undefined;
	function loadPods(query: string, my: number) {
		clearTimeout(podTimer);
		podsLoading = true;
		podTimer = setTimeout(async () => {
			if (my !== seq) return;
			try {
				const r = await api.podcastSearch(query);
				if (my !== seq) return;
				pods = r;
				putCached(`search:pods:${query}`, r);
			} catch {
				if (my === seq) pods = { shows: [], episodes: [] };
			} finally {
				if (my === seq) podsLoading = false;
			}
		}, 300);
	}

	// ─────────────────────────── chips: endless lists ───────────────────────────
	// ── stream: search2 ── each chip pages through `searchPage` (ytmusicapi's filter params and
	// continuations); the next page is asked for ~1200px before the end, with skeleton rows under
	// the list meanwhile. Songs starts from the All view's first page and token.
	type ChipRow = SongItem | BrowseItem | PodcastShow | PodcastEpisode;
	type Chip = {
		key: string;
		query: string;
		rows: ChipRow[];
		next?: string;
		loading: boolean;
		failed: boolean;
		started: boolean;
		correction?: api.SearchCorrection;
		tryThese?: api.SearchTryThese;
	};
	let chip = $state<Chip | null>(null);
	const rowId = (r: ChipRow) => ('video_id' in r ? r.video_id : r.id);

	$effect(() => {
		const f = filter;
		const query = q;
		if (!query || f === 'all' || f === 'audiobooks') return;
		const key = `${query}|${f}`;
		untrack(() => {
			if (chip?.key === key) return;
			const cached = getCached<Chip>(`search:chip:${key}`);
			if (cached) {
				chip = { ...cached, loading: false, failed: false };
				return;
			}
			// Songs: the All view already holds page one and its token.
			if (f === 'songs' && ov && (ov.query === query || ov.correction) && songs.length) {
				chip = { key, query: ov.query, rows: [...songs], next: ov.songsContinuation, loading: false, failed: false, started: true, correction: ov.correction };
				return;
			}
			chip = { key, query, rows: [], loading: false, failed: false, started: false };
			void loadMore();
		});
	});

	let chipInflight: Promise<void> | null = null;
	function loadMore(): Promise<void> {
		chipInflight ??= fetchChipPage().finally(() => {
			chipInflight = null;
			// A short page can leave the sentinel in view: the observer won't fire again by itself.
			requestAnimationFrame(() => sentinelNear() && chip?.next && !chip.failed && loadMore());
		});
		return chipInflight;
	}
	async function fetchChipPage() {
		const c = chip;
		if (!c || (c.started && !c.next)) return;
		const f = filter as api.SearchChip;
		const key = c.key;
		c.loading = true;
		c.failed = false;
		try {
			let token = c.started ? c.next : undefined;
			let added = 0;
			// A page whose rows are all repeats still moves the token on: try a couple more.
			for (let i = 0; i < 3 && added === 0; i++) {
				const p = await api.searchPage(c.query, f, token);
				if (chip?.key !== key) return;
				let fresh: ChipRow[] = f === 'songs' || f === 'videos' ? await markSkips(p.songs) : f === 'podcasts' ? p.shows : f === 'episodes' ? p.episodes : p.items;
				if (chip?.key !== key) return;
				const have = new Set(chip.rows.map(rowId));
				fresh = fresh.filter((r) => !have.has(rowId(r)));
				added = fresh.length;
				chip.rows = [...chip.rows, ...fresh];
				if (!chip.started) {
					chip.query = p.query || chip.query;
					chip.correction = p.correction;
					chip.tryThese = p.tryThese;
					chip.started = true;
				}
				chip.next = p.continuation;
				token = p.continuation;
				if (!token) break;
			}
			putCached(`search:chip:${key}`, $state.snapshot(chip));
		} catch {
			if (chip?.key === key) chip.failed = true;
		} finally {
			if (chip?.key === key) chip.loading = false;
		}
	}

	let sentinelEl: HTMLElement | null = null;
	const mainScroller = () => (typeof document !== 'undefined' ? document.querySelector<HTMLElement>('[data-main-scroll]') : null);
	function sentinelNear(): boolean {
		const root = mainScroller();
		if (!sentinelEl || !root) return false;
		return sentinelEl.getBoundingClientRect().top - root.getBoundingClientRect().bottom < 1200;
	}
	function moreSentinel(node: HTMLElement) {
		sentinelEl = node;
		const io = new IntersectionObserver(([e]) => e.isIntersecting && loadMore(), {
			root: mainScroller(),
			rootMargin: '0px 0px 1200px 0px'
		});
		io.observe(node);
		return {
			destroy: () => {
				io.disconnect();
				if (sentinelEl === node) sentinelEl = null;
			}
		};
	}
	const chipSongs = $derived((chip?.rows ?? []) as SongItem[]);
	const chipCards = $derived((chip?.rows ?? []) as BrowseItem[]);
	const chipShows = $derived((chip?.rows ?? []) as PodcastShow[]);
	const chipEpisodes = $derived((chip?.rows ?? []) as PodcastEpisode[]);
	const chipEnd = $derived(!!chip && chip.started && !chip.next);

	const showCard = (s: { id: string; title: string; author: string; thumbnail?: string | null }) => ({
		kind: 'podcast' as const,
		id: s.id,
		title: s.title,
		subtitle: s.author,
		thumbnail: s.thumbnail ?? undefined
	});

	// ── stream: podtop / search2 ── a show takes the Top result: a show's name with no YouTube card
	// ("the daily"), or a person's name whose own show it is ("chris williamson" → Modern Wisdom).
	// `searchOverview` decides (`podcasts::search::top_show`); when the directories answered late,
	// the same rule runs on `podcastSearch`'s list.
	let podTop = $state<{ q: string; show: PodcastShow } | null>(null);
	$effect(() => {
		const query = q;
		const o = ov;
		if (!query || !o) {
			podTop = null;
			return;
		}
		if (!o.podsPending) {
			podTop = o.topShow != null && o.shows[o.topShow] ? { q: query, show: o.shows[o.topShow] } : null;
			return;
		}
		const shows = pods?.shows ?? [];
		if (!shows.length) {
			podTop = null;
			return;
		}
		api
			.podcastTopShow(o.query, o.results.top[0] ?? null, o.results.artists[0] ?? null, $state.snapshot(shows) as PodcastShow[])
			.then((i) => {
				if (query === q) podTop = i == null || !shows[i] ? null : { q: query, show: shows[i] };
			})
			.catch(() => {});
	});
	const podcastsFirst = $derived(!!podTop || !!ov?.podcastsFirst);
	const songCard = (s: SongItem): BrowseItem => ({
		kind: 'song',
		id: s.video_id,
		title: s.title,
		subtitle: s.artists,
		thumbnail: s.thumbnail,
		duration: s.duration,
		albumId: s.album_id,
		artistRuns: s.artist_runs,
		explicit: s.explicit
	});
	const top = $derived<BrowseItem | ExploreItem | null>(
		podTop && podTop.q === q
			? showCard(podTop.show)
			: lyricTop && lyricTop.q === q
				? songCard(lyricTop.song)
				: (res?.top?.[0] ?? res?.artists?.[0] ?? res?.songs?.[0] ?? null)
	);
	const allSongs = $derived(songs.length ? songs : (res?.songs ?? []).map(toSong));
	type Row = { kind: 'song'; song: SongItem } | { kind: 'item'; item: BrowseItem };
	const songRow = (song: SongItem): Row => ({ kind: 'song', song });
	const itemRow = (item: BrowseItem): Row => ({ kind: 'item', item });
	// Spotify's mixed list: the top songs, then (after Music videos) a mix of kinds.
	const mixedA = $derived(allSongs.slice(0, 4).map(songRow));
	const mixedB = $derived.by(() => {
		if (!res) return [] as Row[];
		const albums = res.albums.filter((a) => a.id !== top?.id).slice(0, 2).map(itemRow);
		const artists = res.artists.filter((a) => a.id !== top?.id).slice(0, 2).map(itemRow);
		const more = allSongs.slice(4, 6).map(songRow);
		const out: Row[] = [];
		for (let i = 0; i < 2; i++) for (const list of [albums, more, artists]) if (list[i]) out.push(list[i]);
		return out;
	});
	// "Featuring <artist>" holds playlists only: a podcast show (`MPSP…`, "Podcast • …") has its
	// own Podcasts shelf. The backend drops them too; this covers cached/older answers.
	const playlists = $derived(
		(res?.playlists ?? []).filter((p) => !p.id.startsWith('MPSP') && !/^Podcast\b/.test(p.subtitle ?? ''))
	);
	const videos = $derived((videosShelf ?? []).slice(0, 12));
	const tryThese = $derived(ov?.tryThese ?? null);
	const corr = $derived(filter === 'all' ? ov?.correction : chip?.correction);
	const shelfShows = $derived((pods?.shows ?? []).filter((s) => s.id !== (podTop?.show.id ?? '')));

	/** Spotify's small line over corrected results. */
	function correctionText(c: api.SearchCorrection | undefined): string | null {
		if (!c) return null;
		if (c.kind === 'offline') return "You're offline. Showing results from your library and downloads.";
		if (c.kind === 'library') return `Showing results from your library for “${c.original}”`;
		return null;
	}

	function toSong(i: BrowseItem): SongItem {
		return {
			video_id: i.id,
			title: i.title,
			artists: i.subtitle ?? '',
			artist_runs: i.artistRuns,
			duration: i.duration,
			album_id: i.albumId,
			thumbnail: i.thumbnail,
			explicit: i.explicit
		};
	}
	// ─────────────────────────── browse ───────────────────────────
	// parity3: the Podcasts tile wears a followed show's artwork (the follows load after the tiles).
	const podcastArt = $derived(podcasts.follows.find((p) => p.thumbnail)?.thumbnail ?? null);
	type Tile = { title: string; color: string; href: string; image?: string | null; params?: string };
	let tiles = $state<Tile[]>([]);
	let tilesLoading = $state(true);
	let tilesError = $state(false);
	$effect(() => {
		if (q) return;
		untrack(() => {
			if (!tiles.length) loadTiles();
		});
	});
	async function loadTiles() {
		tilesLoading = true;
		tilesError = false;
		const fixed: Tile[] = [
			// Spotify's order (parity3): Music · Podcasts · Audiobooks · Live events · Made for you ·
			// New releases. Spotless has no Live events; Charts stands in for the Music tile.
			{ title: 'Charts', color: TILE.charts, href: '/charts' },
			{ title: 'Podcasts', color: TILE.podcasts, href: '/browse/podcasts' },
			{ title: 'Audiobooks', color: TILE.audiobooks, href: '/browse/audiobooks' }, // stream: audiobooks
			{ title: 'Made for you', color: TILE.made_for_you, href: '/made-for-you' }, // stream: smart2
			{ title: 'New Releases', color: TILE.new_releases, href: '/new-releases' }
		];
		tiles = fixed;
		const [moods] = await Promise.all([
			loadMoods().catch(() => null),
			loadCharts()
				.then((c) => {
					const img = c.highlights.topSongs?.thumbnail ?? c.highlights.trending?.thumbnail;
					tiles = tiles.map((t) => (t.href === '/charts' ? { ...t, image: img } : t));
				})
				.catch(() => {}),
			api
				.madeForYou()
				.then((m) => {
					const img = m.find((c) => c.thumbnail)?.thumbnail;
					tiles = tiles.map((t) => (t.href === '/made-for-you' ? { ...t, image: img } : t));
				})
				.catch(() => {}),
			loadNewReleases()
				.then((n) => {
					const img = n.mix?.thumbnail ?? n.sections[0]?.items[0]?.thumbnail;
					tiles = tiles.map((t) => (t.href === '/new-releases' ? { ...t, image: img } : t));
				})
				.catch(() => {})
		]);
		if (!moods) {
			tilesError = tiles.length <= 4 && !q;
			tilesLoading = false;
			return;
		}
		const seen = new Set<string>();
		const moodTiles: Tile[] = [];
		let n = 0;
		for (const sec of moods)
			for (const m of sec.items) {
				if (seen.has(m.params)) continue;
				seen.add(m.params);
				moodTiles.push({
					title: m.title,
					color: m.color || TILE_COLORS[n++ % TILE_COLORS.length],
					href: moodHref(m.title, m.params, m.color),
					params: m.params
				});
			}
		tiles = [...tiles, ...moodTiles];
		tilesLoading = false;
		// A cover per mood tile (the first playlist of its category).
		try {
			const art = await loadMoodArt(moodTiles.map((t) => t.params!));
			tiles = tiles.map((t) => (t.params && art[t.params] ? { ...t, image: art[t.params] } : t));
		} catch {
			/* tiles stay flat */
		}
	}

	// Recent searches: only what you opened from results, never plain typed queries (as on the iPhone).
	const recents = $derived(recentSearches.items);
	const remember = (i: BrowseItem | RecentItem) => addRecent(i);
	const fold = (x: string) => x.normalize('NFKD').replace(/[\u0300-\u036f]/g, '').toLowerCase().replace(/[^\p{L}\p{N}]+/gu, ' ').trim();
</script>

{#snippet resultRow(row: Row)}
	<SearchResultRow
		{row}
		onPlaySong={row.kind === 'song' ? () => actions.playSingle(row.song) : undefined}
		onOpen={row.kind === 'item' ? () => remember(row.item) : undefined}
	/>
{/snippet}

{#snippet removeButton(onRemove: () => void)}
	<div class="absolute top-2 right-2 z-[1] opacity-0 transition-opacity group-hover/recent:opacity-100 focus-within:opacity-100">
		<IconButton icon={XIcon} label="Remove" size="sm" variant="filled" onclick={onRemove} />
	</div>
{/snippet}

<svelte:window onwheel={touch} onpointerdown={touch} />
<h1 class="visually-hidden">{q ? `Search results for “${q}”` : 'Search'}</h1>
{#if !q}
	<!-- ───────────── Browse (empty query) ───────────── -->
	<div class="flex flex-col gap-8 px-[var(--content-spacing)] pt-[67px] pb-10">
		{#if recents.length}
			<Shelf title="Recent searches" items={recents} showAllLabel="Clear recent searches" onShowAll={clearRecents}>
				{#snippet card(r)}
					<div class="group/recent relative">
						<ExploreCard item={r as BrowseItem} onOpen={() => remember(r)} />
						{@render removeButton(() => removeRecent(r.id))}
					</div>
				{/snippet}
			</Shelf>
		{/if}

		<section>
			<h2 class="mb-2 font-title text-xl leading-[normal] font-bold text-white">Browse all</h2>
			{#if tilesError}
				<ErrorState message="Try reloading the page." onRetry={loadTiles} />
			{:else}
				<div class="browse-grid -mx-3 grid">
					{#each tiles as t, ti (ti + t.href)}
						<div class="p-3"><BrowseTile title={t.title} color={t.color} image={t.image ?? (t.href === '/browse/podcasts' ? podcastArt : null)} href={t.href} /></div>
					{/each}
					{#if tilesLoading}
						{#each Array(9) as _, i (i)}
							<div class="p-3"><div class="aspect-[16/9] rounded-lg bg-bg-tinted"></div></div>
						{/each}
					{/if}
				</div>
			{/if}
		</section>
	</div>
{:else}
	<!-- ───────────── Results ───────────── -->
	<div class="sticky top-0 z-[2] bg-bg-base px-[var(--content-spacing)] pt-4 pb-2">
		<div class="no-scrollbar flex gap-2 overflow-x-auto" role="group" aria-label="Filter results">
			{#each FILTERS as f (f.id)}
				<button type="button" class="chip" aria-pressed={filter === f.id} onclick={() => setFilter(f.id)}>{f.label}</button>
			{/each}
		</div>
	</div>

	<div class="flex flex-col gap-8 px-[var(--content-spacing)] pt-2 pb-10">
		{#if corr && corr.query && fold(corr.query) !== fold(q) && corr.kind !== 'library' && corr.kind !== 'offline'}
			<p class="-mb-4 text-sm text-text-subdued" data-testid="search-correction">
				Showing results for <a class="font-bold text-white hover:underline" href={`/search?q=${encodeURIComponent(corr.query)}`}>{corr.query}</a>
			</p>
		{:else if correctionText(corr)}
			<p class="-mb-4 text-sm text-text-subdued" data-testid="search-correction">{correctionText(corr)}</p>
		{/if}
		{#if error && !res}
			<ErrorState title="Something went wrong" message="Search for something else?" onRetry={() => run(q, false)} retryLabel="Try again" />
		{:else if filter === 'all'}
			{#if loading && !res}
				<div class="h-[104px] rounded-md bg-bg-tinted"></div>
				<Skeleton variant="row" count={4} />
				<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
			{:else if res}
				{#if top}
					<TopResult item={top} onOpen={() => remember(top)} />
				{/if}
				{#if podcastsFirst}{@render podcastShelves()}{/if}
				{#if playlists.length}
					<Shelf
						title={top?.kind === 'artist' ? `Featuring ${top.title}` : 'Playlists'}
						items={playlists}
						mode="fit"
						minCard={128}
						showAll={top?.kind === 'artist' ? false : undefined}
						onShowAll={top?.kind === 'artist' ? undefined : () => setFilter('playlists')}
					>
						{#snippet card(i)}<ExploreCard
								item={i}
								subtitle={cardSubtitle('playlist', i.subtitle, 'by')}
								onOpen={() => remember(i)}
							/>{/snippet}
					</Shelf>
				{/if}
				{#if mixedA.length}
					<section class="flex flex-col {playlists.length ? '-mt-8' : ''}" aria-label="Results">
						{#each mixedA as row, ri (ri)}{@render resultRow(row)}{/each}
					</section>
				{/if}
				{#if videos.length}
					<Shelf title="Music videos" items={videos} onShowAll={() => setFilter('videos')}>
						{#snippet card(v)}
							<VideoCard
								title={v.title}
								subtitle={v.artists}
								image={v.thumbnail}
								playing={playback.now?.videoId === v.video_id && !playback.paused}
								onPlay={() => actions.playSingle(v)}
								oncontextmenu={(e) => openTrackMenu(e, [v])}
							/>
						{/snippet}
					</Shelf>
				{/if}
				{#if mixedB.length}
					<section class="flex flex-col" aria-label="More results">
						{#each mixedB as row, ri (ri)}{@render resultRow(row)}{/each}
					</section>
				{/if}

				{#if res.artists.length}
					<Shelf title="Artists" items={res.artists} onShowAll={() => setFilter('artists')}>
						{#snippet card(i)}<ExploreCard item={i} onOpen={() => remember(i)} />{/snippet}
					</Shelf>
				{/if}
				{#if res.albums.length}
					<Shelf title="Albums" items={res.albums} onShowAll={() => setFilter('albums')}>
						{#snippet card(i)}<ExploreCard item={i} onOpen={() => remember(i)} />{/snippet}
					</Shelf>
				{/if}
				{#if !podcastsFirst}{@render podcastShelves()}{/if}
				<AudiobookResults query={q} shelf onShowAll={() => setFilter('audiobooks')} /><!-- stream: audiobooks -->
				{#if tryThese}{@render tryTheseBlock(tryThese)}{/if}
			{/if}
		{:else if filter === 'audiobooks'}
			<AudiobookResults query={q} /><!-- stream: audiobooks -->
		{:else if !chip || (!chip.started && chip.loading) || (!chip.started && !chip.failed)}
			{#if filter === 'songs' || filter === 'videos' || filter === 'episodes'}
				<Skeleton variant="row" count={10} />
			{:else}
				<div class="results-grid -mx-3 grid"><Skeleton variant="card" count={10} /></div>
			{/if}
		{:else}
			{#if !chip.rows.length && chip.failed}
				<ErrorState title="Something went wrong" message="Search for something else?" onRetry={() => loadMore()} retryLabel="Try again" />
			{:else if filter === 'songs' || filter === 'videos'}
				{#if chipSongs.length}
					<div class="-mx-2"><TrackTable tracks={chipSongs} variant="search" stickyTop={56} bleed="var(--content-spacing)" scroller={mainScroller()} /></div>
				{/if}
			{:else if filter === 'podcasts'}
				<div class="results-grid -mx-3 grid">
					{#each chipShows as s, __i (__i + ':' + s.id)}
						{@const it = showCard(s)}
						<Card
							title={s.title}
							subtitle={s.author}
							image={s.thumbnail}
							kind="podcast"
							href={showHref(s.id)}
							onclick={() => {
								remember(it);
								goto(showHref(s.id));
							}}
							playing={isPlaying(s.id)}
							onPlay={() => toggle(s.id, () => playBrowse(it))}
						/>
					{/each}
				</div>
			{:else if filter === 'episodes'}
				<div class="-mx-4 flex max-w-[900px] flex-col">
					{#each chipEpisodes as e, __i (__i + ':' + e.id)}<EpisodeRow episode={e} />{/each}
				</div>
			{:else}
				<div class="results-grid -mx-3 grid">
					{#each chipCards as i, __i (__i + ':' + i.id)}<ExploreCard item={i} onOpen={() => remember(i)} />{/each}
				</div>
			{/if}
			{#if chip.next && !chip.failed}
				<div use:moreSentinel aria-hidden="true" class="-mt-6">
					{#if filter === 'songs' || filter === 'videos' || filter === 'episodes'}
						<Skeleton variant="row" count={3} />
					{:else}
						<div class="results-grid -mx-3 grid"><Skeleton variant="card" count={5} /></div>
					{/if}
				</div>
			{:else if chip.next && chip.failed}
				<div class="flex justify-center"><button type="button" class="btn-outline" onclick={() => loadMore()}>Try again</button></div>
			{/if}
			{#if chipEnd && chip.tryThese}{@render tryTheseBlock(chip.tryThese)}{/if}
		{/if}
	</div>
{/if}

{#snippet podcastShelves()}
	{#if shelfShows.length}
		<Shelf title="Podcasts" items={shelfShows} onShowAll={() => setFilter('podcasts')}>
			{#snippet card(s)}
				{@const it = showCard(s)}
				<Card
					title={s.title}
					subtitle={s.author}
					image={s.thumbnail}
					kind="podcast"
					href={showHref(s.id)}
					onclick={() => {
						remember(it);
						goto(showHref(s.id));
					}}
					playing={isPlaying(s.id)}
					onPlay={() => toggle(s.id, () => playBrowse(it))}
				/>
			{/snippet}
		</Shelf>
	{/if}
	{#if pods?.episodes.length}
		<Shelf title="Episodes" items={pods.episodes} onShowAll={() => setFilter('episodes')}>
			{#snippet card(e)}<EpisodeCard episode={e} />{/snippet}
		</Shelf>
	{:else if podsLoading}
		<section aria-hidden="true" class="flex flex-col gap-3">
			<Skeleton width="160px" height={20} />
			<div class="-mx-3 grid grid-cols-5 overflow-hidden"><Skeleton variant="card" count={5} /></div>
		</section>
	{/if}
{/snippet}

{#snippet tryTheseBlock(t: api.SearchTryThese)}
	<section class="flex flex-col gap-4" data-testid="search-try-these">
		<div>
			<h2 class="font-title text-xl leading-[normal] font-bold text-white">Try these</h2>
			<p class="text-sm text-text-subdued">Nothing matched “{q}” closely, so here’s some of what you play most.</p>
		</div>
		{#if t.songs.length}
			<div class="-mx-2"><TrackTable tracks={t.songs} variant="search" showHeader={false} /></div>
		{/if}
		{#if t.artists.length}
			<Shelf title="Artists" items={t.artists}>
				{#snippet card(i)}<ExploreCard item={i} onOpen={() => remember(i)} />{/snippet}
			</Shelf>
		{/if}
	</section>
{/snippet}

<style>
	.browse-grid {
		grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
	}
	/* Not the global `.card-grid`: that one pins every row to 12.75rem for 10rem columns and
	   paint-contains each card, so these wider (≥180px) cards had their title and subtitle cut
	   off or hidden under the next row. A results page holds a few dozen cards; auto rows are fine. */
	.results-grid {
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
	}
</style>
