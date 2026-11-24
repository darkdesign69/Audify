<script lang="ts" generics="T extends string">
	/**
	 * A row of filter chips with Spotify's "X" clear chip that appears first once something is
	 * selected. Single-select; clicking the selected chip clears it. Horizontal scroll without a bar.
	 * `options` can carry sub-filters: pass the visible list for the current level.
	 */
	import XIcon from 'phosphor-svelte/lib/XIcon';
	import Icon from './Icon.svelte';

	let {
		options,
		value = $bindable(null),
		clearLabel = 'Clear filters',
		class: className = ''
	}: {
		options: { id: T; label: string }[];
		value?: T | null;
		clearLabel?: string;
		class?: string;
	} = $props();
</script>

<div class="no-scrollbar flex items-center gap-2 overflow-x-auto {className}" role="group">
	{#if value !== null}
		<button
			type="button"
			class="chip size-8 justify-center !p-0"
			aria-label={clearLabel}
			onclick={() => (value = null)}
		>
			<Icon icon={XIcon} size={16} />
		</button>
	{/if}
	{#each options as o (o.id)}
		<button
			type="button"
			class="chip"
			aria-pressed={value === o.id}
			onclick={() => (value = value === o.id ? null : o.id)}
		>
			{o.label}
		</button>
	{/each}
</div>
