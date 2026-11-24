<!--
	Spotless audio stream: the equalizer graph. A frequency-response curve (log axis, 20 Hz-20 kHz)
	computed with the same RBJ formulas the player runs (lib/eq-curve.ts, DSSSP-style maths), with
	one draggable point per band. Points move vertically only, like Spotify's.

	Controlled: pass `gains`, update them from `ondrag` (fires while dragging, at pointer rate;
	lib/audio.svelte.ts `dragEq` throttles the IPC to one call per frame) and persist on
	`oncommit` (pointer release, keyboard change). Keyboard: Arrow Up/Down 0.5 dB (Shift: 3 dB),
	Page Up/Down 3 dB, Home/End to the limits.

	Neutral styling through CSS variables; the UI stream skins it:
	--eq-curve, --eq-fill, --eq-grid, --eq-zero, --eq-handle, --eq-handle-ring, --eq-label,
	--eq-focus, --eq-font-size, --eq-disabled-opacity.
-->
<script lang="ts">
	import { EQ_BANDS, EQ_MAX_DB, clampGain, freqLabel, freqToX, logFrequencies, responseDb, type EqBand } from '$lib/eq-curve';

	interface Props {
		gains: number[];
		bands?: EqBand[];
		maxDb?: number;
		disabled?: boolean;
		/** Total height in px; the width follows the container. */
		height?: number;
		showLabels?: boolean;
		ondrag?: (gains: number[]) => void;
		oncommit?: (gains: number[]) => void;
		class?: string;
	}

	let {
		gains,
		bands = EQ_BANDS,
		maxDb = EQ_MAX_DB,
		disabled = false,
		height = 220,
		showLabels = true,
		ondrag,
		oncommit,
		class: className = ''
	}: Props = $props();

	const uid = $props.id();
	let width = $state(0);
	let dragging = $state<number | null>(null);
	/** What is drawn: the props, except mid-drag, when it is ahead of them. */
	let live = $state<number[]>([]);
	$effect(() => {
		if (dragging === null) live = bands.map((_, i) => gains[i] ?? 0);
	});

	const PAD_X = 18;
	const PAD_TOP = 12;
	const LABEL_H = $derived(showLabels ? 22 : 0);
	const plotH = $derived(Math.max(40, height - PAD_TOP - LABEL_H - 8));
	/** The curve can sum past one band's limit; leave it some room above and below. */
	const range = $derived(maxDb * 1.25);

	const x = (f: number) => PAD_X + freqToX(f) * Math.max(0, width - 2 * PAD_X);
	const y = (db: number) => PAD_TOP + ((range - Math.max(-range, Math.min(range, db))) / (2 * range)) * plotH;
	const dbAt = (py: number) => range - ((py - PAD_TOP) / plotH) * 2 * range;

	const freqs = logFrequencies(200);
	const curve = $derived.by(() => {
		if (!width) return '';
		return freqs.map((f, i) => `${i ? 'L' : 'M'}${x(f).toFixed(1)},${y(responseDb(live, f, bands)).toFixed(1)}`).join('');
	});
	const fill = $derived(curve ? `${curve}L${x(freqs[freqs.length - 1]).toFixed(1)},${y(0)}L${x(freqs[0]).toFixed(1)},${y(0)}Z` : '');

	function set(i: number, db: number) {
		const next = [...live];
		next[i] = clampGain(db);
		if (next[i] === live[i]) return;
		live = next;
		ondrag?.(next);
	}

	let svg = $state<SVGSVGElement>();
	function pointerY(e: PointerEvent) {
		const r = svg!.getBoundingClientRect();
		return ((e.clientY - r.top) / r.height) * height;
	}

	function down(e: PointerEvent, i: number) {
		if (disabled || e.button !== 0) return;
		e.preventDefault();
		(e.currentTarget as Element).setPointerCapture(e.pointerId);
		(e.currentTarget as SVGElement).focus();
		dragging = i;
		set(i, Math.round(dbAt(pointerY(e)) * 2) / 2);
	}
	function move(e: PointerEvent, i: number) {
		if (dragging !== i) return;
		set(i, Math.round(dbAt(pointerY(e)) * 2) / 2);
	}
	function up(e: PointerEvent, i: number) {
		if (dragging !== i) return;
		(e.currentTarget as Element).releasePointerCapture(e.pointerId);
		dragging = null;
		oncommit?.([...live]);
	}

	function key(e: KeyboardEvent, i: number) {
		if (disabled) return;
		const step = e.shiftKey ? 3 : 0.5;
		const g = live[i];
		const to: Record<string, number> = {
			ArrowUp: g + step,
			ArrowRight: g + step,
			ArrowDown: g - step,
			ArrowLeft: g - step,
			PageUp: g + 3,
			PageDown: g - 3,
			Home: maxDb,
			End: -maxDb
		};
		if (!(e.key in to)) return;
		e.preventDefault();
		set(i, to[e.key]);
		oncommit?.([...live]);
	}
