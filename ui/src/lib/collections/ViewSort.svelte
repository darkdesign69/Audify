<script lang="ts">
	/**
	 * The right side of a playlist action bar: "List ☰" text button (14px white/70, 32px,
	 * radius 4) opening Spotify's menu: "Sort by" (Custom order, Title, Artist, Album, Date added,
	 * Duration — picking the current key flips its direction) and "View as" (Compact, List).
	 * An album shows only the View as part ("List ☰").
	 */
	import { Icon, openMenu, type MenuEntry, type TrackSort, type TrackSortKey } from '$lib/kit';
	import ListBulletsIcon from 'phosphor-svelte/lib/ListBulletsIcon';
	import ListIcon from 'phosphor-svelte/lib/ListIcon';
	import ArrowUpIcon from 'phosphor-svelte/lib/ArrowUpIcon';
	import ArrowDownIcon from 'phosphor-svelte/lib/ArrowDownIcon';

	let {
		sort = $bindable(null),
		compact = $bindable(false),
		keys = ['title', 'artist', 'album', 'added', 'duration'],
		custom = true,
		sortable = true
	}: {
		sort?: TrackSort;
		compact?: boolean;
		keys?: TrackSortKey[];
		/** Offer "Custom order" (the playlist's own order). Liked Songs has none. */
		custom?: boolean;
		sortable?: boolean;
	} = $props();

	const LABEL: Record<TrackSortKey, string> = {
		title: 'Title',
		artist: 'Artist',
		album: 'Album',
		added: 'Date added',
		duration: 'Duration'
	};
	// Unsorted, the button reads the view ("List ☰" / "Compact"), as on Spotify's playlists,
	// Liked Songs and albums; a picked sort shows its name.
	const label = $derived(sortable && sort ? LABEL[sort.key] : compact ? 'Compact' : 'List');

	function pick(key: TrackSortKey) {
		if (sort?.key === key) sort = { key, dir: sort.dir === 'asc' ? 'desc' : 'asc' };
		else sort = { key, dir: key === 'added' ? 'desc' : 'asc' };
	}

	function open(e: MouseEvent) {
		const entries: MenuEntry[] = [];
		if (sortable) {
			entries.push({ type: 'heading', label: 'Sort by' });
			if (custom) entries.push({ label: 'Custom order', checked: !sort, onSelect: () => (sort = null) });
			for (const k of keys)
				entries.push({
					label: LABEL[k],
					checked: sort?.key === k,
					icon: sort?.key === k ? (sort.dir === 'asc' ? ArrowUpIcon : ArrowDownIcon) : undefined,
					onSelect: () => pick(k)
				});
			entries.push({ type: 'separator' });
		}
		entries.push({ type: 'heading', label: 'View as' });
		entries.push({ label: 'Compact', icon: ListIcon, checked: compact, onSelect: () => (compact = true) });
		entries.push({ label: 'List', icon: ListBulletsIcon, checked: !compact, onSelect: () => (compact = false) });
		openMenu(e.currentTarget as HTMLElement, entries, { align: 'end', width: 200 });
	}
</script>

<button
	type="button"
	class="flex h-8 items-center gap-2 rounded-sm px-2 text-sm text-white/70 transition-transform duration-50 hover:scale-[1.04] hover:text-white"
	onclick={open}
>
	<span>{label}</span>
	<Icon icon={compact ? ListIcon : ListBulletsIcon} size={16} />
</button>
