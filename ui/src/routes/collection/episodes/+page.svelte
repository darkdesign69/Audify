<script lang="ts">
	// Your Episodes (spec: podcasts.md "Your Episodes", states.md): the playlist template with the
	// accent bookmark cover and episode rows instead of a track table.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { ownerName } from '$lib/localuser.svelte';
	import { ActionBar, EmptyState, ErrorState, Icon, PageHeader, Skeleton, StickyHeader, type Tint } from '$lib/kit';
	import BookmarkSimpleIcon from 'phosphor-svelte/lib/BookmarkSimpleIcon';
	import { initPodcasts, podcasts, reloadSaved } from '$lib/podcasts.svelte';
	import EpisodeRow from '$lib/artistpods/EpisodeRow.svelte';
	import { isEpisodeCurrent, playEpisodes } from '$lib/artistpods/pods';
	import { playback } from '$lib/player.svelte';
	import * as api from '$lib/api';

	const TINT = 'rgb(90 14 48)'; // --color-accent-tile
	const CONTEXT = { id: 'EPISODES', title: 'Your Episodes', kind: 'playlist' as const };

	let error = $state<string | null>(null);
	let loaded = $state(podcasts.loaded);

	onMount(async () => {
		try {
			await initPodcasts();
			await reloadSaved();
		} catch (e) {
			if (!podcasts.saved.length) error = String(e);
		} finally {
			loaded = true;
		}
	});

	const episodes = $derived(podcasts.saved);
	const count = $derived(episodes.length);
	const playingHere = $derived(
		!!playback.now && !playback.paused && episodes.some((e) => e.id === playback.now?.videoId)
	);

	function play(i = 0) {
		if (!episodes.length) return;
		if (i === 0 && episodes.some((e) => isEpisodeCurrent(e.id))) return api.togglePause();
		playEpisodes(episodes, i, CONTEXT);
	}

	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();
</script>

<svelte:head><title>Your Episodes</title></svelte:head>

<StickyHeader title="Your Episodes" tint={tint?.color} playing={playingHere} onPlay={() => play()} {sentinel} />
<PageHeader type="Playlist" title="Your Episodes" tint={TINT} bind:tintOut={tint} owner={{ name: ownerName(), noAvatar: true }} details={loaded ? `${count} episode${count === 1 ? '' : 's'}` : null}>
	{#snippet imageSnippet()}
		<div class="grid size-full place-items-center bg-accent-tile text-accent">
			<Icon icon={BookmarkSimpleIcon} size={80} fill />
		</div>
	{/snippet}
</PageHeader>

<ActionBar bind:sentinel tint={tint?.color} playing={playingHere} onPlay={() => play()} playLabel="Play Your Episodes">
	<div class="px-[var(--content-spacing)] pb-10">
		{#if !loaded && !episodes.length}
			<div class="flex flex-col gap-4">
				{#each Array(3) as _, i (i)}
					<div class="flex gap-6 py-4">
						<div class="size-[112px] shrink-0 rounded-lg bg-bg-menu"></div>
						<div class="flex flex-1 flex-col gap-3"><Skeleton width="50%" height={16} /><Skeleton width="80%" /><Skeleton width="25%" /></div>
					</div>
				{/each}
			</div>
		{:else if error}
			<ErrorState onRetry={() => location.reload()} />
		{:else if episodes.length}
			<div class="max-w-[1100px]">
				{#each episodes as ep, i (ep.id)}
					<EpisodeRow {ep} showName onPlay={() => play(i)} />
				{/each}
			</div>
		{:else}
			<div class="py-16">
				<EmptyState
					icon={BookmarkSimpleIcon}
					title="Add to Your Episodes"
					body="Save episodes to this list by tapping the plus icon."
					actionLabel="Find podcasts"
					onAction={() => goto('/search?cat=podcasts')}
				/>
			</div>
		{/if}
	</div>
</ActionBar>
