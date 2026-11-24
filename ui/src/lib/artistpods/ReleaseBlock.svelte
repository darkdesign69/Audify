<script lang="ts">
	/**
	 * One release in the discography's list view (Spotify's /artist/<id>/discography, "View as ▸
	 * List"): cover 136px, title, "Album • 2025 • 12 songs", Play · Save · Download · More, then
	 * the release's track table, playable in place. Tracks load when the block nears the viewport
	 * (an artist can have 80 releases), through the album page's own cache key.
	 */
	import type { AlbumPage, BrowseItem } from '$lib/api';
	import { IconButton, PlayButton, Skeleton, TrackTable, actions, openItemMenu } from '$lib/kit';
	import DownloadButton from '$lib/downloads/DownloadButton.svelte';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import { playItem } from '$lib/browse';
	import { thumb } from '$lib/thumb';
	import type { Release } from './discography';
	import { albumTracks } from './discographyData';
	import { isPlayingContext, playOrToggle } from './playing';

	let { release, scroller }: { release: Release; scroller?: HTMLElement | null } = $props();

	const item = $derived<BrowseItem>(release.item);
	let album = $state.raw<AlbumPage | null>(null);
	let failed = $state(false);
	let el = $state<HTMLElement>();

	async function load() {
		if (album || failed) return;
		try {
			album = await albumTracks(item.id);
		} catch {
			failed = true;
		}
	}

	$effect(() => {
		if (!el) return;
		const io = new IntersectionObserver(
			([e]) => {
				if (!e.isIntersecting) return;
				io.disconnect();
				load();
			},
			{ root: scroller ?? null, rootMargin: '600px 0px' }
		);
		io.observe(el);
		return () => io.disconnect();
	});

	const tracks = $derived(album?.items ?? []);
	const ctx = $derived({
		id: item.id,
		title: item.title,
		kind: 'album' as const,
		subtitle: album?.artist,
		thumbnail: item.thumbnail,
		sourceId: album?.playlistId ?? item.id
	});
	const ids = $derived([item.id, album?.playlistId]);
	const playing = $derived(isPlayingContext(...ids));
	const saved = $derived(actions.isInLibrary(item) || !!album?.inLibrary);
	const href = $derived(actions.hrefFor(item));

	function play() {
		playOrToggle(ids, () => {
			const first = tracks.findIndex((s) => !s.unavailable);
			if (first >= 0) actions.playTracks(tracks, first, ctx, undefined, album?.continuation);
			else playItem(item);
		});
	}
</script>

<section bind:this={el} class="flex flex-col gap-4" aria-label={item.title}>
	<div class="flex items-end gap-6">
		<a {href} class="shrink-0" tabindex="-1" aria-hidden="true">
			{#if item.thumbnail}
				<img
					src={thumb(item.thumbnail, 272)}
					alt=""
					loading="lazy"
					class="size-[136px] rounded object-cover shadow-[0_4px_60px_rgba(0,0,0,.5)]"
				/>
			{:else}
				<div class="size-[136px] rounded bg-bg-elevated-highlight"></div>
			{/if}
		</a>
		<div class="flex min-w-0 flex-col gap-2">
			<a {href} class="line-clamp-2 font-title text-[32px] leading-tight font-bold text-white hover:underline">{item.title}</a>
			<span class="text-sm text-text-subdued">{[release.meta, tracks.length ? (tracks.length === 1 ? '1 song' : `${tracks.length} songs`) : ''].filter(Boolean).join(' • ')}</span>
			<div class="mt-2 flex items-center gap-4">
				<PlayButton size="sm" tone="white" {playing} label={`${playing ? 'Pause' : 'Play'} ${item.title}`} onclick={play} />
				<IconButton
					icon={saved ? CheckCircleIcon : PlusCircleIcon}
					fill={saved}
					active={saved}
					label={saved ? 'Remove from Your Library' : 'Save to Your Library'}
					onclick={() => actions.toggleInLibrary(item)}
				/>
				{#if tracks.length}
					<DownloadButton
						target={{ id: item.id, kind: 'album', title: item.title, subtitle: album?.artist ?? null, thumbnail: item.thumbnail ?? null }}
						size="md"
					/>
				{/if}
				<IconButton icon={DotsThreeIcon} label={`More options for ${item.title}`} onclick={(e) => openItemMenu(e, item)} />
			</div>
		</div>
	</div>
	{#if album}
		<TrackTable
			{tracks}
			variant="album"
			context={ctx}
			menuContext={{ albumId: item.id, artistId: album.artistId }}
		/>
	{:else if failed}
		<p class="py-4 text-sm text-text-subdued">
			Couldn't load these songs. <button type="button" class="font-bold text-white hover:underline" onclick={() => ((failed = false), load())}>Try again</button>
		</p>
	{:else}
		<Skeleton variant="row" count={3} />
	{/if}
</section>
