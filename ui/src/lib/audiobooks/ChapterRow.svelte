<script lang="ts">
	/**
	 * stream: audiobooks — one chapter of a book (book page, Now Playing's chapter card). Spotify's
	 * audiobook chapter row: number (equalizer while playing), title, "18 min" / "5 min left" with a
	 * thin progress bar, an accent check once heard, the play circle on the right.
	 */
	import type { PodcastEpisode } from '$lib/api';
	import { Equalizer, Icon, Tooltip } from '$lib/kit';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import { isEpisodeCurrent, isEpisodePlaying, spokenDuration } from '$lib/artistpods/pods';
	import { chapterProgress } from '$lib/audiobooks.svelte';

	let {
		ep,
		n,
		compact = false,
		onPlay
	}: { ep: PodcastEpisode; n: number; compact?: boolean; onPlay: () => void } = $props();

	const p = $derived(chapterProgress(ep));
	const dur = $derived(p.durationSecs || ep.durationSecs || 0);
	const started = $derived(!p.played && p.positionSecs >= 10 && dur > 0);
	const current = $derived(isEpisodeCurrent(ep.id));
	const playing = $derived(isEpisodePlaying(ep.id));
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group/ch flex items-center gap-4 rounded-md {compact ? 'px-2 py-2' : 'px-3 py-3'} transition-colors duration-100 hover:bg-bg-tinted"
	ondblclick={onPlay}
	data-chapter={n}
>
	<div class="grid w-6 shrink-0 place-items-center text-base tabular-nums {current ? 'text-accent-text' : 'text-text-subdued'}">
		{#if playing}<Equalizer />{:else}{n}{/if}
	</div>
	<div class="min-w-0 flex-1">
		<div class="truncate text-base {current ? 'text-accent-text' : 'text-white'}">{ep.title}</div>
		<div class="mt-1 flex items-center gap-2 text-sm text-text-subdued">
			{#if p.played}
				<Tooltip text="Finished">
					{#snippet trigger(props)}
						<span {...props} class="shrink-0 text-accent"><Icon icon={CheckCircleIcon} size={16} fill /></span>
					{/snippet}
				</Tooltip>
			{/if}
			<span class="truncate">{started ? `${spokenDuration(dur - p.positionSecs)} left` : spokenDuration(dur)}</span>
			{#if started}
				<div class="h-1 w-[85px] shrink-0 overflow-hidden rounded-sm bg-white/30" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p.fraction * 100)} aria-label={`${Math.round(p.fraction * 100)}% heard`}>
					<div class="h-full rounded-sm bg-white" style="width:{Math.min(100, p.fraction * 100)}%"></div>
				</div>
			{/if}
		</div>
	</div>
	<button
		type="button"
		class="grid size-8 shrink-0 place-items-center rounded-full bg-white text-black transition-transform hover:scale-[1.04] {current ? '' : 'opacity-0 group-hover/ch:opacity-100 focus-visible:opacity-100'}"
		aria-label={playing ? `Pause ${ep.title}` : `Play ${ep.title}`}
		onclick={onPlay}
	>
		<Icon icon={playing ? PauseIcon : PlayIcon} size={16} fill />
	</button>
</div>
