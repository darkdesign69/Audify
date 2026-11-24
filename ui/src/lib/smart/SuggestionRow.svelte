<script lang="ts">
	/**
	 * A Smart Shuffle suggestion (stream smart2): 56px row like the playlist's own, with the accent
	 * sparkle before the title and Spotify's two buttons on the right, "Add to this playlist" (+)
	 * and "Remove recommendation" (−, a "not for me" taste signal).
	 * `layout` (from the kit TrackTable's `interleave` row): the row sits inside the playlist's
	 * track list, on the table's own columns (stream wave2).
	 */
	import type { SongItem } from '$lib/api';
	import * as api from '$lib/api';
	import { displayRuns } from '$lib/artistruns';
	import { thumb } from '$lib/thumb';
	import { Icon, IconButton, ExplicitBadge, openTrackMenu, actions, type TrackColumn } from '$lib/kit';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import MinusCircleIcon from 'phosphor-svelte/lib/MinusCircleIcon';
	import MusicNoteIcon from 'phosphor-svelte/lib/MusicNoteIcon';
	import SparkleIcon from 'phosphor-svelte/lib/SparkleIcon';

	let {
		song,
		onAdd,
		onRemove,
		compact = false,
		layout
	}: {
		song: SongItem;
		onAdd?: (song: SongItem) => void;
		onRemove?: (song: SongItem) => void;
		/** Queue panel: no album column. */
		compact?: boolean;
		/** In a TrackTable: its columns and grid template. */
		layout?: { columns: TrackColumn[]; template: string };
	} = $props();

	const columns = $derived<TrackColumn[]>(
		layout?.columns ?? (compact ? ['title', 'duration'] : ['title', 'album', 'duration'])
	);
	const template = $derived(
		layout?.template ?? (compact ? 'minmax(0,1fr) auto' : 'minmax(120px,4fr) minmax(120px,2fr) auto')
	);

	const current = $derived(actions.playback.now?.videoId === song.video_id);
	const playing = $derived(current && !actions.playback.paused);
	const runs = $derived(displayRuns(song.artist_runs, song.artists, song.artist_id));

	function play() {
		if (current) {
			api.togglePause();
			return;
		}
		const i = actions.playback.queue.items.findIndex((s) => s.video_id === song.video_id);
		if (i >= 0) api.playIndex(i);
		else actions.playTracks([song], 0);
	}
</script>

<div
	role="row"
	tabindex="-1"
	data-suggestion
	class="group grid items-center gap-4 rounded-sm hover:bg-bg-tinted {compact ? 'h-16 px-2' : 'h-14 px-4'}"
	style="grid-template-columns:{template}"
	ondblclick={play}
	oncontextmenu={(e) => {
		e.preventDefault();
		openTrackMenu(e, [song]);
	}}
>
	{#each columns as col (col)}
		{#if col === 'index'}
			<div role="gridcell"></div>
		{:else if col === 'title'}
			<div role="gridcell" class="flex min-w-0 items-center gap-3">
				<button
					type="button"
					class="relative grid shrink-0 place-items-center overflow-hidden rounded-sm bg-bg-menu {compact ? 'size-12' : 'size-10'}"
					aria-label={playing ? `Pause ${song.title}` : `Play ${song.title}`}
					onclick={play}
				>
					{#if song.thumbnail}
						<img src={thumb(song.thumbnail, 80)} alt="" class="size-full object-cover" />
					{:else}
						<Icon icon={MusicNoteIcon} class="text-text-subdued" />
					{/if}
					<span class="absolute inset-0 hidden place-items-center bg-black/50 text-white group-hover:grid">
						<Icon icon={playing ? PauseIcon : PlayIcon} fill />
					</span>
				</button>
				<div class="min-w-0">
					<div class="flex min-w-0 items-center gap-1.5">
						<span class="shrink-0 text-accent" title="Smart Shuffle suggestion">
							<Icon icon={SparkleIcon} size={12} fill label="Smart Shuffle suggestion" />
						</span>
						<span class="truncate text-base {current ? 'text-accent-text' : 'text-white'}">{song.title}</span>
					</div>
					{#if !columns.includes('artist')}
						<div class="flex min-w-0 items-center gap-1 text-sm text-text-subdued">
							{#if song.explicit}<ExplicitBadge />{/if}
							<span class="truncate">
								{#each runs as r, i (i)}{#if r.id}<a href={actions.artistHref(r.id)} class="hover:text-white hover:underline">{r.text}</a>{:else}{r.text}{/if}{/each}
							</span>
						</div>
					{/if}
				</div>
			</div>
		{:else if col === 'artist'}
			<div role="gridcell" class="min-w-0 truncate text-sm text-text-subdued">
				{#each runs as r, i (i)}{#if r.id}<a href={actions.artistHref(r.id)} class="hover:text-white hover:underline">{r.text}</a>{:else}{r.text}{/if}{/each}
			</div>
		{:else if col === 'album'}
			<div role="gridcell" class="min-w-0 truncate text-sm text-text-subdued">
				{#if song.album_id && song.album}
					<a href={actions.albumHref(song.album_id)} class="hover:text-white hover:underline">{song.album}</a>
				{:else}{song.album ?? ''}{/if}
			</div>
		{:else if col === 'duration'}
			<div role="gridcell" class="flex items-center justify-end gap-1">
				{#if onAdd}
					<IconButton icon={PlusCircleIcon} label="Add to this playlist" onclick={() => onAdd(song)} />
				{/if}
				{#if onRemove}
					<IconButton icon={MinusCircleIcon} label="Remove recommendation" onclick={() => onRemove(song)} />
				{/if}
			</div>
		{:else}
			<div role="gridcell"></div>
		{/if}
	{/each}
</div>
