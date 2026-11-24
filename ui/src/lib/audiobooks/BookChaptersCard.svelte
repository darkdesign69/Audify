<script lang="ts">
	/**
	 * stream: audiobooks — Now Playing's card for a book chapter (#1f1f1f, radius 8, padding 16):
	 * the book's chapters around the playing one, "Show all" for the rest; a row plays that chapter.
	 * Renders nothing for anything that isn't a book chapter.
	 */
	import { audiobookForChapter, bookHref, playBook, type AudiobookPage } from '$lib/audiobooks.svelte';
	import ChapterRow from './ChapterRow.svelte';

	let { id }: { id: string } = $props();

	let data = $state.raw<AudiobookPage | null>(null);
	let expanded = $state(false);
	$effect(() => {
		const cid = id;
		if (data?.chapters.some((c) => c.id === cid)) return; // same book: keep the list
		data = null;
		expanded = false;
		if (!cid.startsWith('PODURL:')) return;
		let stale = false;
		audiobookForChapter(cid)
			.then((p) => !stale && (data = p))
			.catch(() => {});
		return () => (stale = true);
	});

	const active = $derived(data ? Math.max(0, data.chapters.findIndex((c) => c.id === id)) : 0);
	const start = $derived(expanded || !data ? 0 : Math.max(0, Math.min(active, data.chapters.length - 3)));
	const shown = $derived(data ? (expanded ? data.chapters : data.chapters.slice(start, start + 3)) : []);
</script>

{#if data && data.chapters.length > 1}
	<div class="rounded-lg bg-bg-highlight p-4" data-testid="np-book-chapters">
		<div class="flex items-center justify-between gap-2 pb-2">
			<div class="min-w-0">
				<h3 class="text-base font-bold text-white">Chapters</h3>
				<a href={bookHref(data.book.id)} class="block truncate text-sm text-text-subdued hover:text-white hover:underline"
					>{data.book.title} • {active + 1} of {data.chapters.length}</a
				>
			</div>
			{#if data.chapters.length > 3}
				<button
					type="button"
					class="shrink-0 text-sm font-bold text-text-subdued hover:text-white hover:underline"
					onclick={() => (expanded = !expanded)}>{expanded ? 'Show less' : 'Show all'}</button
				>
			{/if}
		</div>
		<div class="-mx-2">
			{#each shown as ch, i (ch.id)}
				<ChapterRow ep={ch} n={start + i + 1} compact onPlay={() => playBook(data!, start + i)} />
			{/each}
		</div>
	</div>
{/if}
