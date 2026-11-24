<script lang="ts">
	/**
	 * One 56px track row (spec playlist.md §3). Presentational: TrackTable owns selection, menus,
	 * drag and playback; the row reports gestures through callbacks. Usable on its own (e.g. a
	 * Recommended list) by passing `columns` / `template` from `trackTable.ts`.
	 *
	 * Props:
	 * - song: SongItem; number: shown in the # cell (1-based); columns: TrackColumn[]; template:
	 *   grid-template-columns string; variant: TrackVariant (album rows have no artwork)
	 * - selected, current (this row is the playing track), paused (playback paused), focused
	 * - dropLine: 'top' | 'bottom' | null (accent 2px insertion line while reordering)
	 * - rowIndex: aria-rowindex
	 * - onselect(e: MouseEvent) single click; onplay() double-click / hover play glyph;
	 *   ontogglepause() hover pause glyph on the playing row; oncontextmenu(e); onmenu(el: HTMLElement)
	 *   "…" button; ondragstart(e: DragEvent); ondragover(e) / ondrop(e) for reorder targets
	 */
	import type { SongItem } from '$lib/api';
	import { isDownloaded } from '$lib/downloads.svelte'; // stream: downloads
	import ArrowCircleDownIcon from 'phosphor-svelte/lib/ArrowCircleDownIcon';
	import { displayRuns } from '$lib/artistruns';
	import { thumb } from '$lib/thumb';
	import Icon from './Icon.svelte';
	import Tooltip from './Tooltip.svelte';
	import Equalizer from './Equalizer.svelte';
	import ExplicitBadge from './ExplicitBadge.svelte';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import MusicNoteIcon from 'phosphor-svelte/lib/MusicNoteIcon';
	import { albumHref, artistHref, isLiked } from './actions';
	import { openSavedIn } from './menus/definitions';
	import { formatAdded, type TrackColumn, type TrackVariant } from './trackTable';

	let {
		song,
		number,
		columns,
		template,
		variant = 'playlist',
		selected = false,
		current = false,
		paused = false,
		focused = false,
		dropLine = null,
		rowIndex,
		skipReason = null,
		onselect,
		onplay,
		ontogglepause,
		oncontextmenu,
		onmenu,
		ondragstart,
		ondragover,
		ondrop,
		ondragleave,
		compact = false,
		likedOnHover = false
	}: {
		song: SongItem;
		number: number;
		columns: TrackColumn[];
		template: string;
		variant?: TrackVariant;
		selected?: boolean;
		current?: boolean;
		paused?: boolean;
		focused?: boolean;
		dropLine?: 'top' | 'bottom' | null;
		rowIndex?: number;
		/** Why playback steps over this row (polish stream): greys it like an unavailable row. */
		skipReason?: 'explicit' | 'unavailable' | 'offline' | null;
		onselect?: (e: MouseEvent) => void;
		onplay?: () => void;
		ontogglepause?: () => void;
		oncontextmenu?: (e: MouseEvent) => void;
		onmenu?: (el: HTMLElement) => void;
		ondragstart?: (e: DragEvent) => void;
		ondragover?: (e: DragEvent) => void;
		ondrop?: (e: DragEvent) => void;
		ondragleave?: (e: DragEvent) => void;
		/** 32px row, no artwork; artists move to the 'artist' column. */
		compact?: boolean;
		/** Liked Songs: every row is liked, so the accent check shows on hover only (Spotify). */
		likedOnHover?: boolean;
	} = $props();

	const unavailable = $derived(!!song.unavailable || !!skipReason);
	const unavailableText = $derived(
		skipReason === 'explicit'
			? 'Explicit content is turned off'
			: skipReason === 'offline'
				? 'Not available offline'
				: 'This song is not available'
	);
	// stream: downloads — Spotify's small accent arrow in front of the artist line.
	const downloaded = $derived(isDownloaded(song.video_id));
	const liked = $derived(isLiked(song));
	const playing = $derived(current && !paused);
	const hasArt = $derived(variant !== 'album' && !compact);
	const runs = $derived(displayRuns(song.artist_runs, song.artists, song.artist_id));
	// Hover and selection reveal the same things (spec: "Selected … same reveals as hover").
	const reveal = 'opacity-0 group-hover:opacity-100 group-data-[selected=true]:opacity-100';
	const sub =
		'text-text-subdued group-hover:text-white group-data-[selected=true]:text-white';
	let moreBtn = $state<HTMLButtonElement | null>(null);
	// stream: tilefix — art that fails to load shows the placeholder note, not a broken image.
	let failedArt = $state<string | null | undefined>(null);
	const stop = (e: Event) => e.stopPropagation();
