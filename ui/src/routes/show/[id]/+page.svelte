<script lang="ts">
	// Podcast show page (spec: spotify-spec/screens/podcasts.md "Show page"). `id` is a YouTube
	// `MPSP…` show id or an RSS feed URL (URL-encoded in the route).
	import { page } from '$app/state';
	import * as api from '$lib/api';
	import type { EpisodeSort, PodcastEpisode, PodcastShowPage } from '$lib/api';
	import {
		EmptyState,
		ErrorState,
		IconButton,
		PageHeader,
		Pill,
		Skeleton,
		StickyHeader,
		channels,
		openMenu,
		toast,
		type Tint
	} from '$lib/kit';
	import type { MenuEntry } from '$lib/kit/menus';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import UserPlusIcon from 'phosphor-svelte/lib/UserPlusIcon';
	import UserMinusIcon from 'phosphor-svelte/lib/UserMinusIcon';
	import CheckIcon from 'phosphor-svelte/lib/CheckIcon';
	import LinkIcon from 'phosphor-svelte/lib/LinkIcon';
	import ShareFatIcon from 'phosphor-svelte/lib/ShareFatIcon';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';
	import MicrophoneIcon from 'phosphor-svelte/lib/MicrophoneIcon';
	import { Icon } from '$lib/kit';
	import { isFollowed, reloadContinueListening, reloadNewEpisodes, toggleFollow, podcasts } from '$lib/podcasts.svelte';
	import { copyText } from '$lib/clipboard';
	import { getCached, putCached } from '$lib/pagecache';
	import EpisodeRow from '$lib/artistpods/EpisodeRow.svelte';
	import { playEpisodes, showContext } from '$lib/artistpods/pods';

	const id = $derived(page.params.id ?? '');

	let data = $state.raw<PodcastShowPage | null>(null);
	let episodes = $state.raw<PodcastEpisode[]>([]);
	let continuation = $state<string | null>(null);
	let sort = $state<EpisodeSort>('newest');
	let loading = $state(true);
	let error = $state<string | null>(null);
	let more = $state(false);
	let aboutOpen = $state(false);

	async function load(cid: string, s: EpisodeSort) {
		const key = `show:${cid}:${s}`;
		const hit = getCached<PodcastShowPage>(key);
		if (hit) {
			data = hit;
			episodes = hit.episodes;
			continuation = hit.continuation ?? null;
		}
		loading = !hit && !data;
		error = null;
		try {
			const fresh = await api.podcastShow(cid, s);
			if (cid !== id || s !== sort) return;
			data = fresh;
			episodes = fresh.episodes;
			continuation = fresh.continuation ?? null;
			putCached(key, fresh);
		} catch (e) {
			if (cid !== id) return;
			if (!hit) error = String(e);
		} finally {
			if (cid === id) loading = false;
		}
	}

	let lastId = '';
	$effect(() => {
		const cid = id;
		if (cid !== lastId) {
			lastId = cid;
			data = null;
			episodes = [];
			sort = 'newest';
			aboutOpen = false;
		}
		if (cid) load(cid, sort);
	});

	async function loadMore() {
		if (more || !continuation) return;
		const cid = id;
		const s = sort;
		more = true;
		try {
			const r = await api.podcastShowMore(cid, continuation, s);
			if (cid !== id || s !== sort) return;
			const seen = new Set(episodes.map((e) => e.id));
			episodes = [...episodes, ...r.episodes.filter((e) => !seen.has(e.id))];
			continuation = r.continuation ?? null;
		} catch {
			continuation = null;
		} finally {
			more = false;
		}
	}

	function sentinel(node: HTMLElement) {
		const io = new IntersectionObserver(([e]) => e.isIntersecting && loadMore(), { rootMargin: '800px 0px' });
		io.observe(node);
		return () => io.disconnect();
	}

	const show = $derived(data?.show);
	const followed = $derived(show ? isFollowed(show.id) || podcasts.follows.some((f) => f.id === id) : false);

	async function follow() {
		if (!show) return;
		try {
			await toggleFollow(show);
		} catch (e) {
			toast.error(String(e));
		}
	}

	const shareUrl = $derived(
		show
			? show.source === 'youtube'
				? `https://music.youtube.com/browse/${show.ytBrowseId ?? show.id}`
				: (show.feedUrl ?? show.id)
			: null
	);

	async function markAllPlayed() {
		if (!show) return;
		try {
			const n = await api.podcastMarkShowPlayed(show.id);
			const p = await api.podcastProgress(episodes.map((e) => e.id));
			for (const [k, v] of Object.entries(p)) podcasts.progress[k] = v;
			await Promise.all([reloadContinueListening(), reloadNewEpisodes()]);
			toast(n ? `Marked ${n} episode${n === 1 ? '' : 's'} as played` : 'All episodes are played');
		} catch (e) {
			toast.error(String(e));
		}
	}

	function moreMenu(e: MouseEvent) {
		const entries: MenuEntry[] = [
			{ label: followed ? 'Unfollow' : 'Follow', icon: followed ? UserMinusIcon : UserPlusIcon, onSelect: follow },
			{ label: 'Mark all as played', icon: CheckIcon, onSelect: markAllPlayed }
		];
		if (shareUrl)
			entries.push(
				{ type: 'separator' },
				{
					type: 'sub',
					label: 'Share',
					icon: ShareFatIcon,
					items: [
						{
							label: show?.source === 'rss' ? 'Copy RSS feed link' : 'Copy link to show',
							icon: LinkIcon,
							onSelect: async () => {
								await copyText(shareUrl!);
								toast('Link copied to clipboard');
							}
						}
					]
				}
			);
		openMenu(e, entries);
	}

	function sortMenu(e: MouseEvent) {
		openMenu(
			e,
			[
				{ type: 'heading', label: 'Sort by' },
				{ label: 'Newest', checked: sort === 'newest', onSelect: () => (sort = 'newest') },
				{ label: 'Oldest', checked: sort === 'oldest', onSelect: () => (sort = 'oldest') }
			],
			{ width: 160 }
		);
	}

	const play = (i: number) => show && playEpisodes(episodes, i, showContext(show));

	let tint = $state<Tint>();
	let bar = $state<HTMLElement>();
