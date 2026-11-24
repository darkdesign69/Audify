<script lang="ts">
	/**
	 * ── lane magic-d1 ── Spotify's Credits dialog (MAGIC-GAPS #6): the song, then each role YouTube
	 * Music credits ("Performed by", "Written by", "Produced by") with its people, then "Source".
	 * A name YouTube links to a channel opens that artist and can be followed; any other name
	 * searches for it. Data: `song_credits` (crates/innertube/src/credits.rs).
	 */
	import { goto } from '$app/navigation';
	import type { BrowseItem } from '$lib/api';
	import { songCredits, type SongCredits } from '$lib/trackapi';
	import { friendlyError } from '$lib/neterr';
	import Modal from './Modal.svelte';
	import Skeleton from './Skeleton.svelte';
	import { credits } from './credits.svelte';
	import { artistHref, isInLibrary, toggleInLibrary } from './actions';

	/** One fetch per song per session: credits don't change while the app is open. */
	const cache = new Map<string, SongCredits>();

	let data = $state<SongCredits | null>(null);
	let error = $state<string | null>(null);

	$effect(() => {
		const song = credits.song;
		if (!song || !credits.open) return;
		const hit = cache.get(song.video_id);
		data = hit ?? null;
		error = null;
		if (hit) return;
		songCredits($state.snapshot(song))
			.then((c) => {
				cache.set(song.video_id, c);
				if (credits.song?.video_id === song.video_id) data = c;
			})
			.catch((e) => {
				console.warn('[credits]', e);
				if (credits.song?.video_id === song.video_id)
					error = friendlyError(e, "Couldn't load the credits");
			});
	});

	function openName(n: { name: string; artistId?: string }) {
		credits.open = false;
		goto(n.artistId ? artistHref(n.artistId) : `/search?q=${encodeURIComponent(n.name)}`);
	}
	const artistItem = (n: { name: string; artistId?: string }): BrowseItem => ({
		kind: 'artist',
		id: n.artistId!,
		title: n.name
	});
</script>

<Modal bind:open={credits.open} title="Credits" width={480}>
	{#if credits.song}
		<div class="mb-6">
			<div class="truncate text-base font-bold text-white">{credits.song.title}</div>
			<div class="truncate text-sm text-text-subdued">{credits.song.artists}</div>
		</div>
	{/if}
	{#if error}
		<p class="text-sm text-text-subdued">{error}</p>
	{:else if !data}
		<Skeleton variant="row" count={4} />
	{:else if !data.sections.length}
		<p class="text-sm text-text-subdued">YouTube Music has no credits for this song.</p>
	{:else}
		<div class="flex flex-col gap-6" data-testid="credits">
			{#each data.sections as g (g.title)}
				<section>
					<h3 class="mb-2 text-base font-bold text-white">{g.title}</h3>
					<ul class="flex flex-col gap-1">
						{#each g.people as n (n.name)}
							<li class="flex min-h-10 items-center justify-between gap-4">
								<button
									type="button"
									class="min-w-0 truncate text-left text-base text-white hover:underline"
									onclick={() => openName(n)}
								>
									{n.name}
								</button>
								{#if n.artistId}
									{@const following = isInLibrary(artistItem(n))}
									<button
										type="button"
										class="shrink-0 rounded-full border border-white/40 px-3 py-1 text-sm font-bold text-white transition-transform duration-100 hover:scale-[1.04] hover:border-white"
										onclick={() => toggleInLibrary(artistItem(n))}
									>
										{following ? 'Following' : 'Follow'}
									</button>
								{/if}
							</li>
						{/each}
					</ul>
				</section>
			{/each}
			{#if data.source}
				<section>
					<h3 class="mb-1 text-base font-bold text-white">Source</h3>
					<p class="text-sm text-text-subdued">{data.source}</p>
				</section>
			{/if}
		</div>
	{/if}
</Modal>
