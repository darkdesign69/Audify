<script lang="ts">
	import { releaseSpeed, type DragSample } from '$lib/scrub';
	/**
	 * Spotify's progress / volume bar (player-bar.md): 12px hit area, 4px track white/30, white fill.
	 * Hover: fill turns the accent and a 12px white thumb appears at the value (no preview fill); with
	 * `tooltip` a small time label (e.g. "2:12") floats above the cursor. Drag scrubs:
	 * `oninput(v)` fires live, `onchange(v, drag)` on release (`drag`: the bar's width and the
	 * pointer's speed as it let go, px / s; none for keys). ←/→ step by `step` when focused.
	 * Values are 0..max.
	 */
	let {
		value,
		max = 1,
		step,
		tooltip,
		label,
		valuetext,
		disabled = false,
		class: className = '',
		oninput,
		onchange,
		dragging = $bindable(false),
		marks = []
	}: {
		value: number;
		max?: number;
		step?: number;
		/** Label for the value under the cursor (progress bar: the time there). */
		tooltip?: (v: number) => string;
		label: string;
		/** a11y: spoken value (aria-valuetext). Default with `tooltip`: "1:02 of 3:45" (Spotify
		 *  speaks the progress bar as "0:00/2:31"); pass e.g. (v) => `${v}%` for volume. */
		valuetext?: (v: number) => string;
		disabled?: boolean;
		class?: string;
		oninput?: (v: number) => void;
		onchange?: (v: number, drag?: { width: number; speed: number }) => void;
		dragging?: boolean;
		/** Values to mark on the track (podcast chapter starts): thin gaps that split the bar. */
		marks?: number[];
	} = $props();

	let el: HTMLDivElement;
	let width = $state(0);
	let hoverX = $state<number | null>(null);
	let dragValue = $state<number | null>(null);
	/** Recent pointer positions of a drag, for its speed at release. */
	let samples: DragSample[] = [];
	/** Just let go: the fill and thumb ease (150 ms) to where `onchange` put the value, so a
	 *  release that settles onto a chapter start glides the last few pixels. */
	let easing = $state(false);
	let easeTimer: ReturnType<typeof setTimeout> | undefined;
	const sample = (e: PointerEvent) => {
		samples.push({ x: e.clientX, t: e.timeStamp });
		if (samples.length > 12) samples.shift();
	};
	const shown = $derived(dragValue ?? value);
	const pct = $derived(max > 0 ? Math.min(100, Math.max(0, (shown / max) * 100)) : 0);

	function valueAt(clientX: number) {
		const r = el.getBoundingClientRect();
		const f = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
		return f * max;
	}

	function down(e: PointerEvent) {
		if (disabled || e.button !== 0) return;
		el.setPointerCapture(e.pointerId);
		dragging = true;
		samples = [];
		sample(e);
		dragValue = valueAt(e.clientX);
		oninput?.(dragValue);
	}
	function move(e: PointerEvent) {
		const r = el.getBoundingClientRect();
		hoverX = Math.min(r.width, Math.max(0, e.clientX - r.left));
		if (dragValue === null) return;
		sample(e);
		dragValue = valueAt(e.clientX);
		oninput?.(dragValue);
	}
	function up(e: PointerEvent) {
		if (dragValue === null) return;
		const v = valueAt(e.clientX);
		const speed = releaseSpeed(samples, e.timeStamp);
		samples = [];
		dragValue = null;
		dragging = false;
		easing = true;
		clearTimeout(easeTimer);
		easeTimer = setTimeout(() => (easing = false), 180);
		onchange?.(v, { width: el.getBoundingClientRect().width, speed });
	}
	function key(e: KeyboardEvent) {
		const s = step ?? max / 20;
		let v: number | null = null;
		if (e.key === 'ArrowRight' || e.key === 'ArrowUp') v = Math.min(max, value + s);
		else if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') v = Math.max(0, value - s);
		else if (e.key === 'Home') v = 0;
		else if (e.key === 'End') v = max;
		if (v === null) return;
		e.preventDefault();
		e.stopPropagation();
		onchange?.(v);
	}
</script>

<div
	bind:this={el}
	bind:clientWidth={width}
	role="slider"
	tabindex={disabled ? -1 : 0}
	aria-label={label}
	aria-valuemin={0}
	aria-valuemax={max}
	aria-valuenow={Math.round(shown)}
	aria-valuetext={valuetext ? valuetext(shown) : tooltip ? `${tooltip(shown)} of ${tooltip(max)}` : undefined}
	aria-orientation="horizontal"
	aria-disabled={disabled}
	class="group/slider relative flex h-3 w-full touch-none items-center {disabled
		? 'pointer-events-none opacity-50'
		: 'cursor-default'} {className}"
	data-dragging={dragValue !== null || undefined}
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	onpointercancel={up}
	onpointerleave={() => (hoverX = null)}
	onkeydown={key}
>
	<div class="relative h-1 w-full overflow-hidden rounded-[2px] bg-white/30">
		<div
			class="absolute inset-y-0 left-0 rounded-[2px] bg-white group-hover/slider:bg-accent group-focus-visible/slider:bg-accent group-data-dragging/slider:bg-accent"
			class:transition-[width]={easing}
			class:duration-150={easing}
			class:ease-out={easing}
			style="width:{pct}%"
		></div>
		{#each marks as m, i (i)}
			{#if m > 0 && m < max}
				<div class="absolute inset-y-0 w-0.5 -translate-x-1/2 bg-black" style="left:{(m / max) * 100}%" data-mark></div>
			{/if}
		{/each}
	</div>
	<div
		class="pointer-events-none absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white opacity-0 shadow-thumb group-hover/slider:opacity-100 group-focus-visible/slider:opacity-100 group-data-dragging/slider:opacity-100"
		class:transition-[left]={easing}
		class:duration-150={easing}
		class:ease-out={easing}
		style="left:{pct}%"
	></div>
	{#if tooltip && hoverX !== null && width}
		<div
			class="pointer-events-none absolute bottom-full mb-2 -translate-x-1/2 rounded-sm bg-bg-elevated-highlight px-2 py-1 text-xs font-bold whitespace-nowrap text-white tabular-nums shadow-menu"
			style="left:{hoverX}px"
			aria-hidden="true"
		>
			{tooltip((hoverX / width) * max)}
		</div>
	{/if}
</div>
