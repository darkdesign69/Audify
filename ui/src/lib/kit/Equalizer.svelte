<script lang="ts">
	/** The playing track's animated equaliser: 4 accent bars, ~14px. Static (paused) bars when
	 *  `playing` is false. */
	let { size = 14, playing = true, class: className = '' }: { size?: number; playing?: boolean; class?: string } =
		$props();
	const delays = [0, -0.35, -0.7, -0.2];
</script>

<span
	class="inline-flex shrink-0 items-end justify-between {className}"
	style="width:{size}px;height:{size}px"
	aria-label="Playing"
	role="img"
>
	{#each delays as d, i (i)}
		<span
			class="block w-[2px] origin-bottom rounded-[1px] bg-accent"
			style="height:100%;{playing
				? `animation: eq 1.1s ease-in-out ${d}s infinite`
				: `transform: scaleY(${[0.4, 0.8, 0.55, 0.3][i]})`}"
		></span>
	{/each}
</span>
