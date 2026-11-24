<script lang="ts">
	/**
	 * Loading placeholders (spec: states.md). Static #282828 squares + white/10 bars (radius 10),
	 * with a subtle opacity pulse (.6 ↔ 1, 1.5s).
	 *
	 * Props
	 *   variant: 'row' | 'card' | 'header' | 'library-row' | 'bar' (default 'bar')
	 *   count?: number      repeat (rows, cards, library rows); default 1
	 *   width?: string      'bar' only, CSS width (default '40%'); height?: number (default 12)
	 *   class?: string
	 *
	 * Usage: `<Skeleton variant="row" count={8} />`, `<Skeleton variant="header" />`,
	 *        cards in a grid: `<div class="grid grid-cols-5"><Skeleton variant="card" count={5} /></div>`
	 */
	let {
		variant = 'bar',
		count = 1,
		width = '40%',
		height = 12,
		class: className = ''
	}: {
		variant?: 'row' | 'card' | 'header' | 'library-row' | 'bar';
		count?: number;
		width?: string;
		height?: number;
		class?: string;
	} = $props();
	const n = $derived(Array.from({ length: Math.max(1, count) }, (_, i) => i));
	// Vary bar widths a little so a list of placeholders doesn't look stamped.
	const w = (i: number, base: number) => `${base - ((i * 7) % 15)}%`;
</script>

{#if variant === 'bar'}
	<span
		class="sk block rounded-[10px] bg-bg-tinted {className}"
		style="width:{width};height:{height}px"
	></span>
{:else if variant === 'header'}
	<div class="sk flex items-end gap-6 px-[var(--content-spacing)] pt-[60px] pb-6 {className}">
		<div class="aspect-square shrink-0 rounded-sm bg-bg-menu" style="width:clamp(128px, 28%, 232px)"></div>
		<div class="flex min-w-0 flex-1 flex-col gap-4 pb-2">
			<span class="block h-3.5 w-20 rounded-[10px] bg-bg-tinted"></span>
			<span class="block h-14 w-3/5 rounded-[10px] bg-bg-tinted"></span>
			<span class="block h-3.5 w-[30%] rounded-[10px] bg-bg-tinted"></span>
		</div>
	</div>
{:else}
	{#each n as i (i)}
		{#if variant === 'row'}
			<div class="sk flex h-14 items-center gap-3 px-4 {className}">
				<span class="block w-4 shrink-0"></span>
				<span class="block size-10 shrink-0 rounded-sm bg-bg-menu"></span>
				<div class="flex min-w-0 flex-1 flex-col gap-2">
					<span class="block h-3 rounded-[10px] bg-bg-tinted" style="width:{w(i, 40)}"></span>
					<span class="block h-2.5 rounded-[10px] bg-bg-tinted" style="width:{w(i, 25)}"></span>
				</div>
			</div>
		{:else if variant === 'library-row'}
			<div class="sk flex h-16 items-center gap-3 p-2 {className}">
				<span class="block size-12 shrink-0 rounded-sm bg-bg-menu"></span>
				<div class="flex min-w-0 flex-1 flex-col gap-2">
					<span class="block h-3 rounded-[10px] bg-bg-tinted" style="width:{w(i, 70)}"></span>
					<span class="block h-2.5 rounded-[10px] bg-bg-tinted" style="width:{w(i, 45)}"></span>
				</div>
			</div>
		{:else}
			<div class="sk flex min-w-0 flex-col gap-2 p-3 {className}">
				<span class="block aspect-square w-full rounded-md bg-bg-menu"></span>
				<span class="mt-1 block h-3 rounded-[10px] bg-bg-tinted" style="width:{w(i, 75)}"></span>
				<span class="block h-2.5 rounded-[10px] bg-bg-tinted" style="width:{w(i, 50)}"></span>
			</div>
		{/if}
	{/each}
{/if}

<style>
	.sk {
		animation: sk-pulse 1.5s ease-in-out infinite;
	}
	@keyframes sk-pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.6;
		}
	}
</style>
