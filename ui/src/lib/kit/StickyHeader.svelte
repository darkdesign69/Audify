<script lang="ts">
	/**
	 * The 64px bar that fades in at the top of the main view once the action bar's play button has
	 * scrolled under it (spec: playlist.md §4, interactions-motion "Sticky header threshold").
	 * Place it as the FIRST child inside the page (it positions itself `sticky top-0` with zero
	 * layout height, so the content below does not move).
	 *
	 * Props
	 *   title: string          24/700, ellipsis; click scrolls the container to top
	 *   tint?: string          bar colour (rgba(0,0,0,.6) overlay is added); default neutral grey
	 *   playing?: boolean
	 *   onPlay?: () => void    omit to hide the 48px play button
	 *   sentinel?: HTMLElement when this element's bottom passes under the bar, the bar shows.
	 *                          (Usually ActionBar's bindable `sentinel`.) Without one, shows after 300px.
	 *   scroller?: HTMLElement the scroll container; default = nearest scrolling ancestor
	 *   visible?: boolean      bindable out: current state (e.g. to pin a table header under it)
	 *   children?: Snippet     extra content at the right of the bar
	 *
	 * Usage
	 *   <StickyHeader title={p.title} tint={tint.color} {playing} onPlay={play} {sentinel} bind:visible />
	 */
	import { scrollBehavior } from './motion';
	import type { Snippet } from 'svelte';
	import PlayButton from './PlayButton.svelte';
	import { NEUTRAL_TINT } from './tint.svelte';

	let {
		title,
		tint,
		playing = false,
		onPlay,
		sentinel,
		scroller,
		visible = $bindable(false),
		children
	}: {
		title: string;
		tint?: string | null;
		playing?: boolean;
		onPlay?: () => void;
		sentinel?: HTMLElement | null;
		scroller?: HTMLElement | null;
		visible?: boolean;
		children?: Snippet;
	} = $props();

	let anchor = $state<HTMLElement | null>(null);

	function scrollParent(el: HTMLElement | null): HTMLElement | null {
		for (let p = el?.parentElement; p; p = p.parentElement) {
			const o = getComputedStyle(p).overflowY;
			if (o === 'auto' || o === 'scroll') return p;
		}
		return null;
	}

	let box = $derived(scroller ?? scrollParent(anchor));

	$effect(() => {
		const sc = box;
		if (!sc) return;
		const update = () => {
			if (sentinel) {
				const top = sc.getBoundingClientRect().top;
				visible = sentinel.getBoundingClientRect().bottom <= top + 64;
			} else visible = sc.scrollTop > 300;
		};
		update();
		sc.addEventListener('scroll', update, { passive: true });
		return () => sc.removeEventListener('scroll', update);
	});
</script>

<div bind:this={anchor} class="pointer-events-none sticky top-0 z-[2] h-0">
	<div
		class="absolute inset-x-0 top-0 flex h-16 items-center gap-2 overflow-hidden px-[calc(var(--content-spacing)-4px)] transition-opacity duration-250 ease-out {visible
			? 'pointer-events-auto opacity-100'
			: 'opacity-0'}"
		style="background-color:{tint ?? NEUTRAL_TINT};transition-property:opacity,background-color"
		inert={!visible}
	>
		<div class="absolute inset-0 bg-black/60"></div>
		{#if onPlay}
			<div class="relative">
				<PlayButton size="md" {playing} onclick={() => onPlay()} />
			</div>
		{/if}
		<button
			type="button"
			tabindex={visible ? 0 : -1}
			class="relative min-w-0 truncate text-left text-xl font-bold text-white"
			onclick={() => box?.scrollTo({ top: 0, behavior: scrollBehavior() })}
		>
			{title}
		</button>
		{#if children}<div class="relative ml-auto flex items-center">{@render children()}</div>{/if}
	</div>
</div>
