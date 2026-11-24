<script lang="ts">
	/**
	 * stream: chapters. Now Playing's Chapters card (#1f1f1f, radius 8, padding 16): the playing
	 * chapter and the next two, "Show all" for the rest. Hidden when the episode has no chapters.
	 */
	import { nowChapterIndex, nowChapters, seekChapter } from '$lib/chapters.svelte';
	import ChapterRows from './ChapterRows.svelte';

	let expanded = $state(false);
	const list = $derived(nowChapters.list);
	const active = $derived(nowChapterIndex());
	const start = $derived(expanded ? 0 : Math.max(0, Math.min(active, list.length - 3)));
	const shown = $derived(expanded ? list : list.slice(start, start + 3));
</script>

{#if list.length}
	<div class="rounded-lg bg-bg-highlight p-4" data-testid="np-chapters">
		<div class="flex items-center justify-between gap-2 pb-2">
			<h3 class="text-base font-bold text-white">Chapters</h3>
			{#if list.length > 3}
				<button
					type="button"
					class="text-sm font-bold text-text-subdued hover:text-white hover:underline"
					onclick={() => (expanded = !expanded)}>{expanded ? 'Show less' : 'Show all'}</button
				>
			{/if}
		</div>
		<div class="-mx-2">
			<ChapterRows chapters={shown} {active} offset={start} onpick={seekChapter} />
		</div>
	</div>
{/if}
