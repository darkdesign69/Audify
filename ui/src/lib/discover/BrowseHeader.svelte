<script lang="ts">
	/**
	 * Header for Browse category pages (genre/mood, Charts, New releases, Podcasts): the tile's
	 * colour as the header tint, gradient into #121212 under the content (like entity headers),
	 * noise, and a big 800-weight title that steps down by length (96 → 72 → 48px).
	 */
	import type { Snippet } from 'svelte';

	let {
		title,
		color,
		eyebrow,
		hero = false,
		children
	}: {
		title: string;
		color?: string | null;
		eyebrow?: string;
		/** Genre/mood page: Spotify's tall hero (350px; 96px title top 224px below the panel top). */
		hero?: boolean;
		children?: Snippet;
	} = $props();
	const tint = $derived(color || 'rgb(83 83 83)');
	const size = $derived(title.length > 28 ? 'text-3xl' : title.length > 14 ? 'text-4xl' : 'text-5xl');
</script>

<!-- The page root must be `relative isolate`: the wash sits behind the whole page top. -->
<div>
	<div class="absolute inset-x-0 top-0 -z-10 h-[460px] rounded-t-lg" style="background-color:{tint}">
		<div class="absolute inset-0 bg-[linear-gradient(transparent_0,rgba(0,0,0,.5)_50%,#121212_100%)]"></div>
		<div class="noise absolute inset-0"></div>
	</div>
	<header
		class="flex flex-col justify-end px-[var(--content-spacing)] pt-16 pb-6 {hero ? 'min-h-[350px]' : 'min-h-[232px]'}"
	>
		{#if eyebrow}<span class="text-sm text-white">{eyebrow}</span>{/if}
		<h1 class="font-title leading-[1.05] font-extrabold break-words text-white {size}">{title}</h1>
		{#if children}<div class="mt-4">{@render children()}</div>{/if}
	</header>
</div>
