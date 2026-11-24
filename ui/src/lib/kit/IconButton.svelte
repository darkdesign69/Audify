<script lang="ts">
	/**
	 * Icon-only button with a tooltip (every icon button in Spotify has one).
	 * Default look: #b3b3b3 → white + scale 1.04 on hover. `active` = accent icon, plus a 4px accent
	 * dot under it when `dot` (player-bar toggles: shuffle, repeat, lyrics, queue…).
	 * Sizes: 32px hit area with 16px icon (player, rows) by default; `size="lg"` 32px icon in a
	 * 48px box (action bar); `variant="filled"` = round #1f1f1f well (top bar, sidebar create).
	 */
	import Tooltip from './Tooltip.svelte';
	import Icon, { type IconComponent } from './Icon.svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';

	let {
		icon,
		label,
		tooltip,
		tooltipSide = 'top',
		active = false,
		dot = false,
		fill = false,
		iconSize,
		size = 'md',
		variant = 'ghost',
		tone = 'subdued',
		disabled = false,
		class: className = '',
		onclick,
		ref = $bindable(null),
		...rest
	}: {
		icon: IconComponent;
		/** aria-label, and the tooltip text unless `tooltip` is given. */
		label: string;
		tooltip?: string | null;
		tooltipSide?: 'top' | 'bottom' | 'left' | 'right';
		active?: boolean;
		dot?: boolean;
		fill?: boolean;
		iconSize?: number;
		size?: 'sm' | 'md' | 'lg';
		variant?: 'ghost' | 'filled';
		/** Rest colour: subdued #b3b3b3 (default) or muted white/70 (player bar right side). */
		tone?: 'subdued' | 'muted' | 'white';
		disabled?: boolean;
		class?: string;
		onclick?: (e: MouseEvent) => void;
		ref?: HTMLButtonElement | null;
	} & Omit<HTMLButtonAttributes, 'onclick'> = $props();

	const box = $derived(
		variant === 'filled'
			? size === 'lg'
				? 'size-12'
				: size === 'sm'
					? 'size-8'
					: 'size-[35px]'
			: size === 'lg'
				? 'size-12'
				: size === 'sm'
					? 'size-6'
					: 'size-8'
	);
	const glyph = $derived(iconSize ?? (size === 'lg' ? (variant === 'filled' ? 24 : 32) : 16));
	const rest_tone = $derived(
		tone === 'muted' ? 'text-white/70' : tone === 'white' ? 'text-white' : 'text-text-subdued'
	);
</script>

<Tooltip text={tooltip === undefined ? label : tooltip} side={tooltipSide}>
	{#snippet trigger(props)}
		<button
			{...props}
			{...rest}
			bind:this={ref}
			type="button"
			aria-label={label}
			aria-pressed={active || undefined}
			{disabled}
			{onclick}
			class="relative grid shrink-0 place-items-center rounded-full transition-[color,transform,background-color] duration-100 ease-productive hover:scale-[1.04] active:scale-100 active:opacity-70 disabled:pointer-events-none disabled:opacity-30 {box} {variant ===
			'filled'
				? 'bg-bg-highlight hover:bg-bg-elevated-highlight'
				: ''} {active ? 'text-accent hover:text-accent-hover' : `${rest_tone} hover:text-white`} {className}"
		>
			<Icon {icon} size={glyph} fill={fill} />
			{#if active && dot}
				<span class="absolute bottom-0 left-1/2 size-1 -translate-x-1/2 rounded-full bg-accent"
				></span>
			{/if}
		</button>
	{/snippet}
</Tooltip>
