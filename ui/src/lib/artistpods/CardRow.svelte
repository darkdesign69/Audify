<script lang="ts" generics="T">
	/**
	 * The kit Shelf's "fit" grid without its heading, for sections whose heading carries extra
	 * controls (Discography's chips) or grids that wrap (Show all pages, `rows = Infinity`).
	 */
	import type { Snippet } from 'svelte';
	let {
		items,
		card,
		key,
		minCard = 180,
		rows = 1
	}: {
		items: T[];
		card: Snippet<[T, number]>;
		key?: (item: T) => string;
		minCard?: number;
		rows?: number;
	} = $props();
	let width = $state(0);
	const columns = $derived(Math.max(2, Math.floor((width + 24) / minCard)));
	const shown = $derived(rows === Infinity ? items : items.slice(0, columns * rows));
</script>

<div bind:clientWidth={width}>
	<div class="-mx-3 grid" style="grid-template-columns:repeat({columns}, minmax(0, 1fr))">
		{#each shown as item, i (key ? key(item) : i)}
			{@render card(item, i)}
		{/each}
	</div>
</div>
