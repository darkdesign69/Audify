<script lang="ts">
	// Artist ▸ Discography ▸ Show all (Spotify's /artist/<id>/discography/<all|album|single|
	// compilation>, checked live 2026-11). A sticky row: the artist's name, then "All ▾" (All ·
	// Albums · Singles and EPs · Compilations, the last only when there are any) and "Release date
	// ☰" (Sort by: Release date ↓ / Name; View as: List / Grid). List is the default and shows each
	// release with its songs, playable in place; Grid is the card wall. Newest first.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { Card, ErrorState, Icon, Skeleton, actions, openItemMenu, openMenu, type MenuEntry } from '$lib/kit';
	import { artistPage, playItem } from '$lib/browse';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';
	import ListBulletsIcon from 'phosphor-svelte/lib/ListBulletsIcon';
	import SquaresFourIcon from 'phosphor-svelte/lib/SquaresFourIcon';
	import ArrowDownIcon from 'phosphor-svelte/lib/ArrowDownIcon';
	import ArrowUpIcon from 'phosphor-svelte/lib/ArrowUpIcon';
	import CardRow from '$lib/artistpods/CardRow.svelte';
	import ReleaseBlock from '$lib/artistpods/ReleaseBlock.svelte';
	import {
		EMPTY_DISCOGRAPHY,
		FILTER_LABEL,
		availableFilters,
		sortReleases,
		type Discography,
		type SortDir,
		type SortKey
	} from '$lib/artistpods/discography';
	import { fullDiscography } from '$lib/artistpods/discographyData';
	import { isPlayingContext, playOrToggle } from '$lib/artistpods/playing';

	const id = $derived(page.params.id ?? '');
	const FILTERS = ['all', 'album', 'single', 'compilation'] as const;
	type PageFilter = (typeof FILTERS)[number];
	const type = $derived<PageFilter>(
		(FILTERS as readonly string[]).includes(page.params.type ?? '') ? (page.params.type as PageFilter) : 'all'
	);

	let name = $state('');
	let disco = $state.raw<Discography>(EMPTY_DISCOGRAPHY);
	let loading = $state(true);
	let error = $state(false);

	// Sort and view, remembered like Spotify does (best effort).
	const PREFS = 'spotless.discography';
	type Prefs = { key: SortKey; dir: SortDir; grid: boolean };
	function readPrefs(): Prefs {
		try {
			const p = JSON.parse(localStorage.getItem(PREFS) ?? 'null') as Partial<Prefs> | null;
			if (p && (p.key === 'date' || p.key === 'name') && (p.dir === 'asc' || p.dir === 'desc'))
				return { key: p.key, dir: p.dir, grid: !!p.grid };
		} catch {
			/* defaults */
		}
		return { key: 'date', dir: 'desc', grid: false };
	}
	const initial = readPrefs();
	let sortKey = $state<SortKey>(initial.key);
	let sortDir = $state<SortDir>(initial.dir);
	let grid = $state(initial.grid);
	$effect(() => {
		const p: Prefs = { key: sortKey, dir: sortDir, grid };
		try {
			localStorage.setItem(PREFS, JSON.stringify(p));
		} catch {
			/* not remembered */
		}
	});

	async function load(cid: string) {
		loading = true;
		error = false;
		const a = await artistPage(cid);
		if (cid !== id) return;
		if (!a) {
			error = true;
			loading = false;
			return;
		}
		name = a.name ?? '';
		try {
			const d = await fullDiscography(a);
			if (cid !== id) return;
			disco = d;
		} catch {
			if (cid === id) error = true;
		}
		if (cid === id) loading = false;
	}
	$effect(() => {
		if (id) load(id);
	});

	const filters = $derived(availableFilters(disco, false) as PageFilter[]);
	const shown = $derived(sortReleases(disco[type], sortKey, sortDir));
	const base = $derived(`/artist/${encodeURIComponent(id)}/discography`);
	const scroller =
		typeof document !== 'undefined' ? document.querySelector<HTMLElement>('[data-main-scroll]') : null;

	function openFilter(e: MouseEvent) {
		const entries: MenuEntry[] = (filters.length ? filters : ['all' as const]).map((f) => ({
			label: FILTER_LABEL[f],
			checked: f === type,
			onSelect: () => goto(`${base}/${f}`, { replaceState: true, noScroll: true })
		}));
		openMenu(e.currentTarget as HTMLElement, entries, { align: 'end', width: 200 });
	}

	function pickSort(key: SortKey) {
		if (sortKey === key) sortDir = sortDir === 'asc' ? 'desc' : 'asc';
		else {
			sortKey = key;
			sortDir = key === 'date' ? 'desc' : 'asc';
		}
	}
	const arrow = (k: SortKey) => (sortKey === k ? (sortDir === 'asc' ? ArrowUpIcon : ArrowDownIcon) : undefined);

	function openView(e: MouseEvent) {
		const entries: MenuEntry[] = [
			{ type: 'heading', label: 'Sort by' },
			{ label: 'Release date', checked: sortKey === 'date', icon: arrow('date'), onSelect: () => pickSort('date') },
			{ label: 'Name', checked: sortKey === 'name', icon: arrow('name'), onSelect: () => pickSort('name') },
			{ type: 'heading', label: 'View as' },
			{ label: 'List', icon: ListBulletsIcon, checked: !grid, onSelect: () => (grid = false) },
			{ label: 'Grid', icon: SquaresFourIcon, checked: grid, onSelect: () => (grid = true) }
		];
		openMenu(e.currentTarget as HTMLElement, entries, { align: 'end', width: 200 });
	}
