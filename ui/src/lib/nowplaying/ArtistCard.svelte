<script lang="ts">
	/**
	 * "About the artist" card (now-playing-panel.md §5): #1f1f1f, radius 8, overflow hidden. Artist
	 * image across the top with "About the artist" 16/700 over a gradient; body padding 16: name
	 * 16/700, listeners/subscribers 14px #b3b3b3, Follow outline pill right, bio 14px #b3b3b3 3-line
	 * clamp. Click → artist page. Skeleton while loading; hidden if the lookup fails.
	 */
	import type * as api from '$lib/api';
	import { goto } from '$app/navigation';
	import { thumb } from '$lib/thumb';
	import Pill from '$lib/kit/Pill.svelte';
	import { artistHref, isInLibrary, toggleInLibrary } from '$lib/kit/actions';
	import { artistInfo, audienceLine } from './info';

	let { artistId }: { artistId: string } = $props();

	let page = $state<api.ArtistPage | null>(null);
	let status = $state<'loading' | 'ready' | 'error'>('loading');

	$effect(() => {
		const id = artistId;
		let stale = false;
		status = 'loading';
		page = null;
		artistInfo(id)
			.then((a) => {
				if (stale) return;
				page = a;
				status = 'ready';
			})
			.catch(() => !stale && (status = 'error'));
		return () => (stale = true);
	});

	const item = $derived<api.BrowseItem | null>(
		page
			? { kind: 'artist', id: artistId, title: page.name ?? '', thumbnail: page.avatar ?? page.thumbnail }
			: null
	);
	const following = $derived(item ? isInLibrary(item) : false);
	const image = $derived(page ? thumb(page.thumbnail ?? page.avatar, 544) : undefined);
	const audience = $derived(page ? audienceLine(page) : null);
	const open = () => goto(artistHref(artistId));
</script>

{#if status === 'loading'}
	<div class="overflow-hidden rounded-lg bg-bg-highlight" aria-busy="true">
		<div class="aspect-[16/10] w-full animate-pulse bg-bg-menu"></div>
		<div class="space-y-2 p-4">
			<div class="h-4 w-1/2 animate-pulse rounded-full bg-white/10"></div>
			<div class="h-3 w-2/3 animate-pulse rounded-full bg-white/10"></div>
		</div>
	</div>
{:else if status === 'ready' && page}
	<div
		role="link"
		tabindex="0"
		class="group/artist cursor-pointer overflow-hidden rounded-lg bg-bg-highlight"
		onclick={open}
		onkeydown={(e) => e.key === 'Enter' && open()}
	>
		<div class="relative aspect-[16/10] w-full overflow-hidden bg-bg-menu">
			{#if image}
				<img
					src={image}
					alt=""
					class="size-full object-cover transition-transform duration-300 group-hover/artist:scale-[1.02]"
				/>
			{/if}
			<div class="absolute inset-x-0 top-0 h-16 bg-gradient-to-b from-black/50 to-transparent"></div>
			<h3 class="absolute top-4 left-4 text-base font-bold text-white">About the artist</h3>
		</div>
		<div class="p-4">
			<div class="flex items-center gap-3">
				<div class="min-w-0 flex-1">
					<div class="truncate text-base font-bold text-white group-hover/artist:underline">
						{page.name}
					</div>
					{#if audience}
						<div class="mt-1 text-sm text-text-subdued">{audience}</div>
					{/if}
				</div>
				{#if item}
					<!-- svelte-ignore a11y_click_events_have_key_events -->
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="shrink-0" onclick={(e) => e.stopPropagation()}>
						<Pill variant="outline" onclick={() => item && toggleInLibrary(item)}>
							{following ? 'Following' : 'Follow'}
						</Pill>
					</span>
				{/if}
			</div>
			{#if page.description}
				<p class="mt-4 line-clamp-3 text-sm text-text-subdued">{page.description}</p>
			{/if}
		</div>
	</div>
{/if}
