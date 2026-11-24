<script module lang="ts">
	// Spotless's mark: a dark-pink disc with a black beamed pair of eighth notes, constructed in
	// scripts/brand/final.py. The master (assets/brand/spotless.svg) puts it on a black tile for the
	// app icons; in the app it is drawn without the tile. These lines are rewritten by
	// scripts/brand/gen-brand.py ship.
	export const MARK_D = 'M43.70 62.08A9.520 6.944 -22.0 0 1 37.48 72.09A9.520 6.944 -22.0 0 1 26.05 69.22A9.520 6.944 -22.0 0 1 32.27 59.21A9.520 6.944 -22.0 0 1 43.70 62.08ZM66.66 55.92A9.520 6.944 -22.0 0 1 60.44 65.93A9.520 6.944 -22.0 0 1 49.01 63.06A9.520 6.944 -22.0 0 1 55.23 53.05A9.520 6.944 -22.0 0 1 66.66 55.92ZM39.71 32.61L44.08 32.61L44.08 64.05L39.71 64.72ZM62.67 26.45L67.04 26.45L67.04 57.89L62.67 58.56ZM39.71 28.13L67.04 20.80L67.04 29.76L39.71 37.09Z'; // gen-brand
	export const MARK_DISC = '#FF2D7A'; // gen-brand
	export const MARK_INK = '#000000'; // gen-brand
	export const MARK_R = 40; // gen-brand
</script>

<script lang="ts">
	/** The mark at `size` px (the disc's diameter). */
	let { size = 32, class: className = '' }: { size?: number; class?: string } = $props();
	const box = `${50 - MARK_R} ${50 - MARK_R} ${MARK_R * 2} ${MARK_R * 2}`;
</script>

<svg width={size} height={size} viewBox={box} aria-hidden="true" class="shrink-0 {className}">
	<circle cx="50" cy="50" r={MARK_R} fill={MARK_DISC} />
	<path fill={MARK_INK} d={MARK_D} />
</svg>
