<script lang="ts" module>
	import type { SongCredits } from '$lib/trackapi';
	/** One fetch per song per session, shared by every mount (the panel remounts per track). */
	const cache = new Map<string, Promise<SongCredits>>();
</script>

<script lang="ts">
	/**
	 * ── lane magic-d6 ── Spotify's "Credits" card in the Now Playing panel (MAGIC-GAPS #6): #1f1f1f,
	 * radius 8, padding 16. Header "Credits" 16/700 with "Show all" (opens the Credits dialog) on the
	 * right; then up to three people, each a name (16px white, a link when YouTube knows the artist)
	 * over their roles (14px #b3b3b3, "Main Artist", "Writer, Producer"), with a Follow outline pill
	 * for linked artists. One row per person: a name credited twice gets both roles. Hidden while
	 * loading and when YouTube has no credits, the way Spotify leaves the card out.
	 * Data: `song_credits` (lane magic-d1, crates/innertube/src/credits.rs).
	 */
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import type { BrowseItem, SongItem } from '$lib/api';
	import { songCredits } from '$lib/trackapi';
	import Pill from '$lib/kit/Pill.svelte';
	import { openCredits } from '$lib/kit/credits.svelte';
	import { artistHref, isInLibrary, toggleInLibrary } from '$lib/kit/actions';

	let { song }: { song: SongItem } = $props();

	let data = $state<SongCredits | null>(null);

	// Keyed on the id: the panel re-derives the song object often, and that shouldn't blank the card.
	const videoId = $derived(song.video_id);
	$effect(() => {
		videoId;
		const s = untrack(() => $state.snapshot(song)) as SongItem;
		let stale = false;
		data = null;
		let p = cache.get(s.video_id);
		if (!p) {
			p = songCredits(s);
			cache.set(s.video_id, p);
			// A failure isn't cached: the next track change (or reopen) asks again.
			p.catch(() => cache.delete(s.video_id));
		}
		p.then((c) => !stale && (data = c)).catch((e) => console.warn('[credits card]', e));
		return () => (stale = true);
	});

	const shown = $derived((data?.people ?? []).slice(0, 3));
	const artist = (p: { name: string; artistId?: string }): BrowseItem => ({ kind: 'artist', id: p.artistId!, title: p.name });
</script>

{#if shown.length}
	<section class="rounded-lg bg-bg-highlight p-4" aria-label="Credits" data-testid="credits-card">
		<div class="flex items-center justify-between gap-2 pb-2">
			<h3 class="text-base font-bold text-white">Credits</h3>
			<button
				type="button"
				class="text-sm font-bold text-text-subdued hover:text-white hover:underline"
				onclick={() => openCredits(song)}>Show all</button
			>
		</div>
		<ul class="flex flex-col">
			{#each shown as p (p.name)}
				<li class="flex min-h-14 items-center gap-3 py-1">
					<div class="min-w-0 flex-1">
						{#if p.artistId}
							<a href={artistHref(p.artistId)} class="block truncate text-base text-white hover:underline">{p.name}</a>
						{:else}
							<button
								type="button"
								class="block max-w-full truncate text-left text-base text-white hover:underline"
								onclick={() => goto(`/search?q=${encodeURIComponent(p.name)}`)}>{p.name}</button
							>
						{/if}
						<div class="truncate text-sm text-text-subdued">{p.roles.join(', ')}</div>
					</div>
					{#if p.artistId}
						{@const a = artist(p)}
						<Pill variant="outline" class="shrink-0" onclick={() => toggleInLibrary(a)}>
							{isInLibrary(a) ? 'Following' : 'Follow'}
						</Pill>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}
