<script lang="ts">
	/**
	 * stream: chapters. Chapter rows (episode page and the Now Playing card): optional 40px art,
	 * title (accent while it plays) and its start time. A tap jumps there.
	 */
	import type * as api from '$lib/api';
	import { thumb } from '$lib/thumb';

	let {
		chapters,
		active = -1,
		offset = 0,
		onpick
	}: {
		chapters: api.PodcastChapter[];
		/** Index (in the full list) of the chapter playing, -1 when none. */
		active?: number;
		/** Index of `chapters[0]` in the full list, when showing a slice. */
		offset?: number;
		onpick: (c: api.PodcastChapter) => void;
	} = $props();
</script>

<ol class="flex flex-col" aria-label="Chapters">
	{#each chapters as c, i (c.startSecs)}
		{@const on = offset + i === active}
		<li>
			<button
				type="button"
				class="flex w-full items-center gap-3 rounded px-2 py-2 text-left hover:bg-white/10 focus-visible:bg-white/10"
				aria-current={on || undefined}
				onclick={() => onpick(c)}
			>
				{#if c.img}
					<img src={thumb(c.img, 80)} alt="" class="size-10 shrink-0 rounded object-cover" loading="lazy" />
				{/if}
				<span class="min-w-0 flex-1">
					<span class="block truncate text-base {on ? 'text-accent-text' : 'text-white'}">{c.title}</span>
					<span class="block text-sm text-text-subdued tabular-nums">{c.startLabel}</span>
				</span>
			</button>
		</li>
	{/each}
</ol>
