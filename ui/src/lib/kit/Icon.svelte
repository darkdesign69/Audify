<script lang="ts" module>
	import type { Component } from 'svelte';
	/** Any phosphor-svelte icon component. Always deep-import
	 *  (`import HouseIcon from 'phosphor-svelte/lib/HouseIcon'`); the barrel pulls in ~9k files. */
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	export type IconComponent = Component<any>;
</script>

<script lang="ts">
	/**
	 * Phosphor icon wrapper. Spotify's icons are 16px (rows, player, menus, sidebar), 24px (top bar,
	 * big play), 32px (action-bar secondary), 12px (pins). Regular weight by default; `fill` for the
	 * active state (house on Home, library, liked heart, play/pause glyphs).
	 * Colour is always `currentColor`, so set `text-*` on the icon or its parent.
	 */
	let {
		icon,
		size = 16,
		fill = false,
		weight,
		class: className = '',
		label
	}: {
		icon: IconComponent;
		size?: number;
		/** Shorthand for weight="fill". */
		fill?: boolean;
		weight?: 'regular' | 'fill' | 'bold'; // the only weights shipped (vite.config.ts, stream: perf)
		class?: string;
		/** Accessible name; omit for decorative icons (the usual case: the button has the label). */
		label?: string;
	} = $props();
	const Glyph = $derived(icon);
</script>

<Glyph
	size={size}
	weight={weight ?? (fill ? 'fill' : 'regular')}
	class="shrink-0 {className}"
	aria-hidden={label ? undefined : 'true'}
	aria-label={label}
/>
