<script lang="ts">
	/**
	 * stream: magic-d4 — Settings › Your Library › Recover playlists (MAGIC-GAPS #22). Deleted
	 * playlists stay recoverable for 30 days (Spotify's account page keeps them 90), not just until
	 * the undo toast is gone. Lists them with their song count and days left; Restore puts one back
	 * in Your Library (moved to the top of Recents).
	 */
	import type { DeletedPlaylist } from '$lib/api';
	import { lib } from '$lib/library.svelte';
	import Pill from '$lib/kit/Pill.svelte';
	import { toast } from '$lib/kit/toast';
	import Row from './Row.svelte';

	let open = $state(false);
	let list = $state<DeletedPlaylist[] | null>(null);
	let busy = $state<string | null>(null);

	async function load() {
		try {
			list = await lib.deletedPlaylists();
		} catch {
			list = [];
		}
	}
	function toggle() {
		open = !open;
		if (open) void load();
	}
	async function restore(p: DeletedPlaylist) {
		busy = p.id;
		try {
			await lib.restorePlaylist(p.id);
			list = (list ?? []).filter((x) => x.id !== p.id);
			toast.success(`Restored ${p.name}`);
		} catch {
			toast.error("Couldn't restore this playlist. Try again.");
		} finally {
			busy = null;
		}
	}
	const daysLeft = (p: DeletedPlaylist) =>
		Math.max(0, Math.ceil((p.purgeAt - Date.now() / 1000) / 86_400));
</script>

<Row label="Recover playlists" hint="Playlists you delete can be restored for 30 days.">
	{#snippet control()}
		<Pill variant="outline" onclick={toggle}>{open ? 'Hide' : 'Show deleted'}</Pill>
	{/snippet}
	{#snippet below()}
		{#if open}
			<div class="flex flex-col" data-testid="recover-playlists">
				{#if list === null}
					<p class="px-2 text-sm text-text-subdued">Loading…</p>
				{:else}
					{#each list as p (p.id)}
						<div class="flex h-14 items-center gap-3 rounded-sm px-2 hover:bg-bg-tinted">
							{#if p.thumbnail}
								<img src={p.thumbnail} alt="" class="size-10 shrink-0 rounded-sm object-cover" />
							{:else}
								<div class="size-10 shrink-0 rounded-sm bg-bg-tinted"></div>
							{/if}
							<div class="min-w-0 flex-1">
								<div class="truncate text-sm text-white">{p.name}</div>
								<div class="truncate text-xs text-text-subdued">
									{p.count} song{p.count === 1 ? '' : 's'} • {daysLeft(p)} day{daysLeft(p) === 1 ? '' : 's'} left
								</div>
							</div>
							<Pill variant="outline" disabled={busy === p.id} onclick={() => restore(p)}>Restore</Pill>
						</div>
					{:else}
						<p class="px-2 text-sm text-text-subdued">No deleted playlists.</p>
					{/each}
				{/if}
			</div>
		{/if}
	{/snippet}
</Row>
