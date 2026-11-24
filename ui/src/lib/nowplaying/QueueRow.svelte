<script lang="ts">
	/**
	 * Queue row (queue-panel.md "Row"): 64px, padding 8, radius 4; cover 48 radius 4, gap 12; title
	 * 16/400 white (playing = accent), second line E badge · video badge · artists 14px #b3b3b3.
	 * Hover: bg white/10, cover dims with a white play glyph, "…" at the right. Rows that playback
	 * steps over (unavailable / explicit filter) are dimmed and inert, with a tooltip.
	 * The parent owns drag/drop, selection and keyboard; this is presentational + its own clicks.
	 */
	import type { SongItem, SkipReason } from '$lib/api';
	import { thumb } from '$lib/thumb';
	import Icon from '$lib/kit/Icon.svelte';
	import Tooltip from '$lib/kit/Tooltip.svelte';
	import ExplicitBadge from '$lib/kit/ExplicitBadge.svelte';
	import { artistHref, songArtists } from '$lib/kit/actions';
	import { showsVideoLabel } from '$lib/queue';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import MonitorPlayIcon from 'phosphor-svelte/lib/MonitorPlayIcon';

	let {
		song,
		current = false,
		paused = false,
		selected = false,
		skip,
		onplay,
		onmenu,
		onselect,
		index,
		reason
	}: {
		song: SongItem;
		/** The playing row: accent title, equaliser over the cover. */
		current?: boolean;
		paused?: boolean;
		selected?: boolean;
		skip?: SkipReason;
		onplay?: () => void;
		onmenu?: (e: MouseEvent | HTMLElement) => void;
		onselect?: (e: MouseEvent) => void;
		/** a11y: queue index, exposed as data-queue-index for the panel's keyboard handling. */
		index?: number;
		/** Why a predicted podcast episode is up next ("Next episode of …"): replaces the artist line. */
		reason?: string;
	} = $props();

	const inert = $derived(!!skip || !!song.unavailable);
	const why = $derived(
		skip === 'explicit' ? 'Explicit content is turned off' : inert ? 'This song is not available' : null
	);
	const artists = $derived(songArtists(song));
</script>

<Tooltip text={why} side="top">
	{#snippet trigger(props)}
		<div
			{...props}
			role="row"
			tabindex="-1"
			aria-selected={selected}
			aria-disabled={inert || undefined}
			aria-label="{song.title}, {song.artists}"
			data-queue-row
			data-queue-index={index}
			class="group/qr flex h-16 items-center gap-3 rounded-sm p-2 select-none {selected
				? 'bg-bg-selected'
				: 'hover:bg-bg-tinted'} [&[data-menu-open]]:bg-bg-tinted"
			onclick={(e) => onselect?.(e)}
			ondblclick={() => !inert && onplay?.()}
			oncontextmenu={(e) => onmenu?.(e)}
			onkeydown={(e) => {
				// a11y: ⌥J / context-menu key / ⇧F10 open the row menu; Enter plays.
				if (e.target !== e.currentTarget) return;
				if ((e.altKey && !e.metaKey && e.code === 'KeyJ') || e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) {
					e.preventDefault();
					e.stopPropagation();
					onmenu?.(e.currentTarget as HTMLElement);
				}
			}}
		>
			<div role="gridcell" class="contents">
			<div class="relative size-12 shrink-0 overflow-hidden rounded-sm bg-bg-menu {inert ? 'opacity-40' : ''}">
				{#if song.thumbnail}
					<img src={thumb(song.thumbnail, 96)} alt="" class="size-full object-cover" loading="lazy" />
				{/if}
				{#if !inert}
					<button
						type="button"
						aria-label={current && !paused ? `Pause ${song.title}` : `Play ${song.title}`}
						class="absolute inset-0 grid place-items-center bg-black/50 text-white opacity-0 group-hover/qr:opacity-100 focus-visible:opacity-100"
						onclick={(e) => {
							e.stopPropagation();
							onplay?.();
						}}
					>
						<Icon icon={current && !paused ? PauseIcon : PlayIcon} size={16} fill />
					</button>
				{/if}
			</div>
			<div class="min-w-0 flex-1 {inert ? 'opacity-40' : ''}">
				<div class="flex items-center gap-2">
					<span class="truncate text-base leading-[22px] {current ? 'text-accent-text' : 'text-white'}">
						{song.title}
					</span>
				</div>
				<div class="flex min-w-0 items-center gap-1 text-sm {selected ? 'text-white' : 'text-text-subdued'} group-hover/qr:text-white">
					{#if song.explicit}<ExplicitBadge />{/if}
					{#if showsVideoLabel(song, current)}
						<Icon icon={MonitorPlayIcon} size={14} class="shrink-0" />
						<span class="shrink-0">Music video •</span>
					{/if}
					{#if reason}
						<span class="truncate">{reason}</span>
					{:else}
					<span class="truncate">
						{#each artists as a, i (i)}
							{#if i > 0}, {/if}
							{#if a.id}
								<a
									href={artistHref(a.id)}
									class="hover:text-white hover:underline"
									onclick={(e) => e.stopPropagation()}>{a.name}</a
								>
							{:else}{a.name}{/if}
						{/each}
					</span>
					{/if}
				</div>
			</div>
			<button
				type="button"
				aria-label="More options for {song.title}"
				class="grid size-8 shrink-0 place-items-center rounded-full text-text-subdued opacity-0 group-hover/qr:opacity-100 hover:text-white focus-visible:opacity-100"
				onclick={(e) => {
					e.stopPropagation();
					onmenu?.(e);
				}}
			>
				<Icon icon={DotsThreeIcon} size={16} weight="bold" />
			</button>
			</div>
		</div>
	{/snippet}
</Tooltip>
