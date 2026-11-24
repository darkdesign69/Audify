<script lang="ts">
	/**
	 * The right panel's container (now-playing-panel.md / queue-panel.md): header row (16/700 title,
	 * close X) and a scroller. Switches between the views by `shell.rightView`; C5 fills the views in
	 * `shell/panels/`. Content swaps without animation, like Spotify.
	 */
	import { shell } from './layout.svelte';
	import NowPlayingView from './panels/NowPlayingView.svelte';
	import QueueView from './panels/QueueView.svelte';
	import DevicesView from './panels/DevicesView.svelte';
	import PanelHeader from '$lib/nowplaying/PanelHeader.svelte';
	import { playback } from '$lib/player.svelte';

	const title = $derived(
		shell.rightView === 'queue'
			? 'Queue'
			: shell.rightView === 'devices'
				? 'Connect to a device'
				: (playback.queue.sourceName ?? playback.now?.title ?? 'Now playing')
	);
</script>

<section class="group/rp flex h-full min-h-0 flex-col overflow-hidden rounded-lg bg-bg-base" aria-label={title}>
	<!-- p-nowplaying hookup: header (context name, …, sleep timer, close) lives with the views. -->
	<PanelHeader />
	<div class="scroll-area min-h-0 flex-1 overflow-y-auto" data-right-scroll>
		{#if shell.rightView === 'queue'}
			<QueueView />
		{:else if shell.rightView === 'devices'}
			<DevicesView />
		{:else}
			<NowPlayingView />
		{/if}
	</div>
</section>
