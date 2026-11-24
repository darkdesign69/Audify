<script lang="ts">
	/**
	 * Spotify tooltip: #282828, 14px white, 4px 8px padding, radius 4, menu shadow, no arrow.
	 * ~500ms open delay, instant between neighbours (the <Tooltip.Provider> in the root layout sets
	 * the delays). Above the trigger by default; top-bar buttons pass side="bottom".
	 *
	 * Wrap any element: the `trigger` snippet receives `props` to spread on your own button so no
	 * extra DOM is added:
	 *   <Tooltip text="Shuffle">{#snippet trigger(props)}<button {...props}>…</button>{/snippet}</Tooltip>
	 */
	import { Tooltip } from 'bits-ui';
	import type { Snippet } from 'svelte';

	let {
		text,
		side = 'top',
		sideOffset = 8,
		disabled = false,
		trigger
	}: {
		text: string | undefined | null;
		side?: 'top' | 'bottom' | 'left' | 'right';
		sideOffset?: number;
		disabled?: boolean;
		trigger: Snippet<[Record<string, unknown>]>;
	} = $props();
</script>

{#if !text || disabled}
	{@render trigger({})}
{:else}
	<Tooltip.Root disableHoverableContent>
		<Tooltip.Trigger>
			{#snippet child({ props })}
				{@render trigger(props)}
			{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content
				{side}
				{sideOffset}
				collisionPadding={8}
				class="z-[1060] max-w-[300px] rounded-sm bg-bg-menu px-2 py-1 text-sm text-white shadow-menu data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 duration-100"
			>
				{text}
			</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
{/if}
