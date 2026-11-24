<script lang="ts">
	/** A podcast episode as a Home/search card: blue new-dot + "14 Nov • 2 min" subtitle (home.md). */
	import type { PodcastEpisode } from '$lib/api';
	import Card from '$lib/kit/Card.svelte';
	import { podcasts } from '$lib/podcasts.svelte';
	import * as api from '$lib/api';
	import { playback } from '$lib/player.svelte';
	import { playEpisodes } from './play.svelte';
	import { episodeDate, episodeLength } from './format';

	let { episode, list = [episode] }: { episode: PodcastEpisode; list?: PodcastEpisode[] } = $props();
	const progress = $derived(podcasts.progress[episode.id] ?? episode.progress);
	const unplayed = $derived(!progress.played && !progress.positionSecs);
	const key = $derived(`ep:${episode.id}`);
	const current = $derived(playback.now?.videoId === episode.id);
	function play() {
		if (current) api.togglePause().catch(() => {});
		else playEpisodes(key, episode.showTitle, list, episode.thumbnail, Math.max(0, list.indexOf(episode)));
	}
	const href = $derived(`/episode/${encodeURIComponent(episode.id)}`);
</script>

<Card
	title={episode.title}
	image={episode.thumbnail}
	kind="episode"
	{href}
	playing={current && !playback.paused}
	onPlay={play}
>
	{#snippet subtitleSnippet()}
		<span class="inline-flex items-center gap-1">
			{#if unplayed}<span class="inline-block size-2 shrink-0 rounded-full bg-announcement"></span>{/if}
			{[episodeDate(episode.publishedAt), episodeLength(episode, progress)].filter(Boolean).join(' • ')}
		</span>
	{/snippet}
</Card>
