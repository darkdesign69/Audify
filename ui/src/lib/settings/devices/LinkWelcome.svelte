<script lang="ts">
	/**
	 * First run, after the import step: "Using Spotless on your iPhone too?", once, optional.
	 * Welcome.svelte queues it when the import step ends (skipped or read); it waits for the import
	 * dialog to close. Same screen as the iPhone's.
	 */
	import Modal from '$lib/kit/Modal.svelte';
	import Icon from '$lib/kit/Icon.svelte';
	import { imp } from '$lib/import.svelte';
	import { PRIMARY, QUIET, TITLE, LINE } from '$lib/import/onboard';
	import { devices, linked, openPairing, welcomeShown } from '$lib/devices.svelte';
	import { W } from '$lib/devices';
	import DevicesIcon from 'phosphor-svelte/lib/DevicesIcon';

	$effect(() => {
		if (!devices.welcomeQueued || imp.open || devices.pair.open) return;
		const t = setTimeout(() => {
			if (linked()) return welcomeShown();
			devices.welcome = true;
		}, 400);
		return () => clearTimeout(t);
	});

	function linkNow() {
		welcomeShown();
		openPairing();
	}
</script>

<Modal bind:open={devices.welcome} width={480} onClose={welcomeShown}>
	<div class="flex flex-col items-center gap-3 pt-2 text-center" data-testid="devices-welcome">
		<div class="mb-3 grid size-16 place-items-center rounded-full bg-accent-tile text-accent">
			<Icon icon={DevicesIcon} size={32} />
		</div>
		<h2 class={TITLE}>{W.welcomeTitle}</h2>
		<p class="{LINE} max-w-[360px]">{W.welcomeLine}</p>
		<div class="mt-6 flex w-full max-w-[360px] flex-col gap-3">
			<button type="button" class={PRIMARY} onclick={linkNow}>{W.linkNow}</button>
			<button type="button" class={QUIET} onclick={welcomeShown}>{W.notNow}</button>
		</div>
	</div>
</Modal>