</script>

<svelte:head><title>{name ? `${name} – Discography` : 'Discography'}</title></svelte:head>

<div class="flex flex-col pb-10">
	<div
		class="sticky top-0 z-10 flex h-16 items-center gap-4 bg-bg-base px-[var(--content-spacing)]"
	>
		<h1 class="min-w-0 flex-1 truncate font-title text-2xl font-bold text-white">
			<a href={`/artist/${encodeURIComponent(id)}`} class="hover:underline">{name || ' '}</a>
		</h1>
		<button
			type="button"
			class="flex h-8 items-center gap-2 rounded-sm px-2 text-sm text-white/70 transition-transform duration-50 hover:scale-[1.04] hover:text-white"
			aria-haspopup="menu"
			aria-label={`Filter: ${FILTER_LABEL[type]}`}
			onclick={openFilter}
		>
			<span>{FILTER_LABEL[type]}</span>
			<Icon icon={CaretDownIcon} size={16} weight="fill" />
		</button>
		<button
			type="button"
			class="flex h-8 items-center gap-2 rounded-sm px-2 text-sm text-white/70 transition-transform duration-50 hover:scale-[1.04] hover:text-white"
			aria-haspopup="menu"
			aria-label={`Sort by ${sortKey === 'date' ? 'release date' : 'name'}, view as ${grid ? 'grid' : 'list'}`}
			onclick={openView}
		>
			<span>{sortKey === 'date' ? 'Release date' : 'Name'}</span>
			<Icon icon={grid ? SquaresFourIcon : ListBulletsIcon} size={16} />
		</button>
	</div>

	<div class="px-[var(--content-spacing)] pt-6">
		{#if loading}
			<div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4"><Skeleton variant="card" count={10} /></div>
		{:else if error}
			<ErrorState onRetry={() => load(id)} />
		{:else if !shown.length}
			<p class="py-10 text-center text-base text-text-subdued">Nothing here yet.</p>
		{:else if grid}
			<CardRow items={shown} rows={Infinity} key={(r) => r.item.id}>
				{#snippet card(r)}
					<Card
						title={r.item.title}
						subtitle={r.line}
						image={r.item.thumbnail}
						kind="album"
						href={actions.hrefFor(r.item)}
						playing={isPlayingContext(r.item.id)}
						onPlay={() => playOrToggle([r.item.id], () => playItem(r.item))}
						oncontextmenu={(ev) => openItemMenu(ev, r.item)}
					/>
				{/snippet}
			</CardRow>
		{:else}
			<div class="flex flex-col gap-12">
				{#each shown as r (r.item.id)}
					<ReleaseBlock release={r} {scroller} />
				{/each}
			</div>
		{/if}
	</div>
</div>
