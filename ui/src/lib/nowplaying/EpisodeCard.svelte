<script lang="ts">
	/**
	 * Podcast episode variant of the Now Playing view: an "About this episode" card (#1f1f1f,
	 * radius 8, padding 16) with the date · length line, the show notes (5-line clamp, Show more),
	 * and the playback speed control. Skeleton until the episode loads.
	 */
	import type * as api from '$lib/api';
	import { plainText } from './info';
	import SpeedButton from './SpeedButton.svelte';

	let { ep }: { ep: api.PodcastEpisode | null } = $props();

	let expanded = $state(false);
	const notes = $derived(plainText(ep?.description));
	const meta = $derived.by(() => {
		if (!ep) return '';
		const parts: string[] = [];
		if (ep.publishedAt)
			parts.push(
				new Date(ep.publishedAt * 1000).toLocaleDateString(undefined, {
					month: 'short',
					day: 'numeric',
					year: 'numeric'
				})
			);
		if (ep.durationSecs) {
			const m = Math.round(ep.durationSecs / 60);
			parts.push(m >= 60 ? `${Math.floor(m / 60)} hr ${m % 60} min` : `${m} min`);
		}
		return parts.join(' • ');
	});
</script>

<div class="rounded-lg bg-bg-highlight p-4">
	<h3 class="text-base font-bold text-white">About this episode</h3>
	{#if !ep}
		<div class="mt-3 space-y-2" aria-busy="true">
			<div class="h-3 w-1/3 animate-pulse rounded-full bg-white/10"></div>
			<div class="h-3 w-full animate-pulse rounded-full bg-white/10"></div>
			<div class="h-3 w-5/6 animate-pulse rounded-full bg-white/10"></div>
		</div>
	{:else}
		{#if meta}<p class="mt-1 text-sm text-text-subdued">{meta}</p>{/if}
		{#if notes}
			<p class="mt-3 text-sm whitespace-pre-line text-text-subdued {expanded ? '' : 'line-clamp-5'}">
				{notes}
			</p>
			{#if notes.length > 220}
				<button
					type="button"
					class="mt-1 text-sm font-bold text-white hover:underline"
					onclick={() => (expanded = !expanded)}>{expanded ? 'Show less' : 'Show more'}</button
				>
			{/if}
		{/if}
	{/if}
	<div class="mt-4 flex items-center justify-between">
		<span class="text-sm text-text-subdued">Playback speed</span>
		<SpeedButton variant="pill" />
	</div>
</div>
