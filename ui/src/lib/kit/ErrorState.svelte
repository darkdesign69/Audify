<script lang="ts">
	/**
	 * Page-level error (spec: states.md "Error / offline"): "Something went wrong" 32/700, a body
	 * line, and a white pill to retry.
	 *
	 * Props
	 *   title?: string        default "Something went wrong"
	 *   message?: string      default "Try reloading the page." (pass the error text if useful)
	 *   retryLabel?: string   default "Reload"
	 *   onRetry?: () => void  omit to hide the button
	 *   offline?: boolean     wording for no connection ("No internet connection available")
	 */
	import Icon from './Icon.svelte';
	import WarningCircleIcon from 'phosphor-svelte/lib/WarningCircleIcon';
	import WifiSlashIcon from 'phosphor-svelte/lib/WifiSlashIcon';

	let {
		title,
		message,
		retryLabel = 'Reload',
		onRetry,
		offline = false,
		class: className = ''
	}: {
		title?: string;
		message?: string | null;
		retryLabel?: string;
		onRetry?: () => void;
		offline?: boolean;
		class?: string;
	} = $props();
</script>

<div
	class="mx-auto flex max-w-[480px] flex-col items-center gap-4 py-16 text-center {className}"
	role="alert"
>
	<Icon icon={offline ? WifiSlashIcon : WarningCircleIcon} size={56} class="text-text-subdued" />
	<h2 class="text-2xl font-bold text-white">
		{title ?? (offline ? 'No internet connection available' : 'Something went wrong')}
	</h2>
	<p class="text-base text-text-subdued">
		{message ?? (offline ? 'Check your connection and try again.' : 'Try reloading the page.')}
	</p>
	{#if onRetry}
		<button type="button" class="btn-white mt-2" onclick={onRetry}>{retryLabel}</button>
	{/if}
</div>
