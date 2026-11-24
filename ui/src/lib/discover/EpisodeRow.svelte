<script lang="ts">
	/** Episode row for search's Episodes view (podcasts.md row style): 112px art, title 16/700,
	 *  show, 2-line description, then date • length with a progress bar and play button. */
	import type { PodcastEpisode } from '$lib/api';
	import * as api from '$lib/api';
	import PlayButton from '$lib/kit/PlayButton.svelte';
	import { thumb } from '$lib/thumb';
	import { playback } from '$lib/player.svelte';
	import { podcasts } from '$lib/podcasts.svelte';
	import { goto } from '$app/navigation';
	import { playEpisodes } from './play.svelte';
	import { episodeDate, episodeLength } from './format';

	let { episode, onOpen }: { episode: PodcastEpisode; onOpen?: () => void } = $props();
	const p = $derived(podcasts.progress[episode.id] ?? episode.progress);
	const current = $derived(playback.now?.videoId === episode.id);
	const href = $derived(`/episode/${encodeURIComponent(episode.id)}`);
	const text = $derived(
		(episode.description ?? '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim()
	);
	function play() {
		if (current) api.togglePause().catch(() => {});
		else playEpisodes(`ep:${episode.id}`, episode.showTitle, [episode], episode.thumbnail);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group flex cursor-pointer gap-4 rounded-md p-4 hover:bg-bg-tinted"
	onclick={(e) => {
		if ((e.target as HTMLElement).closest('button,a')) return;
		onOpen?.();
		goto(href);
	}}
>
	<img
		src={thumb(episode.thumbnail, 240)}
		alt=""
		loading="lazy"
		class="size-28 shrink-0 rounded-lg bg-bg-menu object-cover shadow-card-image"
	/>
	<div class="flex min-w-0 flex-1 flex-col gap-1">
		<a {href} class="line-clamp-2 text-base font-bold text-white hover:underline" onclick={() => onOpen?.()}
			>{episode.title}</a
		>
		<p class="truncate text-sm text-text-subdued">
			{episode.isVideo ? 'Video • ' : ''}{episode.showTitle}
		</p>
		{#if text}<p class="line-clamp-2 text-sm text-text-subdued">{text}</p>{/if}
		<div class="mt-auto flex items-center gap-3 pt-2">
			<span class="text-sm text-text-subdued">
				{[episodeDate(episode.publishedAt), episodeLength(episode, p)].filter(Boolean).join(' • ')}
			</span>
			{#if p.positionSecs > 0 && !p.played}
				<span class="block h-1 w-24 overflow-hidden rounded-full bg-white/30">
					<span class="block h-full bg-white" style="width:{Math.round(p.fraction * 100)}%"></span>
				</span>
			{/if}
			<span class="ml-auto"><PlayButton size="sm" playing={current && !playback.paused} onclick={play} /></span>
		</div>
	</div>
</div>
