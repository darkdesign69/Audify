<script lang="ts">
	/**
	 * "Your Library" (docs/spotify-spec/screens/library-sidebar.md).
	 * `mode`: 'sidebar' (left panel; collapses to a 72px rail by `shell.leftCollapsed`) or 'page'
	 * (the ↗ expanded full-width library in the main view).
	 */
	import { scrollBehavior } from '$lib/kit/motion';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { lib } from '$lib/library.svelte';
	import { playback } from '$lib/player.svelte';
	import { playItem } from '$lib/browse';
	import * as api from '$lib/api';
	import { books, playBookById, toggleBookSaved } from '$lib/audiobooks.svelte'; // stream: audiobooks
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import { roving } from '$lib/kit/roving';
	import Icon from '$lib/kit/Icon.svelte';
	import IconButton from '$lib/kit/IconButton.svelte';
	import Tooltip from '$lib/kit/Tooltip.svelte';
	import PlayButton from '$lib/kit/PlayButton.svelte';
	import { openMenu, openItemMenu, openFolderMenu } from '$lib/kit/menus';
	import type { MenuEntry } from '$lib/kit/menus';
	import { addToPlaylist, createFolder, createPlaylist, isOwnPlaylist, togglePin } from '$lib/kit/actions';
	import { isTracksDrag, LIB_MIME, readTrackDrop, setDragItem } from '$lib/dnd';
	import { resolveTrackDrop } from '$lib/dndTracks';
	import { toast, toastUndo } from '$lib/kit/toast';
	import { shell, toggleLeft, leftPx, leftRail } from './layout.svelte';
	import {
		currentSort,
		currentView,
		libraryUi,
		setFilter,
		setSort,
		setView,
		type LibFilter,
		type LibSort,
		type LibView
	} from './library.svelte';
	import { hasDownloads, isRowDownloaded, presentFilters, visibleRows, type LibRow } from './libraryItems.svelte';
	import LibraryCover from './LibraryCover.svelte';
	import BooksIcon from 'phosphor-svelte/lib/BooksIcon';
	import SidebarSimpleIcon from 'phosphor-svelte/lib/SidebarSimpleIcon';
	import PlusIcon from 'phosphor-svelte/lib/PlusIcon';
	import ArrowsOutSimpleIcon from 'phosphor-svelte/lib/ArrowsOutSimpleIcon';
	import ArrowsInSimpleIcon from 'phosphor-svelte/lib/ArrowsInSimpleIcon';
	import MagnifyingGlassIcon from 'phosphor-svelte/lib/MagnifyingGlassIcon';
	import XIcon from 'phosphor-svelte/lib/XIcon';
	import CaretRightIcon from 'phosphor-svelte/lib/CaretRightIcon';
	import CaretLeftIcon from 'phosphor-svelte/lib/CaretLeftIcon';
	import ListIcon from 'phosphor-svelte/lib/ListIcon';
	import ListBulletsIcon from 'phosphor-svelte/lib/ListBulletsIcon';
	import GridNineIcon from 'phosphor-svelte/lib/GridNineIcon';
	import SquaresFourIcon from 'phosphor-svelte/lib/SquaresFourIcon';
	import PushPinIcon from 'phosphor-svelte/lib/PushPinIcon';
	import ArrowCircleDownIcon from 'phosphor-svelte/lib/ArrowCircleDownIcon'; // stream: downloads
	import SpeakerHighIcon from 'phosphor-svelte/lib/SpeakerHighIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import MusicNotesPlusIcon from 'phosphor-svelte/lib/MusicNotesPlusIcon';
	import FolderSimplePlusIcon from 'phosphor-svelte/lib/FolderSimplePlusIcon';

	let { mode = 'sidebar' }: { mode?: 'sidebar' | 'page' } = $props();

	const collapsed = $derived(mode === 'sidebar' && leftRail());
	let navWidth = $state(1000);
	const width = $derived(mode === 'page' ? navWidth : leftPx());
	const wide = $derived(width >= 360);
	const view = $derived<LibView>(collapsed ? 'list' : currentView());
	const sort = $derived<LibSort>(currentSort());

	const FILTER_LABEL: Record<LibFilter, string> = {
		playlist: 'Playlists',
		artist: 'Artists',
		album: 'Albums',
		podcast: 'Podcasts & Shows',
		audiobook: 'Audiobooks' // stream: audiobooks
	};
	const filters = $derived(presentFilters());
	const data = $derived(visibleRows());
	const list = $derived([...data.pinned, ...data.rest]);
	const folder = $derived(libraryUi.folderPath.length ? lib.folder(libraryUi.folderPath.at(-1)!) : null);
	// The "Create your first playlist" / "Let's find some podcasts" cards are for a library with
	// nothing in it at all: one like already gives it a Liked Songs row (likewise a followed show,
	// saved episodes, local files).
	const empty = $derived(
		!libraryUi.query && !libraryUi.filter && !folder && lib.ready && list.length === 0
	);

	// Spotify pins Liked Songs at the top. Pin it once, the first time it appears; after that the
	// pin is the user's to keep or remove.
	const LIKED_PIN_KEY = 'encore.likedPinDefaulted';
	$effect(() => {
		if (mode !== 'sidebar' || !lib.ready || lib.likedCount === 0 || lib.isPinned(api.LIKED_ID)) return;
		try {
			if (localStorage.getItem(LIKED_PIN_KEY)) return;
			localStorage.setItem(LIKED_PIN_KEY, '1');
		} catch {
			return;
		}
		lib.pin(api.LIKED_ID).catch(() => {});
	});

	// ── search field ──
	let searchInput = $state<HTMLInputElement>();
	$effect(() => {
		if (libraryUi.searchSeq) {
			if (leftRail() && mode === 'sidebar') toggleLeft();
			queueMicrotask(() => searchInput?.focus());
		}
	});

	// ── chips scroll ──
	let chipRow = $state<HTMLDivElement>();
	let chipsCanRight = $state(false);
	let chipsCanLeft = $state(false);
	function chipScroll() {
		if (!chipRow) return;
		chipsCanLeft = chipRow.scrollLeft > 2;
		chipsCanRight = chipRow.scrollLeft + chipRow.clientWidth < chipRow.scrollWidth - 2;
	}
	$effect(() => {
		void filters.length;
		void libraryUi.filter;
		void width;
		queueMicrotask(chipScroll);
	});

	// ── menus ──
	function createMenu(e: MouseEvent) {
		openMenu(
			e,
			[
				{ type: 'custom', snippet: createItems }
			],
			{ width: 476, align: 'start' }
		);
	}

	const SORTS: [LibSort, string][] = [
		['recents', 'Recents'],
		['added', 'Recently Added'],
		['alpha', 'Alphabetical'],
		['creator', 'Creator'],
		['custom', 'Custom order']
	];
	const VIEWS: [LibView, string, typeof ListIcon][] = [
		['compact', 'Compact list', ListIcon],
		['list', 'Default list', ListBulletsIcon],
		['compact-grid', 'Compact grid', GridNineIcon],
		['grid', 'Default grid', SquaresFourIcon]
	];
	const viewIcon = $derived(VIEWS.find((v) => v[0] === view)?.[2] ?? ListBulletsIcon);
	function sortMenu(e: MouseEvent) {
		const entries: MenuEntry[] = [
			{ type: 'heading', label: 'Sort by' },
			...SORTS.map(([id, label]) => ({ label, checked: sort === id, onSelect: () => setSort(id) })),
			{ type: 'separator' },
			{ type: 'heading', label: 'View as' },
			{ type: 'custom', snippet: viewRow }
		];
		openMenu(e, entries, { width: 208 });
	}

	// ── rows ──
	const playingId = $derived(playback.now ? (playback.queue.sourceId ?? null) : null);
	const isActive = (r: LibRow) =>
		!!r.href &&
		decodeURIComponent(page.url.pathname + (r.href.includes('?') ? page.url.search : '')) ===
			decodeURIComponent(r.href);

	function open(r: LibRow) {
		if (r.kind === 'folder') {
			libraryUi.folderPath = [...libraryUi.folderPath, r.id];
			return;
		}
		goto(r.href, { keepFocus: true }); // a11y: Spotify leaves focus on the row
	}
	function play(r: LibRow) {
		if (r.kind === 'folder' || r.kind === 'podcast' || r.kind === 'episodes' || r.kind === 'local') return open(r);
		if (r.kind === 'audiobook') return void playBookById(r.id); // stream: audiobooks
		if (playingId === r.id) {
			api.togglePause();
			return;
		}
		playItem(r.item);
	}
	// a11y: keyboard on a focused row — Enter opens it, ⌥J / the context-menu key / ⇧F10 open its
	// menu under the row (Spotify), the arrows are handled by the `roving` attachment on the list.
	function rowKey(e: KeyboardEvent, r: LibRow) {
		if (e.target !== e.currentTarget) return;
		if (e.key === 'Enter' && !e.altKey && !e.metaKey && !e.ctrlKey) {
			e.preventDefault();
			open(r);
		} else if (
			(e.altKey && !e.shiftKey && !e.metaKey && (e.code === 'KeyJ' || e.key.toLowerCase() === 'j')) ||
			e.key === 'ContextMenu' ||
			(e.shiftKey && e.key === 'F10')
		) {
			e.preventDefault();
			e.stopPropagation();
			menuFor(e.currentTarget as HTMLElement, r);
		}
	}
	function menuFor(e: MouseEvent | HTMLElement, r: LibRow) {
		if (r.kind === 'folder') {
			const f = lib.folder(r.id);
			if (f) openFolderMenu(e, f, { sidebar: true, onRename: () => rename(r) });
		} else if (r.kind === 'local' || r.kind === 'episodes') {
			const pinned = lib.isPinned(r.id);
			openMenu(e, [
				{ label: pinned ? 'Unpin' : 'Pin', icon: PushPinIcon, onSelect: () => togglePin(r.id) }
			]);
		} else if (r.kind === 'audiobook') {
			// stream: audiobooks — pin and remove; the book page has the rest.
			const pinned = lib.isPinned(r.id);
			const book = books.saved.find((x) => x.id === r.id);
			openMenu(e, [
				{ label: pinned ? 'Unpin' : 'Pin', icon: PushPinIcon, onSelect: () => togglePin(r.id) },
				{ label: 'Remove from Your Library', icon: CheckCircleIcon, onSelect: () => book && toggleBookSaved(book) }
			]);
		} else openItemMenu(e, r.item, { sidebar: true });
	}
	async function rename(r: LibRow) {
		const name = prompt('Folder name', r.title);
		if (name && name.trim()) toastUndo('Folder renamed', await lib.renameFolder(r.id, name.trim()));
	}

	// ── drag & drop ──
	let dropOn = $state<string | null>(null);
	const canTakeSongs = (r: LibRow) => r.kind === 'liked' || (r.kind === 'playlist' && isOwnPlaylist(r.id));
	function dragStart(e: DragEvent, r: LibRow) {
		if (!e.dataTransfer || r.kind === 'liked' || r.kind === 'episodes' || r.kind === 'local') return;
		// Lane magic-d2: an album or playlist row also carries the card payload, so it can be dropped
		// on the queue or into another playlist (its songs go in), with Spotify's name-pill ghost.
		if (r.item && (r.kind === 'playlist' || r.kind === 'album')) setDragItem(e, r.item);
		e.dataTransfer.setData(LIB_MIME, JSON.stringify({ id: r.id, kind: r.kind }));
		e.dataTransfer.setData('text/plain', r.title);
		e.dataTransfer.effectAllowed = 'copyMove';
	}
	/** A library row being moved (folder, Custom order) wins over adding its songs. */
	const isMove = (e: DragEvent, r: LibRow) =>
		!!e.dataTransfer?.types.includes(LIB_MIME) && (r.kind === 'folder' || sort === 'custom');
	function dragOver(e: DragEvent, r: LibRow) {
		if (isMove(e, r) || (isTracksDrag(e) && canTakeSongs(r))) {
			e.preventDefault();
			if (e.dataTransfer) e.dataTransfer.dropEffect = isMove(e, r) ? 'move' : 'copy';
			dropOn = r.id;
		}
	}
	async function drop(e: DragEvent, r: LibRow) {
		e.preventDefault();
		dropOn = null;
		const tracks = isMove(e, r) ? null : readTrackDrop(e);
		if (tracks) {
			// A card or library row: expand it to its songs first (Spotify adds the whole album).
			let songs;
			try {
				songs = (await resolveTrackDrop(tracks)).songs;
			} catch (err) {
				toast.error(String(err));
				return;
			}
			if (!songs.length || ('item' in tracks && tracks.item.id === r.id)) return;
			if (r.kind === 'liked') {
				const todo = songs.filter((s) => !lib.isLiked(s.video_id));
				if (todo.length) toastUndo('Added to Liked Songs', await lib.like(todo));
			} else await addToPlaylist(r.item, songs);
			return;
		}
		const raw = e.dataTransfer?.getData(LIB_MIME);
		if (!raw) return;
		const src = JSON.parse(raw) as { id: string; kind: string };
		if (src.id === r.id) return;
		try {
			if (r.kind === 'folder') {
				if (src.kind === 'folder') toastUndo(`Moved to ${r.title}`, await lib.moveFolder(src.id, r.id));
				else toastUndo(`Moved to ${r.title}`, await lib.setItemFolder(src.id, r.id));
			} else if (sort === 'custom') {
				const parent = libraryUi.folderPath.at(-1) ?? null;
				const index = list.filter((x) => x.kind !== 'folder').findIndex((x) => x.id === r.id);
				if (src.kind === 'folder') await lib.moveFolder(src.id, parent, Math.max(0, index));
				else await lib.setItemFolder(src.id, parent, Math.max(0, index));
			}
		} catch (err) {
			toast.error(String(err));
		}
	}

	const subtitleOf = (r: LibRow) => r.subtitle;
	const fmtDate = (secs: number) =>
		secs ? new Date(secs * 1000).toLocaleDateString('en-US', { day: 'numeric', month: 'short', year: 'numeric' }) : '';
	function fmtAgo(secs: number) {
		if (!secs) return '';
		const d = Date.now() / 1000 - secs;
		if (d < 3600) return `${Math.max(1, Math.round(d / 60))} min ago`;
		if (d < 86400) return `${Math.round(d / 3600)} hr ago`;
		if (d < 86400 * 7) return `${Math.round(d / 86400)} day${Math.round(d / 86400) === 1 ? '' : 's'} ago`;
		if (d < 86400 * 30) return `${Math.round(d / 604800)} wk ago`;
		return fmtDate(secs);
	}
	const gridCols = $derived(
		view === 'compact-grid'
			? Math.max(3, Math.floor((width - 16) / 80))
			: Math.max(2, Math.floor((width - 16) / (mode === 'page' ? 178 : 140)))
	);
