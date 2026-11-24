<script lang="ts">
	/**
	 * Settings › Devices, the first section (same words as the iPhone's Devices section): linked or
	 * not, with whom, when it last synced, changes still waiting, the last problem in plain words,
	 * and Link a device / Sync now / Unlink (asks first). Live through `sync-changed`.
	 */
	import { onDestroy } from 'svelte';
	import * as api from '$lib/api';
	import Icon from '$lib/kit/Icon.svelte';
	import Pill from '$lib/kit/Pill.svelte';
	import { toast } from '$lib/kit/toast';
	import { confirmDialog } from '$lib/kit/confirm.svelte';
	import { devices, openPairing } from '$lib/devices.svelte';
	import { W, statusView, unlinkTitle } from '$lib/devices';
	import DevicesIcon from 'phosphor-svelte/lib/DevicesIcon';
	import DeviceMobileIcon from 'phosphor-svelte/lib/DeviceMobileIcon';
	import WarningCircleIcon from 'phosphor-svelte/lib/WarningCircleIcon';
	import ArrowsClockwiseIcon from 'phosphor-svelte/lib/ArrowsClockwiseIcon';

	let now = $state(Date.now() / 1000);
	const tick = setInterval(() => (now = Date.now() / 1000), 15_000);
	onDestroy(() => clearInterval(tick));

	const s = $derived(devices.status);
	const view = $derived(s ? statusView(s, now) : null);
	let busy = $state(false);

	async function syncNow() {
		busy = true;
		try {
			devices.status = await api.syncNow();
		} catch {
			toast.error("Couldn't sync right now. Try again.");
		} finally {
			busy = false;
		}
	}

	async function unlink() {
		if (!s) return;
		const ok = await confirmDialog({ title: unlinkTitle(s), body: W.unlinkBody, confirmLabel: W.unlink });
		if (!ok) return;
		try {
			devices.status = await api.syncUnpair();
		} catch {
			toast.error("Couldn't unlink. Try again.");
		}
	}
</script>

<section data-settings-section>
	<h2 id="devices" class="mt-6 mb-1 scroll-mt-4 text-base font-bold text-white">{W.section}</h2>
	<div data-settings-row class="py-1" data-testid="devices-status">
		<div class="rounded-lg bg-bg-elevated p-4">
		<div class="flex items-start gap-4">
			<div
				class="grid size-12 shrink-0 place-items-center rounded-full {s?.paired
					? 'bg-accent text-black'
					: 'bg-accent-tile text-accent'}"
			>
				<Icon icon={s?.paired ? DeviceMobileIcon : DevicesIcon} size={24} fill={s?.paired} />
			</div>
			<div class="min-w-0 flex-1">
				{#if view}
					<div class="truncate text-base font-bold text-white">{view.title}</div>
					<div class="mt-0.5 flex items-center gap-1.5 text-sm text-text-subdued">
						{#if s?.paired && !s.lastError && s.lastSync && !s.syncing}
							<span class="size-2 shrink-0 rounded-full bg-text-positive" aria-hidden="true"></span>
						{:else if s?.syncing}
							<Icon icon={ArrowsClockwiseIcon} size={14} class="shrink-0 animate-spin text-accent" />
						{/if}
						<span>{view.line}</span>
					</div>
					{#if view.pending}<div class="mt-0.5 text-sm text-text-subdued">{view.pending}</div>{/if}
					{#if view.error}
						<div class="mt-2 flex items-start gap-1.5 text-[13px] text-text-subdued">
							<Icon icon={WarningCircleIcon} size={16} class="mt-px shrink-0 text-text-warning" />
							<span>{view.error}</span>
						</div>
					{/if}
				{:else}
					<div class="text-base font-bold text-white">{W.notLinked}</div>
					<div class="mt-0.5 text-sm text-text-subdued">{W.notLinkedSub}</div>
				{/if}
			</div>
			{#if !s?.paired}
				<div class="shrink-0"><Pill variant="white" onclick={() => openPairing()}>{W.linkDevice}</Pill></div>
			{/if}
		</div>
		{#if s?.paired}
			<div class="mt-4 flex flex-wrap items-center gap-2 pl-16">
				<Pill variant="white" disabled={busy || s.syncing} onclick={syncNow}>{W.syncNow}</Pill>
				<Pill variant="outline" onclick={() => openPairing()}>{W.linkDevice}</Pill>
				<Pill variant="outline" onclick={unlink}>{W.unlink}</Pill>
			</div>
		{/if}
		</div>
	</div>
</section>
