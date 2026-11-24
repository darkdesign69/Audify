<script lang="ts">
	/**
	 * Smart Shuffle glyph (icons.md: "Shuffle + Sparkle"): the shuffle arrows with a small
	 * four-point sparkle top-left. Drop-in for a Phosphor icon in `<Icon>` / `IconButton`.
	 */
	import ShuffleIcon from 'phosphor-svelte/lib/ShuffleIcon';

	let {
		size = 16,
		weight = 'regular',
		class: className = '',
		...rest
	}: { size?: number | string; weight?: string; class?: string; [k: string]: unknown } = $props();
	const px = $derived(typeof size === 'number' ? size : parseInt(String(size), 10) || 16);
</script>

<span class="relative inline-grid shrink-0 place-items-center {className}" style="width:{px}px;height:{px}px">
	<ShuffleIcon size={px} weight={weight as 'regular'} {...rest} />
	<svg
		viewBox="0 0 16 16"
		width={Math.round(px * 0.5)}
		height={Math.round(px * 0.5)}
		class="absolute"
		style="left:{-px * 0.12}px;top:{-px * 0.14}px"
		aria-hidden="true"
	>
		<path fill="currentColor" d="M8 0c.4 3.9 2.1 5.6 6 6-3.9.4-5.6 2.1-6 6-.4-3.9-2.1-5.6-6-6 3.9-.4 5.6-2.1 6-6z" />
	</svg>
</span>