</script>

<Tooltip text={unavailable ? unavailableText : null} side="top">
	{#snippet trigger(props)}
		<div
			{...props}
			role="row"
			tabindex="-1"
			aria-selected={selected}
			aria-rowindex={rowIndex}
			aria-disabled={unavailable || undefined}
			data-selected={selected}
			data-track-row
			draggable={!unavailable}
			class="group relative grid {compact ? 'h-8' : 'h-14'} select-none items-center gap-4 rounded-sm px-4 {selected
				? 'bg-bg-selected'
				: 'hover:bg-bg-tinted'} {focused ? 'outline outline-1 -outline-offset-1 outline-white/40' : ''}"
			style="grid-template-columns:{template}"
			onclick={(e) => onselect?.(e)}
			ondblclick={() => !unavailable && onplay?.()}
			oncontextmenu={(e) => {
				e.preventDefault();
				oncontextmenu?.(e);
			}}
			{ondragstart}
			{ondragover}
			{ondrop}
			{ondragleave}
		>
			{#if dropLine}
				<span
					class="pointer-events-none absolute inset-x-0 h-0.5 bg-accent {dropLine === 'top'
						? '-top-px'
						: '-bottom-px'}"
				></span>
			{/if}

			{#each columns as col (col)}
				{#if col === 'index'}
					<div role="gridcell" class="relative grid h-4 w-4 place-items-center justify-self-end">
						<!-- Rest state -->
						<span
							class="group-hover:invisible {unavailable ? 'opacity-40' : ''} flex items-center justify-end"
						>
							{#if playing && !unavailable}
								<Equalizer size={14} />
							{:else}
								<span
									class="text-base tabular-nums {current ? 'text-accent-text' : 'text-text-subdued'}"
									>{number}</span
								>
							{/if}
						</span>
						<!-- Hover: play / pause glyph -->
						{#if !unavailable}
							<button
								type="button"
								class="invisible absolute inset-0 grid place-items-center text-white group-hover:visible"
								aria-label={playing ? 'Pause' : `Play ${song.title} by ${song.artists}`}
								onclick={(e) => {
									e.stopPropagation();
									if (current) ontogglepause?.();
									else onplay?.();
								}}
								ondblclick={stop}
							>
								<Icon icon={playing ? PauseIcon : PlayIcon} size={16} fill />
							</button>
						{:else}
							<span
								class="invisible absolute inset-0 grid place-items-center text-text-subdued group-hover:visible"
								>{number}</span
							>
						{/if}
					</div>
				{:else if col === 'title'}
					<div role="gridcell" class="flex min-w-0 items-center gap-3 {unavailable ? 'opacity-40' : ''}">
						{#if hasArt}
							<div class="grid size-10 shrink-0 place-items-center overflow-hidden rounded-sm bg-bg-menu">
								{#if song.thumbnail && failedArt !== song.thumbnail}
									<img
										src={thumb(song.thumbnail, 80)}
										alt=""
										loading="lazy"
										decoding="async"
										draggable="false"
										class="size-full object-cover"
										onerror={() => (failedArt = song.thumbnail)}
									/>
								{:else}
									<Icon icon={MusicNoteIcon} size={16} class="text-text-subdued" />
								{/if}
							</div>
						{/if}
						<div class="min-w-0">
							<div
								class="truncate text-base leading-[22px] {current ? 'text-accent-text' : 'text-white'}"
								title={song.title}
							>
								{song.title}
							</div>
							{#if !compact}
															<div class="flex min-w-0 items-center gap-1 text-sm leading-5 {sub}">
									{#if downloaded}<span class="grid shrink-0 place-items-center text-accent" aria-label="Downloaded"><Icon icon={ArrowCircleDownIcon} size={12} fill /></span>{/if}
									{#if song.explicit}<ExplicitBadge />{/if}
									<span class="truncate" title={runs.map((r) => r.text).join(', ')}>
										{#each runs as r, i (i)}
											{#if i > 0}{", "}{/if}{#if r.id}<a
													href={artistHref(r.id)}
													class="link-sub"
													draggable="false"
													onclick={stop}
													ondblclick={stop}>{r.text}</a
												>{:else}{r.text}{/if}
										{/each}
									</span>
								</div>
							{/if}
						</div>
					</div>
				{:else if col === 'artist'}
					<div role="gridcell" class="flex min-w-0 items-center gap-1 text-sm {sub} {unavailable ? 'opacity-40' : ''}">
						{#if downloaded && compact}<span class="grid shrink-0 place-items-center text-accent" aria-label="Downloaded"><Icon icon={ArrowCircleDownIcon} size={12} fill /></span>{/if}
						{#if song.explicit}<ExplicitBadge />{/if}
						<span class="truncate">
							{#each runs as r, i (i)}
								{#if i > 0}{", "}{/if}{#if r.id}<a
										href={artistHref(r.id)}
										class="link-sub"
										draggable="false"
										onclick={stop}
										ondblclick={stop}>{r.text}</a
									>{:else}{r.text}{/if}
							{/each}
						</span>
					</div>
				{:else if col === 'album'}
					<div role="gridcell" class="min-w-0 truncate text-sm {sub} {unavailable ? 'opacity-40' : ''}" title={song.album ?? undefined}>
						{#if song.album_id && song.album}
							<a
								href={albumHref(song.album_id)}
								class="link-sub"
								draggable="false"
								onclick={stop}
								ondblclick={stop}>{song.album}</a
							>
						{:else}{song.album ?? ''}{/if}
					</div>
				{:else if col === 'added'}
					<!-- Spotify keeps Date added grey on row hover (only title/artist/album turn white). -->
					<div role="gridcell" class="min-w-0 truncate text-sm text-text-subdued {unavailable ? 'opacity-40' : ''}">
						{formatAdded(song.added_at)}
					</div>
				{:else if col === 'plays'}
					<div role="gridcell" class="min-w-0 truncate text-sm tabular-nums {sub} {unavailable ? 'opacity-40' : ''}">
						{song.play_count ?? ''}
					</div>
				{:else if col === 'duration'}
					<div role="gridcell" class="flex items-center justify-end gap-4">
						{#if !unavailable}
							<Tooltip text={liked ? 'Add to playlist' : 'Add to Liked Songs'}>
								{#snippet trigger(p)}
									<button
										{...p}
										type="button"
										aria-label={liked ? 'Add to playlist' : 'Add to Liked Songs'}
										class="grid size-4 place-items-center transition-transform duration-100 hover:scale-[1.04] {liked
											? `text-accent ${likedOnHover ? reveal : ''}`
											: `text-text-subdued hover:text-white ${reveal}`}"
										onclick={(e) => {
											e.stopPropagation();
											openSavedIn(e, song);
										}}
										ondblclick={stop}
									>
										<Icon icon={liked ? CheckCircleIcon : PlusCircleIcon} size={16} fill={liked} />
									</button>
								{/snippet}
							</Tooltip>
						{:else}
							<span class="size-4"></span>
						{/if}
						<span
							class="min-w-[2.5rem] text-right text-sm tabular-nums text-text-subdued {unavailable
								? 'opacity-40'
								: ''}">{song.duration ?? ''}</span
						>
						<Tooltip text={`More options for ${song.title} by ${song.artists}`}>
							{#snippet trigger(p)}
								<button
									{...p}
									bind:this={moreBtn}
									type="button"
									aria-label={`More options for ${song.title}`}
									class="grid size-4 place-items-center text-white transition-transform duration-100 hover:scale-[1.04] {reveal}"
									onclick={(e) => {
										e.stopPropagation();
										if (moreBtn) onmenu?.(moreBtn);
									}}
									ondblclick={stop}
								>
									<Icon icon={DotsThreeIcon} size={16} weight="bold" />
								</button>
							{/snippet}
						</Tooltip>
					</div>
				{/if}
			{/each}
		</div>
	{/snippet}
</Tooltip>