</script>

{#snippet createItems(close: () => void)}
	{#each [
		{ icon: MusicNotesPlusIcon, title: 'Playlist', desc: 'Create a playlist with songs or episodes', run: () => createPlaylist([], true, libraryUi.folderPath.at(-1)) },
		{ icon: FolderSimplePlusIcon, title: 'Folder', desc: 'Organize your playlists', run: () => createFolder(libraryUi.folderPath.at(-1)) }
	] as it, i (i)}
		{#if i === 1}<div class="my-1 h-px bg-white/10"></div>{/if}
		<button
			type="button"
			role="menuitem"
			class="flex h-16 w-full items-center gap-3 rounded-md p-2 text-left hover:bg-white/10 focus-visible:bg-white/10 focus-visible:outline-none"
			onclick={() => {
				close();
				it.run();
			}}
		>
			<span class="grid size-12 shrink-0 place-items-center rounded-full bg-white/10 text-white">
				<Icon icon={it.icon} size={24} />
			</span>
			<span class="min-w-0">
				<span class="block text-base font-bold text-white">{it.title}</span>
				<span class="block truncate text-sm text-text-subdued">{it.desc}</span>
			</span>
		</button>
	{/each}
{/snippet}

{#snippet viewRow(close: () => void)}
	<div class="flex items-center gap-1 px-2 pb-2">
		{#each VIEWS as [id, label, icon] (id)}
			<Tooltip text={label} side="bottom">
				{#snippet trigger(props)}
					<button
						{...props}
						type="button"
						role="menuitemradio"
						aria-label={label}
						aria-checked={view === id}
						class="grid size-8 place-items-center rounded-sm {view === id
							? 'bg-white/10 text-white'
							: 'text-white/70 hover:text-white'}"
						onclick={() => {
							setView(id);
							close();
						}}
					>
						<Icon {icon} size={16} />
					</button>
				{/snippet}
			</Tooltip>
		{/each}
	</div>
{/snippet}

{#snippet nowPlayingGlyph(r: LibRow)}
	{#if mode === 'page' && playingId !== r.id}
		<span class="w-6 shrink-0"></span>
	{:else if playingId === r.id}
		<span class="ml-auto w-6 shrink-0 pl-2 text-accent">
			<span class="group-hover/row:hidden"><Icon icon={SpeakerHighIcon} size={16} fill /></span>
			<span class="hidden group-hover/row:inline">
				<Icon icon={playback.paused ? PlayIcon : PauseIcon} size={16} fill />
			</span>
		</span>
	{/if}
{/snippet}

<!-- a11y: a named region inside the shell's "Main" navigation (or inside <main> when expanded). -->
<section
	bind:clientWidth={navWidth}
	class="flex h-full min-h-0 flex-col overflow-hidden rounded-lg bg-bg-base"
	aria-label="Your Library"
	data-library
>
	<!-- 1. Header -->
	{#if collapsed}
		<div class="flex flex-col items-center gap-2 pt-4 pb-2">
			<IconButton icon={BooksIcon} label="Open Your Library" tooltipSide="right" iconSize={24} onclick={toggleLeft} />
			<IconButton icon={PlusIcon} label="Create" variant="filled" tooltipSide="right" class="[&_svg]:transition-transform [&_svg]:duration-200 [&[data-menu-open]_svg]:rotate-45" onclick={createMenu} />
		</div>
	{:else}
		<div class="flex h-14 shrink-0 items-center justify-between pt-2 pr-4 pl-5">
			{#if mode === 'sidebar'}
				<Tooltip text="Collapse Your Library">
					{#snippet trigger(props)}
						<button
							{...props}
							type="button"
							class="group/title -ml-2 flex items-center gap-2 rounded px-2 py-1 text-base font-bold text-white"
							onclick={toggleLeft}
						>
							<span class="hidden text-text-subdued group-hover/title:inline group-hover/title:text-white"><Icon icon={SidebarSimpleIcon} size={24} /></span>
							<h2>Your Library</h2>
						</button>
					{/snippet}
				</Tooltip>
			{:else}
				<!-- Expanded, Spotify keeps the sidebar's 16/700 title (no page-size heading). -->
				<h1 class="text-base font-bold text-white">Your Library</h1>
			{/if}
			<div class="flex items-center gap-2">
				{#if wide}
					<Tooltip text="Create">
						{#snippet trigger(props)}
							<button
								{...props}
								type="button"
								class="flex h-8 items-center gap-1 rounded-full bg-bg-highlight pr-4 pl-3 text-sm font-bold text-white transition-transform hover:scale-[1.04] hover:bg-bg-elevated-highlight"
								onclick={createMenu}
							>
								<Icon icon={PlusIcon} size={16} /> Create
							</button>
						{/snippet}
					</Tooltip>
				{:else}
					<!-- Spotify: the + turns into an × while its menu is open -->
					<IconButton icon={PlusIcon} label="Create" variant="filled" class="[&_svg]:transition-transform [&_svg]:duration-200 [&[data-menu-open]_svg]:rotate-45" onclick={createMenu} />
				{/if}
				<IconButton
					icon={mode === 'page' ? ArrowsInSimpleIcon : ArrowsOutSimpleIcon}
					label={mode === 'page' ? 'Show less' : 'Expand Your Library'}
					onclick={() => (shell.libraryExpanded = mode !== 'page')}
				/>
			</div>
		</div>

		<!-- 2 + 3 share one row when expanded (chips left, open search + sort right), like Spotify. -->
		<div class={mode === 'page' ? 'mt-2 flex shrink-0 items-center gap-4 pr-0' : 'contents'}>
		<!-- 2. Filter chips -->
		{#if filters.length || libraryUi.filter || hasDownloads()}
			<div class="relative shrink-0 px-4 {mode === 'page' ? 'min-w-0 flex-1' : 'mt-2'}">
				<div
					bind:this={chipRow}
					class="no-scrollbar flex items-center gap-2 overflow-x-auto"
					role="toolbar"
					aria-label="Filter options"
					{@attach roving({ items: 'button.chip', orientation: 'horizontal' })}
					onscroll={chipScroll}
				>
					{#if libraryUi.filter || libraryUi.downloaded || folder}
						<button
							type="button"
							class="chip size-8 justify-center !p-0"
							aria-label="Clear filters"
							onclick={() => {
								setFilter(null);
							}}
						>
							<Icon icon={XIcon} size={16} />
						</button>
					{/if}
					<!-- A selected chip shows alone with X (Spotify): "Downloaded" on its own hides the type chips (parity3). -->
					{#each filters.filter((f) => (libraryUi.filter ? f === libraryUi.filter : !libraryUi.downloaded)) as f (f)}
						<button type="button" class="chip" aria-pressed={libraryUi.filter === f} onclick={() => setFilter(libraryUi.filter === f ? null : f)}>
							{FILTER_LABEL[f]}
						</button>
					{/each}
					{#if libraryUi.filter === 'playlist'}
						<button type="button" class="chip" aria-pressed={libraryUi.byYou} onclick={() => (libraryUi.byYou = !libraryUi.byYou)}>By you</button>
					{/if}
					{#if hasDownloads() || libraryUi.downloaded}
						<!-- stream: downloads -->
						<button type="button" class="chip" aria-pressed={libraryUi.downloaded} onclick={() => (libraryUi.downloaded = !libraryUi.downloaded)}>Downloaded</button>
					{/if}
				</div>
				{#if chipsCanLeft}
					<div class="pointer-events-none absolute inset-y-0 left-4 w-12 bg-gradient-to-r from-bg-base to-transparent"></div>
					<button type="button" aria-label="Scroll left" class="absolute top-0 left-4 grid size-8 place-items-center rounded-full bg-bg-highlight text-white shadow-overlay hover:bg-bg-elevated-highlight" onclick={() => chipRow?.scrollBy({ left: -160, behavior: scrollBehavior() })}><Icon icon={CaretLeftIcon} size={16} /></button>
				{/if}
				{#if chipsCanRight}
					<div class="pointer-events-none absolute inset-y-0 right-4 w-12 bg-gradient-to-l from-bg-base to-transparent"></div>
					<button type="button" aria-label="Scroll right" class="absolute top-0 right-4 grid size-8 place-items-center rounded-full bg-bg-highlight text-white shadow-overlay hover:bg-bg-elevated-highlight" onclick={() => chipRow?.scrollBy({ left: 160, behavior: scrollBehavior() })}><Icon icon={CaretRightIcon} size={16} /></button>
				{/if}
			</div>
		{/if}

		<!-- 3. Search + sort -->
		<div class="flex shrink-0 items-center justify-between gap-2 pr-6 pl-4 {mode === 'page' ? 'h-10' : 'mt-3 h-8'}">
			<div class="flex min-w-0 items-center">
				{#if libraryUi.searchOpen || mode === 'page'}
					<div class="relative flex h-8 w-[188px] min-w-0 items-center rounded-sm bg-bg-elevated-highlight">
						<span class="pl-2 text-text-subdued"><Icon icon={MagnifyingGlassIcon} size={16} /></span>
						<!-- svelte-ignore a11y_autofocus -->
						<input
							bind:this={searchInput}
							bind:value={libraryUi.query}
							autofocus={mode !== 'page' || libraryUi.searchOpen}
							placeholder="Search in Your Library"
							class="h-full min-w-0 flex-1 bg-transparent px-2 text-sm text-white outline-none placeholder:text-text-subdued"
							onkeydown={(e) => {
								if (e.key === 'Escape') {
									libraryUi.query = '';
									libraryUi.searchOpen = false;
								}
							}}
							onblur={() => {
								if (!libraryUi.query) libraryUi.searchOpen = false;
							}}
						/>
						{#if libraryUi.query}
							<button type="button" aria-label="Clear search" class="pr-2 text-text-subdued hover:text-white" onclick={() => (libraryUi.query = '')}>
								<Icon icon={XIcon} size={16} />
							</button>
						{/if}
					</div>
				{:else}
					<IconButton
						icon={MagnifyingGlassIcon}
						label="Search in Your Library"
						onclick={() => {
							libraryUi.searchOpen = true;
							libraryUi.searchSeq++;
						}}
					/>
				{/if}
			</div>
			<button
				type="button"
				class="flex h-8 shrink-0 items-center gap-2 text-sm text-text-subdued transition-transform hover:scale-[1.04] hover:text-white"
				onclick={sortMenu}
				aria-label="{SORTS.find((s) => s[0] === sort)?.[1]}, {VIEWS.find((v) => v[0] === view)?.[1]} view"
				aria-haspopup="menu"
			>
				{SORTS.find((s) => s[0] === sort)?.[1]}
				<Icon icon={viewIcon} size={16} />
			</button>
		</div>
		</div>

		{#if folder}
			<button
				type="button"
				class="mx-4 mt-2 flex items-center gap-2 text-sm font-bold text-white hover:underline"
				onclick={() => (libraryUi.folderPath = libraryUi.folderPath.slice(0, -1))}
			>
				<Icon icon={CaretLeftIcon} size={16} /> {folder.name}
			</button>
		{/if}
	{/if}

	<!-- 4. Items -->
	<div
		class="scroll-area relative min-h-0 flex-1 overflow-y-auto {collapsed ? 'px-2 pt-1' : 'px-2 pt-2'} pb-2"
		role={list.length ? 'grid' : undefined}
		aria-label={list.length ? 'Your Library' : undefined}
		{@attach roving({ items: '[data-lib-row]', orientation: !collapsed && (view === 'grid' || view === 'compact-grid') ? 'grid' : 'vertical' })}
	>
		{#if empty && !collapsed}
			<div class="flex flex-col gap-4 px-2 pt-2">
				<div class="rounded-lg bg-bg-highlight px-5 py-4">
					<div class="text-base font-bold text-white">Create your first playlist</div>
					<div class="mt-2 text-sm text-white">It's easy, we'll help you</div>
					<button type="button" class="btn-white sm mt-5" onclick={() => createPlaylist()}>Create playlist</button>
				</div>
				<div class="rounded-lg bg-bg-highlight px-5 py-4">
					<div class="text-base font-bold text-white">Let's find some podcasts to follow</div>
					<div class="mt-2 text-sm text-white">We'll keep you updated on new episodes</div>
					<button type="button" class="btn-white sm mt-5" onclick={() => goto('/search?cat=podcasts')}>Browse podcasts</button>
				</div>
			</div>
		{:else if !list.length && libraryUi.query}
			<div class="px-4 pt-6 text-center">
				<div class="text-base font-bold text-white">Couldn't find "{libraryUi.query}"</div>
				<div class="mt-2 text-sm text-text-subdued">Try searching again using a different spelling or keyword.</div>
			</div>
		{:else if collapsed}
			{#each list as r (r.id)}
				<Tooltip text={r.title} side="right">
					{#snippet trigger(props)}
						<div
							{...props}
							role="row"
							tabindex="-1"
							aria-label={r.title}
							aria-selected={isActive(r)}
							data-lib-row
							class="group/row flex w-full justify-center rounded-md p-2 hover:bg-white/10 {isActive(r) ? 'bg-white/10' : ''} {dropOn === r.id ? 'ring-1 ring-white' : ''}"
							onclick={() => open(r)}
							onkeydown={(e) => rowKey(e, r)}
							oncontextmenu={(e) => menuFor(e, r)}
							ondragover={(e) => dragOver(e, r)}
							ondragleave={() => (dropOn = null)}
							ondrop={(e) => drop(e, r)}
						>
							<div role="gridcell" class="contents"><LibraryCover row={r} size={48} /></div>
						</div>
					{/snippet}
				</Tooltip>
			{/each}
		{:else if view === 'grid' || view === 'compact-grid'}
			<div class="grid gap-x-0 gap-y-0" style="grid-template-columns:repeat({gridCols},minmax(0,1fr))">
				{#each list as r (r.id)}
					<div
						role="row"
						tabindex="-1"
						aria-selected={isActive(r)}
						data-lib-row
						draggable={r.kind !== 'liked' && r.kind !== 'episodes' && r.kind !== 'local'}
						class="group/row relative flex min-w-0 flex-col gap-2 rounded-md p-3 hover:bg-white/10 data-menu-open:bg-white/10 {isActive(r) ? 'bg-white/10' : ''} {dropOn === r.id ? 'ring-1 ring-white' : ''}"
						onclick={() => open(r)}
						onkeydown={(e) => rowKey(e, r)}
						oncontextmenu={(e) => menuFor(e, r)}
						ondragstart={(e) => dragStart(e, r)}
						ondragover={(e) => dragOver(e, r)}
						ondragleave={() => (dropOn = null)}
						ondrop={(e) => drop(e, r)}
						title={view === 'compact-grid' ? r.title : undefined}
						aria-label={view === 'compact-grid' ? r.title : undefined}
					>
						<div role="gridcell" class="contents">
						<div class="relative aspect-square w-full [&>div:first-child]:!size-full">
							<LibraryCover row={r} size={160} radius={6} />
							{#if r.kind !== 'folder' && view === 'grid'}
								<div class="absolute right-2 bottom-2 transition-[opacity,transform] duration-200 ease-out {playingId === r.id ? 'opacity-100' : 'translate-y-2 opacity-0 group-hover/row:translate-y-0 group-hover/row:opacity-100'}">
									<PlayButton size="md" playing={playingId === r.id && !playback.paused} onclick={() => play(r)} />
								</div>
							{/if}
						</div>
						{#if view === 'grid'}
							<div class="min-w-0">
								<div class="truncate text-base {playingId === r.id ? 'text-accent-text' : 'text-white'}">{r.title}</div>
								<div class="flex items-center gap-1 truncate text-sm text-text-subdued">
									{#if r.pinned}<Icon icon={PushPinIcon} size={12} fill class="text-accent" />{/if}{#if isRowDownloaded(r)}<Icon icon={ArrowCircleDownIcon} size={12} fill class="shrink-0 text-accent" />{/if}
									<span class="truncate">{subtitleOf(r)}</span>
								</div>
							</div>
						{/if}
						</div>
					</div>
				{/each}
			</div>
		{:else}
			{#if mode === 'page' && view !== 'compact'}
				<div role="row" class="mb-2 flex h-9 items-center gap-3 border-b border-white/10 px-2 text-sm text-text-subdued">
					<span role="columnheader" class="flex-1 pl-[60px]">Title</span>
					<span role="columnheader" class="w-40 shrink-0">Date added</span>
					<span role="columnheader" class="w-40 shrink-0">Played</span>
				</div>
			{/if}
			{#each list as r (r.id)}
				<div
					role="row"
					tabindex="-1"
					aria-selected={isActive(r)}
					data-lib-row
					draggable={r.kind !== 'liked' && r.kind !== 'episodes' && r.kind !== 'local'}
					class="group/row relative flex w-full items-center gap-3 rounded-md px-2 text-left hover:bg-white/10 data-menu-open:bg-white/10 {view === 'compact' ? 'h-8' : 'h-16'} {isActive(r) ? 'bg-white/10 hover:!bg-white/15' : ''} {dropOn === r.id ? 'ring-1 ring-white' : ''}"
					onclick={() => open(r)}
					onkeydown={(e) => rowKey(e, r)}
					oncontextmenu={(e) => menuFor(e, r)}
					ondragstart={(e) => dragStart(e, r)}
					ondragover={(e) => dragOver(e, r)}
					ondragleave={() => (dropOn = null)}
					ondrop={(e) => drop(e, r)}
				>
					<div role="gridcell" class="contents">
					{#if view === 'compact'}
						{#if r.pinned}<Icon icon={PushPinIcon} size={12} fill class="text-accent" />{/if}{#if isRowDownloaded(r)}<Icon icon={ArrowCircleDownIcon} size={12} fill class="shrink-0 text-accent" />{/if}
						<span class="min-w-0 flex-1 truncate">
							<span class="text-base {playingId === r.id ? 'text-accent-text' : 'text-white'}">{r.title}</span>
							<span class="text-sm text-text-subdued"> • {r.typeLabel}</span>
						</span>
					{:else}
						<div class="relative">
							<LibraryCover row={r} size={48} />
							{#if r.kind !== 'folder'}
								<button
									type="button"
									aria-label="Play {r.title}"
									class="absolute inset-0 hidden place-items-center bg-black/50 text-white group-hover/row:grid {r.kind === 'artist' ? 'rounded-full' : 'rounded-sm'}"
									onclick={(e) => {
										e.stopPropagation();
										play(r);
									}}
								>
									<Icon icon={playingId === r.id && !playback.paused ? PauseIcon : PlayIcon} size={20} fill />
								</button>
							{/if}
						</div>
						<div class="min-w-0 flex-1">
							<div class="truncate text-base {playingId === r.id ? 'text-accent-text' : 'text-white'}">{r.title}</div>
							<div class="flex items-center gap-1 truncate text-sm text-text-subdued">
								{#if r.pinned}<Icon icon={PushPinIcon} size={12} fill class="text-accent" />{/if}{#if isRowDownloaded(r)}<Icon icon={ArrowCircleDownIcon} size={12} fill class="shrink-0 text-accent" />{/if}
								<span class="truncate">{subtitleOf(r)}</span>
							</div>
						</div>
					{/if}
					{#if mode === 'page' && view !== 'compact'}
						<span class="w-40 shrink-0 truncate text-sm text-text-subdued">{fmtDate(r.addedAt)}</span>
						<span class="w-40 shrink-0 truncate text-sm text-text-subdued">{fmtAgo(r.playedAt)}</span>
					{/if}
					{@render nowPlayingGlyph(r)}
					</div>
				</div>
			{/each}
		{/if}
	</div>
</section>
