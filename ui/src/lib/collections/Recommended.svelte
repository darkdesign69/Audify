<script lang="ts">
	/**
	 * "Recommended — Based on what's in this playlist" (playlist.md §5) from the smart layer's
	 * `playlistRecommendations(id, 10, refresh)`. Each row has an outline "Add"; "Refresh" asks for
	 * the next batch. Added songs leave the list at once (Spotify does the same).
	 */
	import { untrack } from 'svelte';
	import type { SongItem } from '$lib/api';
	import * as api from '$lib/api';
	import { Skeleton } from '$lib/kit';
	import SuggestRow from './SuggestRow.svelte';

	let {
		playlistId,
		onAdd,
		version = 0
	}: {
		playlistId: string;
		/** Adds the song; resolves true when it went in. */
		onAdd: (song: SongItem) => Promise<boolean>;
		/** Bump to refetch (e.g. after the playlist's first songs arrive). */
		version?: number;
	} = $props();

	let songs = $state.raw<SongItem[]>([]);
	let basis = $state<api.Recommendation['basis']>('playlist');
	let loading = $state(true);
	let failed = $state(false);
	let refresh = $state(0);
	let adding = $state<string | null>(null);

	async function load(pid: string, r: number) {
		loading = !songs.length;
		failed = false;
		try {
			const recs = await api.playlistRecommendations(pid, 10, r);
			if (pid !== playlistId) return;
			songs = recs.map((x) => x.song);
			basis = recs[0]?.basis ?? 'playlist';
		} catch {
			if (pid === playlistId) failed = true;
		} finally {
			if (pid === playlistId) loading = false;
		}
	}

	$effect(() => {
		void version;
		const pid = playlistId;
		const r = refresh;
		untrack(() => load(pid, r));
	});

	async function add(song: SongItem) {
		adding = song.video_id;
		try {
			if (await onAdd(song)) songs = songs.filter((s) => s.video_id !== song.video_id);
		} finally {
			adding = null;
		}
	}
</script>

{#if loading || songs.length || failed}
	<section class="mt-10">
		<h2 class="text-xl font-bold text-white">Recommended</h2>
		<p class="mt-1 text-sm text-text-subdued">
			{basis === 'listening' ? 'Based on what you listen to' : "Based on what's in this playlist"}
		</p>
		<div class="mt-4">
			{#if loading}
				<Skeleton variant="row" count={5} />
			{:else if failed}
				<p class="py-4 text-sm text-text-subdued">Couldn't load recommendations.</p>
			{:else}
				<div role="grid" aria-label="Recommended">
					{#each songs as s (s.video_id)}
						<SuggestRow song={s} onAdd={add} adding={adding === s.video_id} />
					{/each}
				</div>
			{/if}
		</div>
		<div class="mt-4 flex justify-end">
			<button
				type="button"
				class="text-sm font-bold text-text-subdued hover:scale-[1.04] hover:text-white"
				onclick={() => refresh++}>Refresh</button
			>
		</div>
	</section>
{/if}
