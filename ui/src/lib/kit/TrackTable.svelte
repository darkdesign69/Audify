<script lang="ts">
	/**
	 * Spotify track table (spec playlist.md §3, album.md, artist.md "Popular").
	 *
	 * Props:
	 * - tracks: SongItem[]
	 * - variant: 'playlist' | 'album' | 'artist' | 'queue' | 'search' (column sets in trackTable.ts;
	 *   narrow widths drop "Date added", then "Album"/"Plays")
	 * - context?: { id?, title?, kind? } → actions.playTracks(tracks, index, context) on double-click
	 * - menuContext?: TrackMenuContext → passed to openTrackMenu (right-click, "…", ⌥J)
	 * - showHeader = true (false by default for variant 'artist')
	 * - stickyTop?: number → header sticks at this offset; when stuck it gets bg #1f1f1f, bottom
	 *   border and column dividers. `bleed` (CSS length, default '0px') widens the stuck header by
	 *   that much on each side (pass 'var(--content-spacing)' when the table sits in the gutter).
	 * - sort (bindable TrackSort) + onSort?(sort): header click cycles asc → desc → none. With
	 *   onSort the parent re-orders `tracks` itself (server sort); without it the table sorts locally.
	 * - startIndex = 1 (first number shown)
	 * - isPlaying?: (song, index) => boolean (default: playback.now?.videoId === song.video_id)
	 * - onReorder?(from: number[], to: number): drag reorder (indices into `tracks`; `to` = insert
	 *   before that index, tracks.length = end). Only offered while unsorted.
	 * - acceptDrop?(e) / onDropIn?(e, to): songs or cards dragged in from elsewhere (lane magic-d2,
	 *   the playlist page). `acceptDrop` says during dragover whether this drag may land; the same
	 *   accent line as a reorder marks the spot, and `to` indexes `tracks` like onReorder's.
	 * - scroller?: HTMLElement → windowed rendering (fixed 56px rows) for long lists
	 * - selected (bindable SvelteSet<number> of indices into `tracks`)
	 * - empty?: Snippet shown when there are no tracks
	 * - onPlay?: (list, pos) => void  replaces the default double-click/Enter play (e.g. to pass a
	 *   polish `PlayContext` via `api.playPlaylistFrom`, or a source id that isn't a YouTube list)
	 * - compact = false → Spotify's "View as ▸ Compact": 32px rows, no artwork, Artist column
	 * - interleave?: { every, count, height = 56, row } → extra rows mixed into the list (Smart
	 *   Shuffle suggestions): `row(k, layout)` renders the k-th after every `every` tracks, up to
	 *   `count`; `layout` = { columns, template } to line its cells up with the table's. They
	 *   aren't tracks (no number, selection or keyboard focus); the page owns their behaviour.
	 */
	import { onMount, type Snippet } from 'svelte';
	import { prefs } from '$lib/player.svelte';
	import { downloads, isDownloaded } from '$lib/downloads.svelte'; // stream: downloads
	import { SvelteSet } from 'svelte/reactivity';
	import type { BrowseItem, SkipReason, SongItem } from '$lib/api';
	import * as api from '$lib/api';
	import Icon from './Icon.svelte';
	import ClockIcon from 'phosphor-svelte/lib/ClockIcon';
	import CaretUpIcon from 'phosphor-svelte/lib/CaretUpIcon';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';
	import TrackRow from './TrackRow.svelte';
	import { isOwnPlaylist, playback, playTracks, removeFromPlaylist, type PlayTracksContext } from './actions';
	import { openTrackMenu } from './menus';
	import type { TrackMenuContext } from './menus/types';
	import {
		COLUMN_LABEL,
		SORTABLE,
		gridTemplate,
		nextSort,
		setDragSongs,
		sortedOrder,
		visibleColumns,
		type TrackColumn,
		type TrackSort,
		type TrackVariant
	} from './trackTable';

	let {
		tracks,
		variant = 'playlist',
		context,
		menuContext,
		showHeader = variant !== 'artist',
		stickyTop,
		bleed = '0px',
		sort = $bindable(null),
		onSort,
		startIndex = 1,
		isPlaying,
		onReorder,
		acceptDrop,
		onDropIn,
		scroller,
		skipReasons,
		selected = $bindable(new SvelteSet<number>()),
		empty,
		onPlay,
		compact = false,
		likedOnHover = false,
		interleave
	}: {
		tracks: SongItem[];
		variant?: TrackVariant;
		context?: PlayTracksContext;
		menuContext?: TrackMenuContext;
		showHeader?: boolean;
		stickyTop?: number;
		bleed?: string;
		sort?: TrackSort;
		onSort?: (sort: TrackSort) => void;
		startIndex?: number;
		isPlaying?: (song: SongItem, index: number) => boolean;
		onReorder?: (from: number[], to: number) => void;
		acceptDrop?: (e: DragEvent) => boolean;
		onDropIn?: (e: DragEvent, to: number) => void;
		scroller?: HTMLElement | null;
		/** videoId → skip reason. Omit to have the table ask `playbackSkipReasons` itself. */
		skipReasons?: Record<string, SkipReason>;
		selected?: SvelteSet<number>;
		empty?: Snippet;
		onPlay?: (list: SongItem[], pos: number) => void;
		compact?: boolean;
		/** Liked Songs: the liked check shows on row hover only. */
		likedOnHover?: boolean;
		interleave?: {
			every: number;
			count: number;
			height?: number;
			row: Snippet<[number, { columns: TrackColumn[]; template: string }]>;
		};
	} = $props();

	// Interleaved rows: one after display position `pos` when (pos + 1) is a multiple of `every`.
	const mixEvery = $derived(interleave && interleave.every > 0 ? interleave.every : 0);
	const mixHeight = $derived(interleave?.height ?? 56);
	const mixedBefore = (pos: number) =>
		mixEvery ? Math.min(interleave!.count, Math.floor(pos / mixEvery)) : 0;
	const mixedAfter = (pos: number) =>
		mixEvery && (pos + 1) % mixEvery === 0 && (pos + 1) / mixEvery <= interleave!.count
			? (pos + 1) / mixEvery - 1
			: -1;
	/** Offset of display position `pos` from the top of the body (windowing). */
	const yOf = (pos: number) => pos * ROW + mixedBefore(pos) * mixHeight;

	// Rows playback will step over (explicit filter, unresolvable): greyed like Spotify.
	let fetchedReasons = $state<Record<string, SkipReason>>({});
	const baseReasons = $derived(skipReasons ?? fetchedReasons);
	// stream: downloads — offline, everything that isn't downloaded (or a local file) is greyed.
	const reasons = $derived.by(() => {
		if (!downloads.network.offline) return baseReasons;
		const out: Record<string, SkipReason> = { ...baseReasons };
		for (const t of tracks)
			if (!out[t.video_id] && !isDownloaded(t.video_id) && !api.isLocalId(t.video_id)) out[t.video_id] = 'offline';
		return out;
	});
	$effect(() => {
		if (skipReasons) return;
		const list = tracks;
		if (!list.length) return;
		let stale = false;
		const t = setTimeout(() => {
			api
				.playbackSkipReasons($state.snapshot(list) as SongItem[])
				.then((r) => !stale && (fetchedReasons = r ?? {}))
				.catch(() => {});
		}, 50);
		return () => {
			stale = true;
			clearTimeout(t);
		};
	});

	const ROW = $derived(compact ? 32 : 56);

	// ── Layout ────────────────────────────────────────────────────────────────────────────────
	let root = $state<HTMLDivElement | null>(null);
	let width = $state(0);
	// Date added only when some row has a date: Spotify drops the column on Your Mixes (parity3).
	const hasAdded = $derived(tracks.some((t) => t.added_at));
	const columns = $derived(visibleColumns(variant, width, compact).filter((c) => c !== 'added' || hasAdded));
	const template = $derived(gridTemplate(columns));

	// Display order: local sort unless the parent sorts (onSort). Settings › Show unplayable songs
	// off: the greyed rows aren't listed at all.
	const order = $derived.by(() => {
		const o = onSort ? tracks.map((_, i) => i) : sortedOrder(tracks, sort);
		return prefs.showUnplayable ? o : o.filter((i) => !reasons[tracks[i].video_id]);
	});

	// ── Playing state ─────────────────────────────────────────────────────────────────────────
	const nowId = $derived(playback.now?.videoId);
	const current = (song: SongItem, i: number) =>
		isPlaying ? isPlaying(song, i) : !!nowId && nowId === song.video_id;

	// ── Selection ─────────────────────────────────────────────────────────────────────────────
	let anchor = $state<number | null>(null); // display position of the last plain/⌘ click
	let focusPos = $state<number | null>(null);

	function select(pos: number, e: MouseEvent | KeyboardEvent) {
		const i = order[pos];
		if (e.shiftKey && anchor !== null) {
			const [a, b] = anchor < pos ? [anchor, pos] : [pos, anchor];
			if (!(e.metaKey || e.ctrlKey)) selected.clear();
			for (let p = a; p <= b; p++) selected.add(order[p]);
		} else if (e.metaKey || e.ctrlKey) {
			if (selected.has(i)) selected.delete(i);
			else selected.add(i);
			anchor = pos;
		} else {
			selected.clear();
			selected.add(i);
			anchor = pos;
		}
		focusPos = pos;
		root?.focus({ preventScroll: true });
	}

	const selectedSongs = () =>
		order.filter((i) => selected.has(i)).map((i) => tracks[i]);

	function songsFor(pos: number): SongItem[] {
		const i = order[pos];
		if (selected.has(i) && selected.size > 1) return selectedSongs();
		return [tracks[i]];
	}

	function play(pos: number) {
		const i = order[pos];
		if (tracks[i]?.unavailable || reasons[tracks[i]?.video_id]) return;
		// Play in display order so a sorted table plays as shown.
		const list = order.map((j) => tracks[j]);
		if (onPlay) onPlay(list, pos);
		else playTracks(list, pos, context);
	}

	function contextMenu(pos: number, e: MouseEvent) {
		const i = order[pos];
		if (!selected.has(i)) {
			selected.clear();
			selected.add(i);
			anchor = pos;
		}
		focusPos = pos;
		openTrackMenu(e, songsFor(pos), menuContext);
	}

	function moreMenu(pos: number, el: HTMLElement) {
		const i = order[pos];
		if (!selected.has(i)) {
			selected.clear();
			selected.add(i);
			anchor = pos;
		}
		focusPos = pos;
		openTrackMenu(el, songsFor(pos), menuContext);
	}

	const canRemove = $derived(
		!!menuContext?.playlistId && (menuContext.own ?? isOwnPlaylist(menuContext.playlistId))
	);

	async function removeSelected() {
		if (!canRemove || !selected.size || !menuContext?.playlistId) return;
		const songs = selectedSongs();
		selected.clear();
		await removeFromPlaylist(menuContext.playlistId, songs);
		menuContext.onRemove?.(songs);
	}

	function rowEl(pos: number): HTMLElement | null {
		return root?.querySelector<HTMLElement>(`[data-pos="${pos}"] [data-track-row]`) ?? null;
	}

	function onKey(e: KeyboardEvent) {
		if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
		const mod = e.metaKey || e.ctrlKey;
		if (mod && e.key.toLowerCase() === 'a') {
			for (const i of order) selected.add(i);
			e.preventDefault();
		} else if (e.key === 'Escape' && selected.size) {
			selected.clear();
			e.preventDefault();
		} else if (e.key === 'Enter' && selected.size) {
			const pos = order.findIndex((i) => selected.has(i));
			if (pos >= 0) play(pos);
			e.preventDefault();
		} else if ((e.key === 'Delete' || e.key === 'Backspace') && selected.size && canRemove) {
			removeSelected();
			e.preventDefault();
		} else if (e.altKey && (e.key.toLowerCase() === 'j' || e.code === 'KeyJ')) {
			const pos = focusPos ?? order.findIndex((i) => selected.has(i));
			if (pos === null || pos < 0) return;
			const el = rowEl(pos);
			if (el) openTrackMenu(el, songsFor(pos), menuContext);
			e.preventDefault();
		} else if (
			(e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) &&
			(focusPos ?? -1) >= 0
		) {
			// a11y: the context-menu key / ⇧F10 do what ⌥J does.
			const el = rowEl(focusPos!);
			if (el) openTrackMenu(el, songsFor(focusPos!), menuContext);
			e.preventDefault();
		} else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp' || ((e.key === 'Home' || e.key === 'End') && !e.shiftKey)) && !e.altKey && !mod) {
			if (!order.length) return;
			const cur = focusPos ?? -1;
			const next =
				e.key === 'Home'
					? 0
					: e.key === 'End'
						? order.length - 1
						: Math.max(0, Math.min(order.length - 1, cur + (e.key === 'ArrowDown' ? 1 : -1)));
			select(next, e);
			scrollToPos(next);
			e.preventDefault();
		} else {
			return;
		}
		// Handled: keep the app-wide shortcuts (space etc.) from seeing it.
		e.stopPropagation();
	}

	// a11y: the grid keeps DOM focus and points at the focused row with aria-activedescendant, so
	// screen readers read the row ↑/↓ lands on. One id, moved to whichever row element is current
	// (rows are windowed/recycled, so ids aren't tied to positions).
	const activeId = `tt-${Math.random().toString(36).slice(2, 9)}-active`;
	$effect(() => {
		const pos = focusPos;
		void tracks.length;
		const apply = () => {
			const prev = document.getElementById(activeId);
			const el = pos === null ? null : rowEl(pos);
			if (prev && prev !== el) prev.removeAttribute('id');
			if (el) el.id = activeId;
			if (el) root?.setAttribute('aria-activedescendant', activeId);
			else root?.removeAttribute('aria-activedescendant');
			return !!el || pos === null;
		};
		// A windowed row scrolled into view renders a frame later.
		if (!apply()) {
			const f = requestAnimationFrame(apply);
			return () => cancelAnimationFrame(f);
		}
	});

	// Clicking anywhere outside the table (and outside menus) clears the selection.
	onMount(() => {
		const down = (e: PointerEvent) => {
			if (!selected.size) return;
			const t = e.target as Element | null;
			if (!t || root?.contains(t)) return;
			if (t.closest('[role=menu],[role=dialog],[data-keep-selection]')) return;
			selected.clear();
			focusPos = null;
		};
		window.addEventListener('pointerdown', down, true);
		return () => window.removeEventListener('pointerdown', down, true);
	});

	// Drop indices that no longer exist when the list shrinks.
	$effect(() => {
		const n = tracks.length;
		for (const i of [...selected]) if (i >= n) selected.delete(i);
	});

	// ── Drag: source + reorder ───────────────────────────────────────────────────────────────
	let dragging = $state<number[] | null>(null); // indices into tracks
	let dropAt = $state<number | null>(null); // display position to insert before
	const reorderable = $derived(!!onReorder && !sort);

	function dragStart(pos: number, e: DragEvent) {
		const i = order[pos];
		const songs = songsFor(pos);
		dragging = selected.has(i) && selected.size > 1 ? order.filter((j) => selected.has(j)) : [i];
		setDragSongs(e, songs);
		e.dataTransfer?.setData('application/x-encore-table', '1');
	}

	function dragOver(pos: number, e: DragEvent) {
		if (!dragging && onDropIn && acceptDrop?.(e)) {
			// From outside this table: a copy, at the line.
			e.preventDefault();
			if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
			const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
			dropAt = e.clientY < r.top + r.height / 2 ? pos : pos + 1;
			return;
		}
		if (!reorderable || !dragging) return;
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
		const el = e.currentTarget as HTMLElement;
		const r = el.getBoundingClientRect();
		dropAt = e.clientY < r.top + r.height / 2 ? pos : pos + 1;
	}

	function drop(e: DragEvent) {
		if (!dragging && onDropIn && dropAt !== null) {
			e.preventDefault();
			const to = dropAt >= order.length ? tracks.length : order[dropAt];
			dropAt = null;
			onDropIn(e, to);
			return;
		}
		if (!reorderable || !dragging || dropAt === null) return;
		e.preventDefault();
		const to = dropAt >= order.length ? tracks.length : order[dropAt];
		const from = dragging;
		dragging = null;
		dropAt = null;
		if (!(from.length === 1 && (to === from[0] || to === from[0] + 1))) onReorder?.(from, to);
	}

	function dragEnd() {
		dragging = null;
		dropAt = null;
	}

	// ── Header: sort + sticky ────────────────────────────────────────────────────────────────
	function clickHeader(col: (typeof columns)[number]) {
		const key = SORTABLE[col];
		if (!key) return;
		sort = nextSort(sort, key);
		onSort?.(sort);
	}

	let sentinel = $state<HTMLDivElement | null>(null);
	let stuck = $state(false);
	$effect(() => {
		if (stickyTop === undefined || !sentinel) return;
		const io = new IntersectionObserver(
			([en]) => {
				const rootTop = en.rootBounds?.top ?? 0;
				stuck = !en.isIntersecting && en.boundingClientRect.top < rootTop + stickyTop + 1;
			},
			{ root: scroller ?? null, rootMargin: `-${stickyTop + 1}px 0px 0px 0px`, threshold: 0 }
		);
		io.observe(sentinel);
		return () => io.disconnect();
	});

	// ── Windowing ────────────────────────────────────────────────────────────────────────────
	let body = $state<HTMLDivElement | null>(null);
	let win = $state({ start: 0, end: 60 });
	const windowed = $derived(!!scroller && order.length > 80);

	function measure() {
		if (!scroller || !body) return;
		const top =
			body.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
		const avg = ROW + (mixEvery ? mixHeight / mixEvery : 0);
		const from = Math.floor((scroller.scrollTop - top) / avg) - 10;
		const count = Math.ceil(scroller.clientHeight / ROW) + 20;
		const start = Math.max(0, Math.min(order.length, from));
		const end = Math.max(start, Math.min(order.length, from + count));
		if (start !== win.start || end !== win.end) win = { start, end };
	}

	$effect(() => {
		if (!windowed || !scroller) return;
		void order.length;
		const sc = scroller;
		let raf = 0;
		const on = () => {
			cancelAnimationFrame(raf);
			raf = requestAnimationFrame(measure);
		};
		measure();
		sc.addEventListener('scroll', on, { passive: true });
		const ro = new ResizeObserver(on);
		ro.observe(sc);
		return () => {
			cancelAnimationFrame(raf);
			sc.removeEventListener('scroll', on);
			ro.disconnect();
		};
	});

	const range = $derived(windowed ? win : { start: 0, end: order.length });
	const visible = $derived(order.slice(range.start, range.end).map((_, k) => range.start + k));

	function scrollToPos(pos: number) {
		if (windowed && scroller && body) {
			const top =
				body.getBoundingClientRect().top -
				scroller.getBoundingClientRect().top +
				scroller.scrollTop +
				yOf(pos);
			const vTop = scroller.scrollTop + (stickyTop ?? 0) + 36;
			const vBot = scroller.scrollTop + scroller.clientHeight;
			if (top < vTop) scroller.scrollTop = top - (stickyTop ?? 0) - 36;
			else if (top + ROW > vBot) scroller.scrollTop = top + ROW - scroller.clientHeight;
		} else {
			rowEl(pos)?.scrollIntoView({ block: 'nearest' });
		}
	}

	const sortIcon = (col: (typeof columns)[number]) =>
		sort && SORTABLE[col] === sort.key ? (sort.dir === 'asc' ? CaretUpIcon : CaretDownIcon) : null;
