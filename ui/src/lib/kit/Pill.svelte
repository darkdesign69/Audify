<script lang="ts">
	/**
	 * Pills. `chip` = filter chip (32px, white/10 → white bg + black text when `selected`),
	 * `outline` = secondary action (Follow / Add / Edit: 1px #7c7c7c border, 14/700),
	 * `tinted` = owner action pill (+ Add, Name & details: white/10, 14/700),
	 * `white` = primary action (Create playlist, Save; `large` → 48px).
	 */
	import type { Snippet } from 'svelte';
	import Icon, { type IconComponent } from './Icon.svelte';

	let {
		variant = 'chip',
		selected = false,
		large = false,
		icon,
		disabled = false,
		class: className = '',
		onclick,
		children
	}: {
		variant?: 'chip' | 'outline' | 'tinted' | 'white';
		selected?: boolean;
		large?: boolean;
		icon?: IconComponent;
		disabled?: boolean;
		class?: string;
		onclick?: (e: MouseEvent) => void;
		children: Snippet;
	} = $props();

	const cls = $derived(
		variant === 'chip'
			? 'chip gap-1'
			: variant === 'outline'
				? 'btn-outline'
				: variant === 'white'
					? `btn-white ${large ? '' : 'sm'}`
					: 'inline-flex h-8 items-center gap-2 rounded-full bg-bg-tinted px-3 text-sm font-bold text-white transition-transform duration-50 hover:scale-[1.04] hover:bg-bg-tinted-highlight'
	);
</script>

<button
	type="button"
	class="{cls} disabled:pointer-events-none disabled:opacity-30 {className}"
	aria-pressed={variant === 'chip' ? selected : undefined}
	{disabled}
	{onclick}
>
	{#if icon}<Icon {icon} size={16} />{/if}
	{@render children()}
</button>
