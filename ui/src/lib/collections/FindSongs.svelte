<script lang="ts">
	/**
	 * "Let's find something for your playlist" (playlist.md §5, states.md): a 40px search box
	 * (white/10, radius 4) whose results are rows with an outline "Add". Used under an empty own
	 * playlist (always open) and behind "Find more" (closable ✕).
	 */
	import type { SongItem } from '$lib/api';
	import * as api from '$lib/api';
	import { Icon, IconButton, Skeleton } from '$lib/kit';
	import MagnifyingGlassIcon from 'phosphor-svelte/lib/MagnifyingGlassIcon';
	import XIcon from 'phosphor-svelte/lib/XIcon';
	import SuggestRow from './SuggestRow.svelte';

	let {
		onAdd,
		onClose,
		autofocus = false
	}: {
		onAdd: (song: SongItem) => Promise<boolean>;
		onClose?: () => void;
		autofocus?: boolean;
	} = $props();

	let query = $state('');
	let results = $state.raw<SongItem[]>([]);
	let loading = $state(false);
	let failed = $state(false);
	let adding = $state<string | null>(null);
	let input = $state<HTMLInputElement | null>(null);

	$effect(() => {
		if (autofocus) input?.focus();
	});

	$effect(() => {
		const q = query.trim();
		if (!q) {
			results = [];
			loading = false;
			return;
		}
		loading = true;
		const t = setTimeout(async () => {
			failed = false;
			try {
				const r = await api.search(q);
				if (q === query.trim()) results = r.slice(0, 10);
			} catch {
				if (q === query.trim()) failed = true;
			} finally {
				if (q === query.trim()) loading = false;
			}
		}, 300);
		return () => clearTimeout(t);
	});

	async function add(song: SongItem) {
		adding = song.video_id;
		try {
			if (await onAdd(song)) results = results.filter((s) => s.video_id !== song.video_id);
		} finally {
			adding = null;
		}
	}
</script>

<section class="mt-6 border-t border-border-subtle pt-6">
	<div class="flex items-start justify-between gap-4">
		<div class="min-w-0 flex-1">
			<h2 class="text-xl font-bold text-white">Let's find something for your playlist</h2>
			<label
				class="mt-4 flex h-10 w-full max-w-[364px] items-center gap-2 rounded-sm bg-bg-tinted px-3 text-text-subdued focus-within:bg-bg-elevated-highlight"
			>
				<Icon icon={MagnifyingGlassIcon} size={16} />
				<input
					bind:this={input}
					bind:value={query}
					type="text"
					placeholder="Search for songs or episodes"
					class="min-w-0 flex-1 bg-transparent text-sm text-white outline-none placeholder:text-text-subdued"
					onkeydown={(e) => e.key === 'Escape' && (query ? (query = '') : onClose?.())}
				/>
				{#if query}
					<button type="button" aria-label="Clear search" class="text-text-subdued hover:text-white" onclick={() => (query = '')}>
						<Icon icon={XIcon} size={16} />
					</button>
				{/if}
			</label>
		</div>
		{#if onClose}
			<IconButton icon={XIcon} label="Close" iconSize={24} onclick={onClose} />
		{/if}
	</div>
	<div class="mt-4">
		{#if loading && !results.length}
			<Skeleton variant="row" count={4} />
		{:else if failed}
			<p class="py-4 text-sm text-text-subdued">Something went wrong. Try again.</p>
		{:else if query.trim() && !results.length && !loading}
			<p class="py-4 text-base font-bold text-white">No results found for "{query.trim()}"</p>
		{:else}
			<div role="grid" aria-label="Search results">
				{#each results as s (s.video_id)}
					<SuggestRow song={s} onAdd={add} adding={adding === s.video_id} />
				{/each}
			</div>
		{/if}
	</div>
</section>