</script>

<div
	bind:this={root}
	bind:clientWidth={width}
	role="grid"
	tabindex="0"
	aria-rowcount={tracks.length + (showHeader ? 1 : 0)}
	aria-multiselectable="true"
	aria-label={context?.title ?? 'Tracks'}
	class="relative outline-none {focusPos === null ? 'focus-visible:outline-2 focus-visible:outline-white' : ''} rounded-sm"
	onkeydown={onKey}
>
	{#if showHeader}
		{#if stickyTop !== undefined}<div bind:this={sentinel} class="h-px"></div>{/if}
		<div
			role="row"
			aria-rowindex={1}
			class="group/head z-[2] mb-3 {stickyTop !== undefined ? 'sticky' : ''} {stuck
				? 'border-b border-border-subtle bg-bg-highlight shadow-[0_-1px_0_#181818]'
				: 'border-b border-border-subtle'}"
			style="{stickyTop !== undefined ? `top:${stickyTop}px;` : ''}{stuck
				? `margin-inline:calc(${bleed} * -1);padding-inline:${bleed};`
				: ''}"
		>
			<div class="grid h-9 items-center gap-4 px-4" style="grid-template-columns:{template}">
				{#each columns as col, ci (col)}
					{@const sortable = !!SORTABLE[col]}
					{@const icon = sortIcon(col)}
					<div
						role="columnheader"
						aria-sort={icon ? (sort?.dir === 'asc' ? 'ascending' : 'descending') : undefined}
						class="relative flex min-w-0 items-center text-sm text-text-subdued {col === 'index'
							? 'justify-end'
							: col === 'duration'
								? 'justify-end pr-8'
								: ''}"
					>
						{#if stuck && ci > 0}
							<!-- Spotify shows the column dividers only while the pinned header is hovered. -->
							<span class="absolute top-1/2 -left-2 h-4 w-px -translate-y-1/2 bg-white/10 opacity-0 group-hover/head:opacity-100"></span>
						{/if}
						{#if sortable}
							<button
								type="button"
								class="flex min-w-0 items-center gap-1 hover:text-white {icon ? 'text-white' : ''}"
								onclick={() => clickHeader(col)}
							>
								{#if col === 'duration'}
									<Icon icon={ClockIcon} size={16} label="Duration" />
								{:else}
									<span class="truncate">{COLUMN_LABEL[col]}</span>
								{/if}
								{#if icon}<Icon {icon} size={12} fill class="text-accent" />{/if}
							</button>
						{:else if col === 'duration'}
							<Icon icon={ClockIcon} size={16} label="Duration" />
						{:else}
							<span class="truncate">{COLUMN_LABEL[col]}</span>
						{/if}
					</div>
				{/each}
			</div>
		</div>
	{/if}

	{#if !tracks.length}
		{@render empty?.()}
	{:else}
		<div
			bind:this={body}
			role="rowgroup"
			style={windowed
				? `padding-top:${yOf(range.start)}px;padding-bottom:${yOf(order.length) - yOf(range.end)}px`
				: ''}
			ondragend={dragEnd}
			ondragleave={(e) => {
				if (!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node)) dropAt = null;
			}}
		>
			{#each visible as pos (order[pos] + ':' + tracks[order[pos]].video_id)}
				{@const i = order[pos]}
				{@const song = tracks[i]}
				{@const cur = current(song, i)}
				<div data-pos={pos}>
					<TrackRow
						{song}
						skipReason={reasons[song.video_id] ?? null}
						number={startIndex + pos}
						{columns}
						{template}
						{variant}
						{compact}
						{likedOnHover}
						rowIndex={pos + (showHeader ? 2 : 1)}
						selected={selected.has(i)}
						current={cur}
						paused={playback.paused}
						dropLine={dropAt === pos
							? 'top'
							: dropAt === order.length && pos === order.length - 1
								? 'bottom'
								: null}
						onselect={(e) => select(pos, e)}
						onplay={() => play(pos)}
						ontogglepause={() => api.togglePause()}
						oncontextmenu={(e) => contextMenu(pos, e)}
						onmenu={(el) => moreMenu(pos, el)}
						ondragstart={(e) => dragStart(pos, e)}
						ondragover={(e) => dragOver(pos, e)}
						ondrop={drop}
					/>
				</div>
				{#if interleave && mixedAfter(pos) >= 0}
					{@render interleave.row(mixedAfter(pos), { columns, template })}
				{/if}
			{/each}
		</div>
	{/if}
</div>
