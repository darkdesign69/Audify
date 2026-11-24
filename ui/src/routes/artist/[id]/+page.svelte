<script lang="ts">
	// Artist page (spec: spotify-spec/screens/artist.md). Full-bleed hero when YouTube has a banner,
	// the round-avatar header otherwise; Popular (5 → 10), Discography with chips, Featuring, Music
	// videos, About card + modal, Artist playlists, Fans also like, Appears on.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api';
	import type { ArtistPage, BrowseItem, SongItem } from '$lib/api';
	import {
		ActionBar,
		Card,
		ErrorState,
		Modal,
		PageHeader,
		Pill,
		Shelf,
		Skeleton,
		StickyHeader,
		TrackTable,
		actions,
		openItemMenu,
		openTrackMenu,
		type Tint
	} from '$lib/kit';
	import { lib } from '$lib/library.svelte';
	import { playFrom, playback } from '$lib/player.svelte';
	import { getCached, putCached } from '$lib/pagecache';
	import { asSong, playItem } from '$lib/browse';
	import { thumb } from '$lib/thumb';
	import VideoCard from '$lib/artistpods/VideoCard.svelte';
	import { audienceLine, type SectionKind } from '$lib/artistpods/artist';
	import { EMPTY_DISCOGRAPHY, FILTER_LABEL, availableFilters, type DiscFilter, type Discography } from '$lib/artistpods/discography';
	import { carouselDiscography, fullDiscography, topSongs, topSongsPlaylist } from '$lib/artistpods/discographyData';
	import { isContext, isPlayingContext, playOrToggle } from '$lib/artistpods/playing';

	let artist = $state.raw<ArtistPage | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let appearsOn = $state.raw<BrowseItem[]>([]);
	// "See more" under Popular: the first 10 of the artist's full top-songs playlist.
	let moreSongs = $state.raw<SongItem[] | null>(null);
	let expanded = $state(false);
	let expanding = $state(false);
	let aboutOpen = $state(false);
	// Discography (the core's chips): the carousels' releases at once, every release (and the top
	// songs that rank Popular releases) once the background fetch lands.
	let disco = $state.raw<Discography>(EMPTY_DISCOGRAPHY);
	let discoFull = false;

	const id = $derived(page.params.id ?? '');

	async function load(cid: string) {
		const key = `artist:${cid}`;
		const hit = getCached<ArtistPage>(key);
		artist = hit ?? null;
		loading = !hit;
		error = null;
		expanded = false;
		moreSongs = null;
		disco = EMPTY_DISCOGRAPHY;
		discoFull = false;
		if (hit) showCarouselDiscography(cid, hit);
		appearsOn = getCached<BrowseItem[]>(`appears:${cid}`) ?? [];
		try {
			const fresh = await api.getArtist(cid);
			if (cid !== id) return;
			artist = fresh;
			putCached(key, fresh);
			if (!hit) showCarouselDiscography(cid, fresh);
			loadDiscography(cid, fresh);
			if (!appearsOn.length) loadAppearsOn(cid, fresh);
			fillDurations(cid, fresh);
		} catch (e) {
			if (cid !== id) return;
			if (!hit) error = String(e);
		} finally {
			if (cid === id) loading = false;
		}
	}

	// YouTube's artist page lists Popular without lengths; Spotify shows one on every row. The
	// artist's songs playlist (`topSongsId`) has them for the same video ids, and See more and
	// Shuffle read that same cached playlist, so this costs one fetch they'd mostly make anyway.
	// Never blocks the page: the rows paint first and the times fill in.
	async function fillDurations(cid: string, a: ArtistPage) {
		const pid = a.topSongsId;
		if (!pid || a.topSongs.every((s) => s.duration)) return;
		try {
			const pl = await topSongsPlaylist(pid);
			if (cid !== id || artist !== a) return;
			const byId = new Map(pl.items.filter((s) => s.duration).map((s) => [s.video_id, s.duration]));
			const byTitle = new Map(
				pl.items.filter((s) => s.duration).map((s) => [s.title.trim().toLowerCase(), s.duration])
			);
			let changed = false;
			const topSongs = a.topSongs.map((s) => {
				if (s.duration) return s;
				const d = byId.get(s.video_id) ?? byTitle.get(s.title.trim().toLowerCase());
				if (!d) return s;
				changed = true;
				return { ...s, duration: d };
			});
			if (!changed) return;
			artist = { ...a, topSongs };
			putCached(`artist:${cid}`, artist);
		} catch {
			/* the rows just stay without a length */
		}
	}

	function showCarouselDiscography(cid: string, a: ArtistPage) {
		carouselDiscography(a)
			.then((d) => cid === id && !discoFull && (disco = d))
			.catch(() => {});
	}

	async function loadDiscography(cid: string, a: ArtistPage) {
		try {
			const d = await fullDiscography(a, topSongs(a));
			if (cid !== id || !d.all.length) return;
			discoFull = true;
			disco = d;
		} catch {
			/* the carousels' releases stay */
		}
	}

	async function loadAppearsOn(cid: string, a: ArtistPage) {
		if (!a.topSongsId) return;
		try {
			const own = new Set(a.sections.flatMap((s) => s.items.map((i) => i.id)));
			const items = await api.getArtistAppearsOn(cid, a.topSongsId);
			if (cid !== id) return;
			appearsOn = items.filter((i) => !own.has(i.id));
			putCached(`appears:${cid}`, appearsOn);
		} catch {
			/* the shelf just stays off */
		}
	}

	$effect(() => {
		if (id) load(id);
	});

	const name = $derived(artist?.name ?? '');
	const item = $derived<BrowseItem>({
		kind: 'artist',
		id,
		title: name,
		subtitle: 'Artist',
		thumbnail: artist?.avatar ?? artist?.thumbnail
	});
	const following = $derived(lib.isSaved(id) || actions.isInLibrary(item));
	const audience = $derived(artist ? audienceLine(artist) : null);
	const hero = $derived(!!artist?.thumbnail && !artist?.avatar);

	// ── Sections, mapped onto Spotify's ──────────────────────────────────────────────────────
	type Sec = { title: string; items: BrowseItem[]; kind: SectionKind; more?: string; params?: string };
	const sections = $derived<Sec[]>(
		(artist?.sections ?? []).map((s) => ({
			title: s.title,
			items: s.items,
			kind: s.kind ?? 'other',
			more: s.moreBrowseId,
			params: s.moreParams
		}))
	);
	const of = (k: SectionKind) => sections.find((s) => s.kind === k);
	const videos = $derived(of('videos'));
	const featured = $derived(of('featured'));
	const playlists = $derived(of('playlists'));
	const fans = $derived(of('fans'));
	const others = $derived(sections.filter((s) => s.kind === 'other'));

	let disc = $state<DiscFilter>('popular');
	const discOptions = $derived(availableFilters(disco, true));
	$effect(() => {
		if (discOptions.length && !discOptions.includes(disc)) disc = 'popular';
	});
	const discItems = $derived(disco[disc].map((r) => ({ i: r.item, sub: r.line })));
	const discHref = $derived(
		`/artist/${encodeURIComponent(id)}/discography/${disc === 'popular' ? 'all' : disc}`
	);

	// ── Popular ──────────────────────────────────────────────────────────────────────────────
	const popular = $derived(
		expanded && moreSongs ? moreSongs.slice(0, 10) : (artist?.topSongs ?? []).slice(0, expanded ? 10 : 5)
	);
	const canExpand = $derived(!!artist && (artist.topSongs.length > 5 || !!artist.topSongsId));

	async function seeMore() {
		if (expanded) {
			expanded = false;
			return;
		}
		expanded = true;
		if (moreSongs || !artist?.topSongsId || artist.topSongs.length >= 10) return;
		const pid = artist.topSongsId;
		const cid = id;
		expanding = true;
		try {
			const pl = await topSongsPlaylist(pid);
			if (cid !== id) return;
			// YouTube's playlist order is the same popularity order; keep the page's rows first
			// (they carry play counts) and fill with the playlist's next ones.
			const seen = new Set(artist.topSongs.map((s) => s.video_id));
			const len = new Map(pl.items.map((s) => [s.video_id, s.duration]));
			const top = artist.topSongs.map((s) =>
				s.duration || !len.get(s.video_id) ? s : { ...s, duration: len.get(s.video_id) }
			);
			moreSongs = [...top, ...pl.items.filter((s) => !seen.has(s.video_id))];
		} catch {
			/* keep showing what we have */
		} finally {
			expanding = false;
		}
	}

	// ── Play / shuffle ───────────────────────────────────────────────────────────────────────
	const ctxIds = $derived([id, artist?.topSongsId]);
	const playing = $derived(isPlayingContext(...ctxIds));
	const shuffleOn = $derived(isContext(...ctxIds) && !!playback.queue.shuffle);

	function play() {
		if (!artist) return;
		playOrToggle(ctxIds, () =>
			actions.playTracks(moreSongs ?? artist!.topSongs, 0, { id, title: name, kind: 'artist' })
		);
	}

	// Shuffle while this artist plays toggles the queue's shuffle; otherwise it starts the whole
	// top-songs playlist shuffled (the backend pages the rest in), else the rows on this page.
	async function shuffle() {
		if (!artist) return;
		if (isContext(...ctxIds)) return api.toggleShuffle();
		const pid = artist.topSongsId;
		const cid = id;
		if (pid) {
			try {
				const pl = await topSongsPlaylist(pid);
				if (cid !== id) return;
				if (pl.items.length) {
					playFrom(item, pl.items, null, pid, true, pl.continuation);
					return;
				}
			} catch {
				/* fall back to the visible rows */
			}
		}
		if (artist.topSongs.length) {
			playFrom(item, artist.topSongs, null, id, true);
		}
	}

	async function toggleFollow() {
		// Spotify: instant, no toast.
		try {
			if (lib.isSaved(id)) await lib.unsave(id);
			else await lib.save(item);
		} catch {
			/* the pill keeps its state */
		}
	}

	// ── Sticky header + tint ─────────────────────────────────────────────────────────────────
	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();

	const cardPlaying = (i: BrowseItem) => isPlayingContext(i.id);
	const playCard = (i: BrowseItem) => playOrToggle([i.id], () => playItem(i));
	const playVideo = (i: BrowseItem) => actions.playTracks([asSong(i)], 0);
