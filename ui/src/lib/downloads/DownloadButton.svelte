<script lang="ts">
	/**
	 * Spotify's download toggle (playlist.md §2, album.md, liked-songs.md, podcasts.md): a circled
	 * down-arrow, #b3b3b3 → white, scale 1.04 on hover.
	 *  - none: outline arrow-circle, tooltip "Download".
	 *  - waiting / downloading: a progress ring (accent arc over the grey circle) around an accent
	 *    arrow; the arc spins while the size is still unknown. Click = remove (with confirm).
	 *  - done: solid accent circle, dark arrow. Click = "Remove from Downloads?" confirm.
	 *  - failed: nothing left in flight and some songs didn't download. A white retry arrow;
	 *    click = retry every failed song (`downloads_retry`), like iOS.
	 *
	 * Props
	 *   target: DownloadCollection   what this button downloads (id, kind, title, thumbnail)
	 *   songs?: SongItem[]           only for fixed items (an episode); lists are read by Rust
	 *   size?: 'lg' | 'md'           lg = 32px glyph in a 48px box (action bar); md = 16px in 32px
 *   iconSize?: number            glyph override (episode rows: 24 in the 32px box)
	 */
	import type { DownloadCollection, SongItem } from '$lib/api';
	// Direct imports, not the barrel: ActionBar (in the barrel) renders this component.
	import Icon from '$lib/kit/Icon.svelte';
	import Tooltip from '$lib/kit/Tooltip.svelte';
	import { confirmDialog } from '$lib/kit/confirm.svelte';
	import { toast } from '$lib/kit/toast';
	import ArrowCircleDownIcon from 'phosphor-svelte/lib/ArrowCircleDownIcon';
	import ArrowDownIcon from 'phosphor-svelte/lib/ArrowDownIcon';
	import ArrowClockwiseIcon from 'phosphor-svelte/lib/ArrowClockwiseIcon';
	import { collectionState, collections, retryDownloads, toggleDownload } from '$lib/downloads.svelte';

	let {
		target,
		songs,
		size = 'lg',
		iconSize
	}: { target: DownloadCollection; songs?: SongItem[]; size?: 'lg' | 'md'; iconSize?: number } = $props();

	const st = $derived(collectionState(target.id));
	const glyph = $derived(iconSize ?? (size === 'lg' ? 32 : 16));
	const failed = $derived(collections.get(target.id)?.failed ?? 0);
	const label = $derived(
		st.state === 'none'
			? 'Download'
			: st.state === 'failed'
				? `${failed === 1 ? "1 song didn't" : `${failed} songs didn't`} download. Retry`
				: 'Remove from Downloads'
	);
	// Ring geometry (viewBox 32): radius 13.5 with a 2px stroke, like the outline glyph's circle.
	const R = 13.5;
	const C = 2 * Math.PI * R;

	async function click(e: MouseEvent) {
		e.stopPropagation();
		try {
			if (st.state === 'failed') {
				await retryDownloads();
				toast('Trying the downloads again');
				return;
			}
			await toggleDownload(target, {
				songs,
				confirm: () =>
					confirmDialog({
						title: 'Remove from Downloads?',
						body: "You won't be able to play this offline.",
						confirmLabel: 'Remove'
					})
			});
		} catch (err) {
			toast.error(String(err));
		}
	}
</script>

<Tooltip text={label}>
	{#snippet trigger(props)}
		<button
			{...props}
			type="button"
			aria-label={label}
			aria-pressed={st.state !== 'none'}
			data-download-state={st.state}
			onclick={click}
			class="relative grid shrink-0 place-items-center rounded-full transition-[color,transform] duration-100 ease-productive hover:scale-[1.04] active:scale-100 active:opacity-70 {size ===
			'lg'
				? 'size-12'
				: 'size-8'} {st.state === 'none'
				? 'text-text-subdued hover:text-white'
				: st.state === 'failed'
					? 'text-white'
					: 'text-accent hover:text-accent-hover'}"
		>
			{#if st.state === 'none'}
				<Icon icon={ArrowCircleDownIcon} size={glyph} />
			{:else if st.state === 'failed'}
				<Icon icon={ArrowClockwiseIcon} size={glyph} />
			{:else if st.state === 'done'}
				<Icon icon={ArrowCircleDownIcon} size={glyph} fill />
			{:else}
				<span class="relative grid place-items-center" style="width:{glyph}px;height:{glyph}px">
					<svg
						viewBox="0 0 32 32"
						class="absolute inset-0 size-full -rotate-90 {st.state === 'waiting' && st.fraction === 0 ? 'ring-spin' : ''}"
						aria-hidden="true"
					>
						<circle cx="16" cy="16" r={R} fill="none" stroke="var(--color-text-subdued, #b3b3b3)" stroke-width="2" opacity="0.5" />
						<circle
							cx="16"
							cy="16"
							r={R}
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-dasharray={C}
							stroke-dashoffset={st.state === 'waiting' && st.fraction === 0 ? C * 0.75 : C * (1 - Math.max(0.03, st.fraction))}
							class="transition-[stroke-dashoffset] duration-300"
						/>
					</svg>
					<Icon icon={ArrowDownIcon} size={glyph / 2} weight="bold" />
				</span>
			{/if}
		</button>
	{/snippet}
</Tooltip>

<style>
	.ring-spin {
		animation: ring-spin 1s linear infinite;
	}
	@keyframes ring-spin {
		from {
			transform: rotate(-90deg);
		}
		to {
			transform: rotate(270deg);
		}
	}
</style>
