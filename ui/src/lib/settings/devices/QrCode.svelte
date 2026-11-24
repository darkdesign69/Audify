<script lang="ts">
	/**
	 * The linking QR (a `spotless://pair` link): dark modules on a white rounded card, one SVG path,
	 * drawn from `uqr`'s matrix so it stays crisp at any zoom.
	 */
	import { encode } from 'uqr';

	let { value, size = 168 }: { value: string; size?: number } = $props();

	const qr = $derived(encode(value, { ecc: 'M', border: 2 }));
	const path = $derived.by(() => {
		let d = '';
		qr.data.forEach((row, y) =>
			row.forEach((on, x) => {
				if (on) d += `M${x} ${y}h1v1h-1z`;
			})
		);
		return d;
	});
</script>

<div class="rounded-lg bg-white p-1.5" style:width="{size}px" style:height="{size}px">
	<svg viewBox="0 0 {qr.size} {qr.size}" class="block size-full" shape-rendering="crispEdges" role="img" aria-label="QR code">
		<path d={path} fill="#000" />
	</svg>
</div>
