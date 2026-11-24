<script lang="ts">
	/** Spotify's default playlist cover: a 2x2 mosaic of the first four distinct album covers. */
	import { thumb } from '$lib/thumb';
	let { covers, px = 300 }: { covers: string[]; px?: number } = $props();
</script>

<div class="grid size-full grid-cols-2 grid-rows-2 bg-bg-menu">
	{#each covers.slice(0, 4) as c, i (i)}
		<img src={thumb(c, px)} alt="" class="size-full object-cover" draggable="false" />
	{/each}
</div>