</script>

<div
	class="eq-graph {className}"
	class:disabled
	style:height="{height}px"
	bind:clientWidth={width}
>
	{#if width}
		<svg bind:this={svg} {width} {height} viewBox="0 0 {width} {height}" role="group" aria-label="Equalizer">
			<defs>
				<linearGradient id="eq-fill-{uid}" x1="0" y1="0" x2="0" y2="1">
					<stop offset="0%" stop-color="var(--eq-fill, currentColor)" stop-opacity="0.45" />
					<stop offset="50%" stop-color="var(--eq-fill, currentColor)" stop-opacity="0.12" />
					<stop offset="100%" stop-color="var(--eq-fill, currentColor)" stop-opacity="0.45" />
				</linearGradient>
			</defs>

			<!-- grid: the limits, 0 dB, one line per band -->
			{#each [maxDb, -maxDb] as db (db)}
				<line class="grid" x1={PAD_X} x2={width - PAD_X} y1={y(db)} y2={y(db)} />
			{/each}
			{#each bands as b (b.freq)}
				<line class="grid" x1={x(b.freq)} x2={x(b.freq)} y1={y(range)} y2={y(-range)} />
			{/each}
			<line class="zero" x1={PAD_X} x2={width - PAD_X} y1={y(0)} y2={y(0)} />

			<path class="fill" d={fill} fill="url(#eq-fill-{uid})" />
			<path class="curve" d={curve} />

			{#each bands as b, i (b.freq)}
				{@const cx = x(b.freq)}
				{@const cy = y(live[i] ?? 0)}
				<g
					class="handle"
					class:active={dragging === i}
					role="slider"
					tabindex={disabled ? -1 : 0}
					aria-label="{freqLabel(b.freq)} band"
					aria-orientation="vertical"
					aria-valuemin={-maxDb}
					aria-valuemax={maxDb}
					aria-valuenow={live[i] ?? 0}
					aria-valuetext="{(live[i] ?? 0) > 0 ? '+' : ''}{live[i] ?? 0} dB"
					aria-disabled={disabled}
					onpointerdown={(e) => down(e, i)}
					onpointermove={(e) => move(e, i)}
					onpointerup={(e) => up(e, i)}
					onpointercancel={(e) => up(e, i)}
					onkeydown={(e) => key(e, i)}
				>
					<!-- a generous invisible hit area, the full plot height of this band -->
					<rect class="hit" x={cx - 16} y={PAD_TOP} width="32" height={plotH} />
					<circle class="ring" {cx} {cy} r="11" />
					<circle class="dot" {cx} {cy} r="6.5" />
				</g>
				{#if showLabels}
					<text class="label" x={cx} y={height - 6} text-anchor="middle">{freqLabel(b.freq)}</text>
				{/if}
			{/each}
		</svg>
	{/if}
</div>

<style>
	.eq-graph {
		position: relative;
		width: 100%;
		user-select: none;
		touch-action: none;
		color: var(--eq-curve, currentColor);
	}
	.eq-graph.disabled {
		opacity: var(--eq-disabled-opacity, 0.4);
	}
	.eq-graph.disabled .handle {
		pointer-events: none;
	}
	svg {
		display: block;
		overflow: visible;
	}
	.grid {
		stroke: var(--eq-grid, color-mix(in srgb, currentColor 18%, transparent));
		stroke-width: 1;
	}
	.zero {
		stroke: var(--eq-zero, color-mix(in srgb, currentColor 35%, transparent));
		stroke-width: 1;
	}
	.curve {
		fill: none;
		stroke: var(--eq-curve, currentColor);
		stroke-width: 2.5;
		stroke-linejoin: round;
		stroke-linecap: round;
	}
	.handle {
		cursor: ns-resize;
		outline: none;
	}
	.hit {
		fill: transparent;
	}
	.ring {
		fill: transparent;
		stroke: transparent;
		stroke-width: 2;
		transition: stroke 120ms;
	}
	.handle:focus-visible .ring {
		stroke: var(--eq-focus, var(--eq-curve, currentColor));
	}
	.handle.active .ring,
	.handle:hover .ring {
		fill: var(--eq-handle-ring, color-mix(in srgb, currentColor 20%, transparent));
	}
	.dot {
		fill: var(--eq-handle, #fff);
		stroke: var(--eq-curve, currentColor);
		stroke-width: 2;
	}
	.label {
		fill: var(--eq-label, color-mix(in srgb, currentColor 70%, transparent));
		font-size: var(--eq-font-size, 11px);
		font-family: inherit;
	}
</style>
