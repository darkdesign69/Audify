<script lang="ts">
	/**
	 * Find in playlist/album (Spotify's action-bar magnifier → "Search in playlist" field).
	 * ⌘F / menu://find (`$lib/shell/find`) opens and focuses it. Collapses on blur when empty;
	 * Esc clears, then closes.
	 */
	import { untrack } from 'svelte';
	import { Icon, IconButton } from '$lib/kit';
	import { find } from '$lib/shell/find.svelte';
	import MagnifyingGlassIcon from 'phosphor-svelte/lib/MagnifyingGlassIcon';
	import XIcon from 'phosphor-svelte/lib/XIcon';

	let {
		value = $bindable(''),
		placeholder = 'Search in playlist',
		button = true
	}: {
		value?: string;
		placeholder?: string;
		/** Show the magnifier at rest. Albums don't (Spotify); ⌘F still opens the field there. */
		button?: boolean;
	} = $props();

	let open = $state(false);
	let input = $state<HTMLInputElement | null>(null);

	const startSeq = untrack(() => find.seq);
	$effect(() => {
		if (find.seq !== startSeq) show();
	});

	async function show() {
		open = true;
		await Promise.resolve();
		input?.focus();
		input?.select();
	}
</script>

{#if open || value}
	<label
		class="flex h-8 w-[200px] items-center gap-2 rounded-sm bg-bg-tinted px-2 text-text-subdued focus-within:bg-bg-elevated-highlight"
	>
		<Icon icon={MagnifyingGlassIcon} size={16} />
		<input
			bind:this={input}
			bind:value
			type="text"
			{placeholder}
			aria-label={placeholder}
			class="min-w-0 flex-1 bg-transparent text-sm text-white outline-none placeholder:text-text-subdued"
			onblur={() => {
				if (!value) open = false;
			}}
			onkeydown={(e) => {
				if (e.key !== 'Escape') return;
				e.stopPropagation();
				if (value) value = '';
				else {
					open = false;
					input?.blur();
				}
			}}
		/>
		{#if value}
			<button
				type="button"
				aria-label="Clear search"
				class="text-text-subdued hover:text-white"
				onmousedown={(e) => e.preventDefault()}
				onclick={() => {
					value = '';
					input?.focus();
				}}
			>
				<Icon icon={XIcon} size={16} />
			</button>
		{/if}
	</label>
{:else if button}
	<IconButton icon={MagnifyingGlassIcon} label={placeholder} onclick={show} />
{/if}