</script>

<svelte:head><title>{show?.title ?? 'Podcast'}</title></svelte:head>

{#if loading && !show}
	<div aria-busy="true">
		<div class="px-[var(--content-spacing)] pt-[60px] pb-6"><Skeleton variant="header" /></div>
		<div class="flex flex-col gap-4 px-[var(--content-spacing)] py-6">
			{#each Array(4) as _, i (i)}
				<div class="flex gap-6 py-4">
					<div class="size-[112px] shrink-0 rounded-lg bg-bg-menu"></div>
					<div class="flex flex-1 flex-col gap-3"><Skeleton width="50%" height={16} /><Skeleton width="80%" /><Skeleton width="25%" /></div>
				</div>
			{/each}
		</div>
	</div>
{:else if error && !show}
	<div class="px-[var(--content-spacing)] py-16">
		<ErrorState message="Couldn't load this show. Try reloading the page." onRetry={() => load(id, sort)} />
	</div>
{:else if show}
	<StickyHeader title={show.title} tint={tint?.color} sentinel={bar} />
	<PageHeader type="Podcast" title={show.title} image={show.thumbnail} bind:tintOut={tint}>
		{#snippet meta()}
			<span class="-mt-1 text-xl font-bold text-white">{show.author}</span>
		{/snippet}
	</PageHeader>

	<div class="relative">
		{#if tint}
			<div
				class="pointer-events-none absolute inset-x-0 top-0 h-[232px]"
				style="background:linear-gradient(rgb({channels(tint.color)} / 0.7), transparent)"
			></div>
			<div class="noise pointer-events-none absolute inset-x-0 top-0 h-[232px]"></div>
		{/if}
		<div bind:this={bar} class="relative flex items-center gap-[var(--content-spacing)] px-[var(--content-spacing)] py-6">
			<Pill variant="outline" onclick={follow} class={followed ? '!border-white' : ''}>{followed ? 'Following' : 'Follow'}</Pill>
			<IconButton icon={DotsThreeIcon} label={`More options for ${show.title}`} size="lg" class="!size-8" onclick={moreMenu} />
		</div>

		<div class="relative flex flex-col gap-8 px-[var(--content-spacing)] pb-10">
			{#if show.description}
				<section>
					<h2 class="mb-4 font-title text-xl font-bold text-white">About</h2>
					<p class="text-base whitespace-pre-line text-text-subdued {aboutOpen ? '' : 'line-clamp-3'}">{show.description}</p>
					{#if show.description.length > 240}
						<button type="button" class="mt-2 text-base font-bold text-white hover:underline" onclick={() => (aboutOpen = !aboutOpen)}>
							{aboutOpen ? 'Show less' : '... Show more'}
						</button>
					{/if}
					{#if show.episodeCount}
						<div class="mt-4 flex gap-2">
							<span class="inline-flex h-8 items-center rounded-full bg-bg-tinted px-3 text-sm text-white">{show.episodeCount} episodes</span>
							<span class="inline-flex h-8 items-center rounded-full bg-bg-tinted px-3 text-sm text-white">{show.source === 'rss' ? 'RSS' : 'YouTube'}</span>
						</div>
					{/if}
				</section>
			{/if}

			<section>
				<div class="mb-2 flex items-center justify-between gap-4">
					<h2 class="font-title text-xl font-bold text-white">All Episodes</h2>
					<button
						type="button"
						class="flex items-center gap-1 text-sm text-text-subdued hover:text-white"
						onclick={sortMenu}
					>
						{sort === 'newest' ? 'Newest' : 'Oldest'}
						<Icon icon={CaretDownIcon} size={16} />
					</button>
				</div>
				{#if episodes.length}
					<div>
						{#each episodes as ep, i (ep.id)}
							<EpisodeRow {ep} showName="plain" onPlay={() => play(i)} />
						{/each}
					</div>
					{#if continuation}
						<div {@attach sentinel} class="py-4"><Skeleton variant="row" count={2} /></div>
					{/if}
				{:else if loading}
					<Skeleton variant="row" count={4} />
				{:else}
					<EmptyState icon={MicrophoneIcon} size="sm" title="No episodes yet" body="New episodes will show up here when the show publishes them." />
				{/if}
			</section>
		</div>
	</div>
{/if}
