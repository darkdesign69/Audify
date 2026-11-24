<script lang="ts">
	/** The 8px gap between panels is the resize handle (cursor col-resize). Drag reports the new
	 *  width from the drag start; `side` = which panel it sizes ('left' grows rightwards).
	 *  Keyboard (a11y): like Spotify's resizer (a range input, step 10) it is a focusable separator
	 *  with a value; ←/→ (↑/↓) nudge by 10px, Home/End jump to the min/max. */
	import { shell, LEFT_MIN, LEFT_MAX, LEFT_COLLAPSED, RIGHT_MIN, RIGHT_MAX } from './layout.svelte';
	let {
		side,
		width,
		onresize,
		label
	}: { side: 'left' | 'right'; width: number; onresize: (px: number) => void; label: string } = $props();

	const min = $derived(side === 'left' ? LEFT_COLLAPSED : RIGHT_MIN);
	const max = $derived(side === 'left' ? LEFT_MAX : RIGHT_MAX);
	function key(e: KeyboardEvent) {
		if (e.altKey || e.metaKey || e.ctrlKey) return;
		// The left handle grows rightwards; the right handle grows leftwards.
		const grow = side === 'left' ? 'ArrowRight' : 'ArrowLeft';
		const shrink = side === 'left' ? 'ArrowLeft' : 'ArrowRight';
		let next: number;
		if (e.key === grow || e.key === 'ArrowUp') next = width < LEFT_MIN && side === 'left' ? LEFT_MIN : width + 10;
		else if (e.key === shrink || e.key === 'ArrowDown') next = width - 10;
		else if (e.key === 'Home') next = min;
		else if (e.key === 'End') next = max;
		else return;
		e.preventDefault();
		e.stopPropagation();
		onresize(next);
	}

	let startX = 0;
	let startW = 0;
	function down(e: PointerEvent) {
		if (e.button !== 0) return;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		startX = e.clientX;
		startW = width;
		shell.resizing = true;
	}
	function move(e: PointerEvent) {
		if (!shell.resizing || !(e.currentTarget as HTMLElement).hasPointerCapture(e.pointerId)) return;
		const dx = e.clientX - startX;
		onresize(side === 'left' ? startW + dx : startW - dx);
	}
	function up() {
		shell.resizing = false;
	}
</script>

<!-- A focusable separator with a value is an interactive widget (WAI-ARIA window splitter). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	role="separator"
	aria-orientation="vertical"
	aria-label={label}
	aria-valuenow={Math.round(width)}
	aria-valuemin={min}
	aria-valuemax={max}
	aria-valuetext="{Math.round(width)} pixels"
	tabindex="0"
	class="group relative z-[5] w-2 shrink-0 cursor-col-resize rounded-full focus-visible:outline-offset-[-2px]"
	onkeydown={key}
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	onpointercancel={up}
>
	<span class="absolute inset-y-2 left-1/2 w-px -translate-x-1/2 bg-white/0 transition-colors group-hover:bg-white/30"></span>
</div>
