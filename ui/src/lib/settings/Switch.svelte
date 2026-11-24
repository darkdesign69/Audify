<script lang="ts">
	/**
	 * Spotify's settings switch (settings.md): 42x24 track, radius 24; off #535353 (hover #727272),
	 * on the accent (hover accent-hover); 20px white knob, 2px inset, slides 18px in ~100ms.
	 *
	 * Props: checked, label (aria), disabled, onchange(next)
	 */
	let {
		checked,
		label,
		disabled = false,
		onchange
	}: { checked: boolean; label: string; disabled?: boolean; onchange?: (next: boolean) => void } = $props();
</script>

<button
	type="button"
	role="switch"
	aria-checked={checked}
	aria-label={label}
	{disabled}
	class="group relative inline-block h-6 w-[42px] shrink-0 rounded-3xl transition-colors duration-100 disabled:pointer-events-none disabled:opacity-40 {checked
		? 'bg-switch-on hover:bg-accent-hover'
		: 'bg-switch-off hover:bg-[#727272]'}"
	onclick={() => onchange?.(!checked)}
>
	<span
		class="absolute top-0.5 left-0.5 size-5 rounded-full bg-white shadow-[0_2px_4px_rgba(0,0,0,.2)] transition-transform duration-100 ease-productive {checked
			? 'translate-x-[18px]'
			: ''}"
	></span>
</button>
