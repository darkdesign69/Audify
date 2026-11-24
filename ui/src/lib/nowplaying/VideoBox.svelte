<script lang="ts">
	/**
	 * Borrows the one live music-video element (VideoSurface owns it, `$lib/video.svelte`) into this
	 * box while mounted, and parks it again on unmount. Claim/park are synchronous on purpose (a
	 * media element out of the document across a microtask gets paused).
	 * `fit`: 'cover' fills the box (Now Playing art slot), 'contain' letterboxes (video mode).
	 * `rank`: which box wins when several are mounted (`claimVideo`); the player bar's is 0.
	 * `holding` is bound out: whether this box has the picture right now.
	 */
	import { claimVideo, parkVideo, video } from '$lib/video.svelte';

	let {
		fit = 'contain',
		class: klass = '',
		rank = 1,
		holding = $bindable(false)
	}: { fit?: 'cover' | 'contain'; class?: string; rank?: number; holding?: boolean } = $props();

	let el = $state<HTMLElement | null>(null);
	$effect(() => {
		holding = !!el && video.holder === el;
	});

	function hold(box: HTMLElement) {
		el = box;
		claimVideo(box, rank);
		return () => {
			el = null;
			parkVideo(box);
		};
	}
</script>

<div
	class="video-box overflow-hidden bg-black {klass}"
	data-fit={fit}
	{@attach hold}
></div>

<style>
	.video-box :global(video) {
		display: block;
		width: 100% !important;
		height: 100% !important;
		border-radius: 0 !important;
		object-fit: contain;
	}
	.video-box[data-fit='cover'] :global(video) {
		object-fit: cover !important;
	}
</style>
