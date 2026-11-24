<script lang="ts">
	/**
	 * Lane magic-d3: the Enhance button on own playlists (#13). Off: an outline pill with a sparkle.
	 * On: the sparkle and label turn accent. A press mixes suggestions in, or takes them out.
	 */
	import { Icon, Tooltip } from '$lib/kit';
	import SparkleIcon from 'phosphor-svelte/lib/SparkleIcon';
	import { enhance, enhanced, toggleEnhance } from './enhance.svelte';

	let { playlistId, len }: { playlistId: string; len: number } = $props();

	const on = $derived(enhanced(playlistId));
	const busy = $derived(enhance.busy === playlistId);
</script>

<Tooltip text={on ? 'Take the suggestions out' : 'Mix suggested songs into this playlist'}>
	{#snippet trigger(props)}
		<button
			{...props}
			type="button"
			class="btn-outline !gap-1.5 {on ? '!border-accent !text-accent-text' : ''}"
			aria-pressed={on}
			aria-busy={busy}
			disabled={busy}
			onclick={() => toggleEnhance(playlistId, len)}
		>
			<Icon icon={SparkleIcon} size={16} fill class={busy ? 'animate-pulse' : ''} />
			{on ? 'Enhanced' : 'Enhance'}
		</button>
	{/snippet}
</Tooltip>
