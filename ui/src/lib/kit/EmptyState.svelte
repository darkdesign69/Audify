<script lang="ts">
	/**
	 * Centred empty state (spec: states.md). Column max-width ~400, gap 16.
	 *
	 * Props
	 *   icon?: IconComponent    64px #b3b3b3
	 *   title: string           32/700 (size="sm" → 16/700, e.g. library search "Couldn't find …")
	 *   body?: string           16px #b3b3b3 (sm: 14px)
	 *   actionLabel?: string; onAction?: () => void   white pill (48px; sm: 32px)
	 *   size?: 'md' | 'sm'
	 *   children?: Snippet      extra content under the body (e.g. a search field)
	 *
	 * Usage: <EmptyState icon={HeartIcon} title="Songs you like will appear here"
	 *          body="Save songs by tapping the heart icon." actionLabel="Find songs" onAction={…} />
	 */
	import type { Snippet } from 'svelte';
	import Icon, { type IconComponent } from './Icon.svelte';

	let {
		icon,
		title,
		body,
		actionLabel,
		onAction,
		size = 'md',
		children,
		class: className = ''
	}: {
		icon?: IconComponent;
		title: string;
		body?: string | null;
		actionLabel?: string;
		onAction?: () => void;
		size?: 'md' | 'sm';
		children?: Snippet;
		class?: string;
	} = $props();
</script>

<div
	class="mx-auto flex max-w-[400px] flex-col items-center text-center {size === 'sm'
		? 'gap-2 py-6'
		: 'gap-4 py-16'} {className}"
>
	{#if icon}<Icon {icon} size={size === 'sm' ? 32 : 64} class="text-text-subdued" />{/if}
	<h2 class="font-bold text-white {size === 'sm' ? 'text-base' : 'text-2xl'}">{title}</h2>
	{#if body}
		<p class="text-text-subdued {size === 'sm' ? 'text-sm' : 'text-base'}">{body}</p>
	{/if}
	{#if children}{@render children()}{/if}
	{#if actionLabel && onAction}
		<button type="button" class="btn-white {size === 'sm' ? 'sm' : ''} mt-2" onclick={onAction}>
			{actionLabel}
		</button>
	{/if}
</div>
