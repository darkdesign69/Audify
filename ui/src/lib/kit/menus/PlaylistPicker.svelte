<script lang="ts">
	/**
	 * "Add to playlist ▸" submenu body (menus.md): search input "Find a playlist" (338x32, white/10,
	 * radius 4, magnifier at left 8), "+ New playlist", separator, then the user's editable
	 * playlists (recent first, text only, 40px rows). Typing filters instantly.
	 */
	import { DropdownMenu } from 'bits-ui';
	import Icon from '../Icon.svelte';
	import PlusIcon from 'phosphor-svelte/lib/PlusIcon';
	import MagnifyingGlassIcon from 'phosphor-svelte/lib/MagnifyingGlassIcon';
	import type { SongItem } from '$lib/api';
	import { addToPlaylist, createPlaylist, editablePlaylists } from '../actions';
	import { closeMenu } from './state.svelte';

	let { songs, itemClass }: { songs: SongItem[]; itemClass: string } = $props();
	let q = $state('');
	const list = $derived.by(() => {
		const all = editablePlaylists();
		const needle = q.trim().toLowerCase();
		return needle ? all.filter((p) => p.title.toLowerCase().includes(needle)) : all;
	});
</script>

<div class="relative px-0 pb-1">
	<Icon
		icon={MagnifyingGlassIcon}
		size={16}
		class="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-white/70"
	/>
	<!-- svelte-ignore a11y_autofocus -->
	<input
		bind:value={q}
		placeholder="Find a playlist"
		class="h-8 w-full rounded-sm bg-white/10 py-2 pr-2 pl-8 text-sm text-white placeholder:text-white/60 outline-none focus:bg-white/15"
		onkeydown={(e) => {
			// Typing letters must stay in the field (bits-ui typeahead would steal them).
			if (e.key.length === 1 || e.key === 'Backspace') e.stopPropagation();
		}}
	/>
</div>
<DropdownMenu.Item
	class={itemClass}
	onSelect={() => {
		closeMenu();
		createPlaylist(songs, false);
	}}
>
	<Icon icon={PlusIcon} size={16} />
	<span class="flex-1">New playlist</span>
</DropdownMenu.Item>
<DropdownMenu.Separator class="my-1 h-px bg-white/10" />
<div class="max-h-[min(60vh,420px)] overflow-y-auto">
	{#each list as p (p.id)}
		<DropdownMenu.Item
			class={itemClass}
			onSelect={() => {
				closeMenu();
				addToPlaylist(p, songs);
			}}
		>
			<span class="min-w-0 flex-1 truncate">{p.title}</span>
		</DropdownMenu.Item>
	{:else}
		<div class="px-3 py-3 text-sm text-white/60">No playlists found</div>
	{/each}
</div>
