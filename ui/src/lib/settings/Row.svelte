<script lang="ts">
	/**
	 * One settings row (settings.md): label 14px #b3b3b3 on the left (wraps, max ≈ 480px), the
	 * control right-aligned, min-height 40px. `below` renders full width under the row (folder
	 * lists, the equalizer graph). `sub` indents a dependent row a little (crossfade length).
	 */
	import type { Snippet } from 'svelte';

	let {
		label,
		hint,
		control,
		below,
		children,
		id,
		hidden = false
	}: {
		label: string;
		/** A second, smaller line under the label. */
		hint?: string;
		control?: Snippet;
		below?: Snippet;
		children?: Snippet;
		id?: string;
		hidden?: boolean;
	} = $props();
</script>

{#if !hidden}
	<!-- Spotify (measured): a one-line row is 40px (a 32px control + 4px each side), a two-line
	     label 48px; rows sit back to back. -->
	<div {id} data-settings-row class="py-1">
		<div class="flex min-h-8 items-center justify-between gap-6">
			<div class="max-w-[480px] min-w-0 text-sm text-text-subdued">
				<div>{label}</div>
				{#if hint}<div class="mt-0.5 text-xs">{hint}</div>{/if}
				{#if children}{@render children()}{/if}
			</div>
			{#if control}<div class="flex shrink-0 items-center gap-3">{@render control()}</div>{/if}
		</div>
		{#if below}<div class="mt-2">{@render below()}</div>{/if}
	</div>
{/if}
