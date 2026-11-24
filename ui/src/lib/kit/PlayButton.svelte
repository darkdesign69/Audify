<script lang="ts">
	/**
	 * Accent circular play/pause. Sizes: `lg` 56px (entity action bar), `md` 48px (cards, sticky
	 * header, sidebar grid), `sm` 32px (shortcut tiles). Hover: accent-hover + scale 1.04 (50ms).
	 * `playing` shows the pause glyph (pass true when this button's context is the one playing and
	 * not paused). Stops click propagation so it can sit on a clickable card.
	 * `tone="white"`: the white variant Spotify uses on list rows (What's New, episode rows).
	 */
	import Icon from './Icon.svelte';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import Tooltip from './Tooltip.svelte';

	let {
		size = 'lg',
		playing = false,
		label,
		tooltip = true,
		tone = 'accent',
		onclick,
		class: className = ''
	}: {
		size?: 'sm' | 'md' | 'lg';
		playing?: boolean;
		/** e.g. "Play Liked Songs"; defaults to "Play" / "Pause". */
		label?: string;
		tooltip?: boolean;
		tone?: 'accent' | 'white';
		onclick?: (e: MouseEvent) => void;
		class?: string;
	} = $props();

	const px = $derived(size === 'lg' ? 56 : size === 'md' ? 48 : 32);
	const glyph = $derived(size === 'sm' ? 16 : 24);
	const name = $derived(label ?? (playing ? 'Pause' : 'Play'));
</script>

<Tooltip text={tooltip ? name : null}>
	{#snippet trigger(props)}
		<button
			{...props}
			type="button"
			aria-label={name}
			class="btn-play shrink-0 {tone === 'white'
				? 'bg-white hover:bg-white active:bg-text-subdued active:scale-100'
				: ''} {className}"
			style="width:{px}px;height:{px}px"
			onclick={(e) => {
				e.stopPropagation();
				onclick?.(e);
			}}
			ondblclick={(e) => e.stopPropagation()}
		>
			<Icon icon={playing ? PauseIcon : PlayIcon} size={glyph} fill />
		</button>
	{/snippet}
</Tooltip>
