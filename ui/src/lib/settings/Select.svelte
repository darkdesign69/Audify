<script lang="ts" generics="T extends string">
	/**
	 * Spotify's settings dropdown (settings.md): 32px, bg #1f1f1f (hover #2a2a2a), radius 4,
	 * padding 0 20px 0 12px... we keep 12px + chevron 16px, 14px text. The list is the kit menu
	 * (#282828, 40px items, check on the current one) opened under the button.
	 *
	 * Props: value, options [{value,label}], label (aria), width?, disabled?, onchange(value)
	 */
	import Icon from '$lib/kit/Icon.svelte';
	import { openMenu } from '$lib/kit/menus';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';

	let {
		value,
		options,
		label,
		width,
		disabled = false,
		onchange
	}: {
		value: T;
		options: { value: T; label: string }[];
		label: string;
		width?: number;
		disabled?: boolean;
		onchange?: (v: T) => void;
	} = $props();

	const current = $derived(options.find((o) => o.value === value)?.label ?? String(value ?? ''));
	let btn = $state<HTMLButtonElement>();

	async function open(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		const el = btn!;
		// A long list (EQ presets) needs room under the button: bring the row up first.
		const need = Math.min(options.length * 40 + 16, 0.7 * innerHeight);
		if (innerHeight - el.getBoundingClientRect().bottom < need + 96) {
			el.scrollIntoView({ block: 'start', behavior: 'instant' as ScrollBehavior });
			el.closest('[data-main-scroll]')?.scrollBy({ top: -96, behavior: 'instant' as ScrollBehavior });
			await new Promise(requestAnimationFrame);
		}
		openMenu(
			el,
			options.map((o) => ({
				label: o.label,
				checked: o.value === value,
				onSelect: () => {
					if (o.value !== value) onchange?.(o.value);
				}
			})),
			{ align: 'end', side: 'bottom', width: Math.max(160, btn?.offsetWidth ?? 160) }
		);
	}
</script>

<button
	bind:this={btn}
	type="button"
	aria-label="{label}, {current}"
	aria-haspopup="menu"
	{disabled}
	class="inline-flex h-8 max-w-full items-center justify-between gap-3 rounded-sm bg-bg-highlight pr-3 pl-3 text-sm text-text-subdued transition-colors hover:bg-bg-elevated-highlight hover:text-white data-[menu-open]:text-white disabled:pointer-events-none disabled:opacity-40 data-[menu-open]:bg-bg-elevated-highlight"
	style={width ? `width:${width}px` : undefined}
	onclick={open}
>
	<span class="truncate">{current}</span>
	<Icon icon={CaretDownIcon} size={16} class="shrink-0 text-text-subdued" />
</button>
