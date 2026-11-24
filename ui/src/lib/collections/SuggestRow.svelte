<script lang="ts">
	/**
	 * One row of "Recommended" / "Let's find something for your playlist" (playlist.md §5):
	 * 56px, grid `title 4fr | album 2fr | button`, no index column. The cover shows a play glyph on
	 * hover; the outline "Add" pill adds the song to the playlist.
	 */
	import type { SongItem } from '$lib/api';
	import { displayRuns } from '$lib/artistruns';
	import { thumb } from '$lib/thumb';
	import { Icon, ExplicitBadge, openTrackMenu, actions } from '$lib/kit';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import MusicNoteIcon from 'phosphor-svelte/lib/MusicNoteIcon';
	import * as api from '$lib/api';

	let {
		song,
		onAdd,
		adding = false
	}: { song: SongItem; onAdd: (song: SongItem) => void; adding?: boolean } = $props();

	const current = $derived(actions.playback.now?.videoId === song.video_id);
	const playing = $derived(current && !actions.playback.paused);
	const runs = $derived(displayRuns(song.artist_runs, song.artists, song.artist_id));
	const stop = (e: Event) => e.stopPropagation();

	function play() {
		if (current) api.togglePause();
		else actions.playTracks([song], 0);
	}
</script>

<div
	role="row"
	tabindex="-1"
	class="group grid h-14 items-center gap-4 rounded-sm px-4 hover:bg-bg-tinted"
	style="grid-template-columns:minmax(120px,1fr) minmax(120px,1fr) minmax(96px,auto)"
	ondblclick={play}
	oncontextmenu={(e) => {
		e.preventDefault();
		openTrackMenu(e, [song]);
	}}
>
	<div role="gridcell" class="flex min-w-0 items-center gap-3">
		<button
			type="button"
			class="relative grid size-10 shrink-0 place-items-center overflow-hidden rounded-sm bg-bg-menu"
			aria-label={playing ? `Pause ${song.title}` : `Play ${song.title}`}
			onclick={play}
			ondblclick={stop}
		>
			{#if song.thumbnail}
				<img src={thumb(song.thumbnail, 80)} alt="" loading="lazy" class="size-full object-cover" draggable="false" />
			{:else}
				<Icon icon={MusicNoteIcon} size={16} class="text-text-subdued" />
			{/if}
			<span
				class="absolute inset-0 grid place-items-center bg-black/50 text-white opacity-0 group-hover:opacity-100"
			>
				<Icon icon={playing ? PauseIcon : PlayIcon} size={16} fill />
			</span>
		</button>
		<div class="min-w-0">
			<div class="truncate text-base leading-[22px] {current ? 'text-accent-text' : 'text-white'}">
				{song.title}
			</div>
			<div class="flex min-w-0 items-center gap-1 text-sm leading-5 text-text-subdued group-hover:text-white">
				{#if song.explicit}<ExplicitBadge />{/if}
				<span class="truncate">
					{#each runs as r, i (i)}{#if i > 0}{", "}{/if}{#if r.id}<a href={actions.artistHref(r.id)} class="link-sub" onclick={stop}>{r.text}</a
							>{:else}{r.text}{/if}{/each}
				</span>
			</div>
		</div>
	</div>
	<div role="gridcell" class="min-w-0 truncate text-sm text-text-subdued group-hover:text-white">
		{#if song.album_id && song.album}
			<a href={actions.albumHref(song.album_id)} class="link-sub" onclick={stop}>{song.album}</a>
		{:else}{song.album ?? ''}{/if}
	</div>
	<div role="gridcell" class="flex justify-end">
		<button
			type="button"
			class="btn-outline"
			disabled={adding}
			onclick={() => onAdd(song)}
			ondblclick={stop}>Add</button
		>
	</div>
</div>
