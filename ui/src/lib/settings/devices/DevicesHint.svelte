<script lang="ts">
	/**
	 * Home's one-time hint: after three days of use, not linked, shown once (devices.svelte.ts).
	 * Same words as the iPhone's.
	 */
	import Icon from '$lib/kit/Icon.svelte';
	import { devices, dismissHint, linked, openPairing } from '$lib/devices.svelte';
	import { W } from '$lib/devices';
	import DevicesIcon from 'phosphor-svelte/lib/DevicesIcon';
	import XIcon from 'phosphor-svelte/lib/XIcon';
</script>

{#if devices.hint && !linked()}
	<div
		class="flex items-center gap-4 rounded-lg bg-[linear-gradient(90deg,var(--color-accent-tile),var(--color-bg-elevated)_70%)] py-3 pr-3 pl-4"
		role="note"
		data-testid="devices-hint"
	>
		<div class="grid size-10 shrink-0 place-items-center rounded-full bg-accent text-black">
			<Icon icon={DevicesIcon} size={22} />
		</div>
		<p class="min-w-0 flex-1 text-sm font-bold text-white">{W.hint}</p>
		<button
			type="button"
			class="h-8 shrink-0 rounded-full bg-white px-4 text-sm font-bold text-black transition-transform hover:scale-[1.04]"
			onclick={() => openPairing()}
		>
			{W.link}
		</button>
		<button
			type="button"
			aria-label="Dismiss"
			class="grid size-8 shrink-0 place-items-center rounded-full text-white/70 hover:text-white"
			onclick={dismissHint}
		>
			<Icon icon={XIcon} size={16} />
		</button>
	</div>
{/if}