</script>

<svelte:head><title>{name || 'Artist'}</title></svelte:head>

{#snippet releaseCard(entry: { i: BrowseItem; sub: string })}
	<Card
		title={entry.i.title}
		subtitle={entry.sub}
		image={entry.i.thumbnail}
		kind="album"
		href={actions.hrefFor(entry.i)}
		playing={cardPlaying(entry.i)}
		onPlay={() => playCard(entry.i)}
		oncontextmenu={(e) => openItemMenu(e, entry.i)}
	/>
{/snippet}

{#snippet itemCard(i: BrowseItem)}
	{#if i.kind === 'song'}
		<Card
			title={i.title}
			subtitle={i.subtitle}
			image={i.thumbnail}
			kind="song"
			onclick={() => playVideo(i)}
			playing={playback.now?.videoId === i.id && !playback.paused}
			onPlay={() => playVideo(i)}
			oncontextmenu={(e) => openTrackMenu(e, [asSong(i)])}
		/>
	{:else}
		<Card
			title={i.title}
			subtitle={i.kind === 'artist' ? 'Artist' : i.subtitle}
			image={i.thumbnail}
			kind={i.kind}
			href={actions.hrefFor(i)}
			playing={cardPlaying(i)}
			onPlay={() => playCard(i)}
			oncontextmenu={(e) => openItemMenu(e, i)}
		/>
	{/if}
{/snippet}

{#snippet shelf(s: Sec, title: string)}
	<Shelf
		{title}
		items={s.items}
		key={(i) => i.id}
		href={s.more
			? `/list?${new URLSearchParams({ id: s.more, title, ...(s.params ? { params: s.params } : {}) })}`
			: undefined}
	>
		{#snippet card(i)}{@render itemCard(i)}{/snippet}
	</Shelf>
{/snippet}

{#if loading}
	<div aria-busy="true">
		<div class="flex items-end gap-6 px-[var(--content-spacing)] pt-[60px] pb-6" style="height:clamp(340px,40vh,500px)">
			<div class="flex w-full flex-col gap-4">
				<Skeleton width="60%" height={72} />
				<Skeleton width="30%" height={14} />
			</div>
		</div>
		<div class="px-[var(--content-spacing)] py-6"><Skeleton variant="row" count={5} /></div>
	</div>
{:else if error}
	<div class="px-[var(--content-spacing)] py-16">
		<ErrorState message="Couldn't load this artist. Try reloading the page." onRetry={() => load(id)} />
	</div>
{:else if artist}
	<StickyHeader title={name} tint={tint?.color} {playing} onPlay={play} {sentinel} />

	{#if hero}
		<PageHeader variant="artist-hero" title={name} image={artist.thumbnail} bind:tintOut={tint}>
			{#snippet meta()}
				{#if audience}<span class="text-base text-white">{audience}</span>{/if}
			{/snippet}
		</PageHeader>
	{:else}
		<PageHeader type="Artist" title={name} image={artist.avatar ?? artist.thumbnail} round bind:tintOut={tint}>
			{#snippet meta()}
				{#if audience}<span class="text-base leading-8 text-white">{audience}</span>{/if}
			{/snippet}
		</PageHeader>
	{/if}

	<ActionBar
		bind:sentinel
		tint={tint?.color}
		{playing}
		onPlay={play}
		playLabel={`Play ${name}`}
		shuffle={shuffleOn}
		onShuffle={shuffle}
		onMore={(e) => openItemMenu(e, item)}
		moreLabel={`More options for ${name}`}
	>
		{#snippet extra()}
			<Pill variant="outline" onclick={toggleFollow} class={following ? '!border-white' : ''}>
				{following ? 'Following' : 'Follow'}
			</Pill>
		{/snippet}

		<div class="flex flex-col gap-10 px-[var(--content-spacing)] pb-10">
			{#if artist.topSongs.length}
				<section>
					<h2 class="mt-2 mb-[17px] font-title text-xl leading-[normal] font-bold text-white">Popular</h2>
					<TrackTable
						tracks={popular}
						variant="artist"
						context={{ id, title: name, kind: 'artist' }}
						menuContext={{ artistId: id }}
						showHeader={false}
					/>
					{#if canExpand}
						<button
							type="button"
							class="mt-2 p-4 text-sm font-bold text-white/70 hover:text-white"
							disabled={expanding}
							onclick={seeMore}
						>
							{expanded ? 'Show less' : 'See more'}
						</button>
					{/if}
				</section>
			{/if}

			{#if discOptions.length}
				<Shelf title="Discography" href={discHref} items={discItems} key={(e) => e.i.id}>
					{#snippet controls()}
						<div class="no-scrollbar mb-4 flex gap-2 overflow-x-auto" role="group">
							{#each discOptions as o (o)}
								<Pill variant="chip" selected={disc === o} onclick={() => (disc = o)}>{FILTER_LABEL[o]}</Pill>
							{/each}
						</div>
					{/snippet}
					{#snippet card(e)}{@render releaseCard(e)}{/snippet}
				</Shelf>
			{/if}

			{#if featured?.items.length}{@render shelf(featured, `Featuring ${name}`)}{/if}

			{#if videos?.items.length}
				<Shelf
					title="Music videos"
					items={videos.items}
					key={(i) => i.id}
					href={videos.more
						? `/list?${new URLSearchParams({ id: videos.more, title: 'Music videos', ...(videos.params ? { params: videos.params } : {}) })}`
						: undefined}
				>
					{#snippet card(i)}
						<VideoCard
							title={i.title}
							subtitle={i.subtitle}
							image={i.thumbnail}
							playing={playback.now?.videoId === i.id && !playback.paused}
							onPlay={() => playVideo(i)}
							oncontextmenu={(e) => openTrackMenu(e, [asSong(i)])}
						/>
					{/snippet}
				</Shelf>
			{/if}

			{#each others as s (s.title)}{@render shelf(s, s.title)}{/each}

			{#if artist.description || audience}
				<section>
					<h2 class="mb-4 font-title text-xl font-bold text-white">About</h2>
					<button
						type="button"
						class="group relative block w-full max-w-[830px] overflow-hidden rounded-lg bg-bg-elevated-highlight text-left"
						style="aspect-ratio: 763 / 567; max-height: 70vh"
						onclick={() => (aboutOpen = true)}
					>
						{#if artist.thumbnail || artist.avatar}
							<img
								src={thumb(artist.avatar ?? artist.thumbnail, 1200)}
								alt=""
								class="absolute inset-0 size-full object-cover object-[50%_20%] transition-transform duration-300 group-hover:scale-[1.01]"
							/>
						{/if}
						<div class="absolute inset-0" style="background:linear-gradient(transparent 40%, rgb(0 0 0 / 0.75))"></div>
						<div class="absolute inset-x-0 bottom-0 flex flex-col gap-2 p-10">
							{#if audience}<span class="text-base font-bold text-white">{audience}</span>{/if}
							{#if artist.description}
								<p class="line-clamp-3 max-w-[672px] text-base text-white">{artist.description}</p>
							{/if}
						</div>
					</button>
				</section>
			{/if}

			{#if playlists?.items.length}{@render shelf(playlists, 'Artist Playlists')}{/if}
			{#if fans?.items.length}{@render shelf(fans, 'Fans also like')}{/if}
			{#if appearsOn.length}
				<Shelf title="Appears On" items={appearsOn} key={(i) => i.id}>
					{#snippet card(i)}{@render itemCard(i)}{/snippet}
				</Shelf>
			{/if}
		</div>
	</ActionBar>

	<Modal bind:open={aboutOpen} width={760} title={name}>
		<div class="flex flex-col">
			{#if artist.thumbnail || artist.avatar}
				<img src={thumb(artist.thumbnail ?? artist.avatar, 1200)} alt="" class="max-h-[420px] w-full rounded-lg object-cover object-[50%_20%]" />
			{/if}
			<div class="flex flex-col gap-6 pt-6 md:flex-row">
				<div class="flex shrink-0 flex-col gap-4 md:w-48">
					{#if artist.monthlyListeners}
						<div><div class="text-2xl font-bold text-white">{artist.monthlyListeners.replace(/\s.*$/, '')}</div><div class="text-sm text-text-subdued">{artist.monthlyListeners.replace(/^\S+\s*/, '') || 'Monthly audience'}</div></div>
					{/if}
					{#if artist.subscribers}
						<div><div class="text-2xl font-bold text-white">{artist.subscribers.replace(/\s.*$/, '')}</div><div class="text-sm text-text-subdued">{artist.subscribers.replace(/^\S+\s*/, '') || 'Subscribers'}</div></div>
					{/if}
				</div>
				<p class="text-base whitespace-pre-line text-text-subdued">{artist.description ?? ''}</p>
			</div>
		</div>
	</Modal>
{/if}
