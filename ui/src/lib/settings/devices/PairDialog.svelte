<script lang="ts">
	/**
	 * Linking a device, in two steps (docs/SYNC.md §6), with the same screens and words as the
	 * iPhone's PairSheet:
	 *
	 *   intro    "Link your iPhone": Show code / Enter code, and one line on the Local Network prompt
	 *   show     the big 6-digit code and its QR, "Waiting for your other device…", m:ss, Cancel;
	 *            ends linked, expired, or withdrawn after the last wrong try
	 *   enter    six boxes (auto-advance, paste), joins on the sixth digit; errors in plain words
	 *   success  "Linked" and what syncs
	 *
	 * Opened from Settings › Devices, the avatar menu, the first-run screen, Home's hint and a
	 * `spotless://pair` link (which joins with the link's code and addresses at once).
	 */
	import { onDestroy } from 'svelte';
	import * as api from '$lib/api';
	import Modal from '$lib/kit/Modal.svelte';
	import Icon from '$lib/kit/Icon.svelte';
	import { PRIMARY, QUIET, SECONDARY, TITLE, LINE } from '$lib/import/onboard';
	import { devices, refreshDevices, type PairStep } from '$lib/devices.svelte';
	import {
		EMPTY_CODE,
		LOCAL_NETWORK_SETTINGS,
		W,
		codeComplete,
		codeOf,
		countdown,
		formatCode,
		hostOutcome,
		joinError,
		typeInto,
		type JoinError
	} from '$lib/devices';
	import CodeInput from './CodeInput.svelte';
	import QrCode from './QrCode.svelte';
	import CaretLeftIcon from 'phosphor-svelte/lib/CaretLeftIcon';
	import CheckIcon from 'phosphor-svelte/lib/CheckIcon';
	import CircleNotchIcon from 'phosphor-svelte/lib/CircleNotchIcon';
	import DevicesIcon from 'phosphor-svelte/lib/DevicesIcon';
	import InfoIcon from 'phosphor-svelte/lib/InfoIcon';
	import WarningCircleIcon from 'phosphor-svelte/lib/WarningCircleIcon';

	let step = $state<PairStep>('intro');
	// show
	let shown = $state<{ code: string; qr: string; expiresAt: number } | null>(null);
	let peersAtStart = 0;
	let showFailed = $state(false);
	let now = $state(Date.now() / 1000);
	// enter
	let boxes = $state([...EMPTY_CODE]);
	let joining = $state(false);
	let error = $state<JoinError | null>(null);

	const clock = setInterval(() => (now = Date.now() / 1000), 500);
	onDestroy(() => clearInterval(clock));

	// Every (re)open starts over at the asked step.
	let seen = -1;
	$effect(() => {
		const p = devices.pair;
		if (!p.open || p.nonce === seen) return;
		seen = p.nonce;
		reset(p.step);
		if (p.qr) joinWith(null, p.qr);
		else if (p.step === 'show') startShow();
	});

	function reset(to: PairStep) {
		step = to;
		shown = null;
		showFailed = false;
		boxes = [...EMPTY_CODE];
		joining = false;
		error = null;
		if (import.meta.env.DEV) {
			const c = new URLSearchParams(location.search).get('joincode');
			if (c && to === 'enter')
				setTimeout(() => {
					boxes = typeInto(EMPTY_CODE, 0, c).boxes;
					if (codeComplete(boxes)) joinWith(codeOf(boxes));
				}, 400);
		}
	}

	// ── show a code ──
	async function startShow() {
		step = 'show';
		shown = null;
		showFailed = false;
		peersAtStart = devices.status?.peers.length ?? 0;
		try {
			const p = await api.syncStartPairing();
			shown = { code: p.code, qr: p.qr, expiresAt: p.expiresAt };
			await refreshDevices();
		} catch {
			showFailed = true;
		}
	}
	const outcome = $derived(
		step === 'show' && shown && devices.status ? hostOutcome(devices.status, peersAtStart, shown.expiresAt, now) : 'waiting'
	);
	$effect(() => {
		if (outcome === 'linked') step = 'success';
	});

	// ── enter a code ──
	async function joinWith(code: string | null, qr?: string) {
		if (joining) return;
		step = 'enter';
		if (qr) {
			const c = /[?&]code=(\d{6})/.exec(qr)?.[1];
			if (c) boxes = c.split('');
		}
		joining = true;
		error = null;
		try {
			const s = await api.syncJoin(code, qr);
			devices.status = s;
			step = 'success';
		} catch (e) {
			error = joinError(e);
			if (error.kind === 'wrong') boxes = [...EMPTY_CODE];
		} finally {
			joining = false;
		}
	}

	function close() {
		if (step === 'show' && outcome === 'waiting' && shown) api.syncCancelPairing().catch(() => {});
		devices.pair.open = false;
	}
	function back() {
		if (step === 'show' && shown) api.syncCancelPairing().catch(() => {});
		reset('intro');
	}
	const openNetworkSettings = () => api.openExternal(LOCAL_NETWORK_SETTINGS).catch(() => {});
</script>

{#snippet backButton()}
	<button
		type="button"
		class="-mt-2 -ml-2 grid size-8 place-items-center self-start rounded-full text-text-subdued transition-colors hover:text-white"
		aria-label="Back"
		onclick={back}
	>
		<Icon icon={CaretLeftIcon} size={20} />
	</button>
{/snippet}

{#snippet note(text: string)}
	<p class="flex items-start gap-2 text-left text-[13px] text-text-subdued">
		<Icon icon={InfoIcon} size={16} class="mt-px shrink-0" />
		<span>{text}</span>
	</p>
{/snippet}

<Modal bind:open={devices.pair.open} width={480} onClose={close}>
	<div class="flex min-h-[420px] flex-col" data-testid="pair-dialog" data-step={step}>
		{#if step === 'intro'}
			<div class="flex flex-1 flex-col items-center gap-3 pt-2 text-center">
				<div class="mb-3 grid size-16 place-items-center rounded-full bg-accent-tile text-accent">
					<Icon icon={DevicesIcon} size={32} />
				</div>
				<h2 class={TITLE}>{W.introTitle}</h2>
				<p class="{LINE} max-w-[380px]">{W.introBody}</p>
				<div class="mt-6 flex w-full max-w-[360px] flex-col gap-3">
					<button type="button" class={PRIMARY} onclick={startShow}>{W.showCode}</button>
					<button type="button" class={SECONDARY} onclick={() => reset('enter')}>{W.enterCode}</button>
				</div>
				<div class="mt-auto max-w-[360px] pt-6">{@render note(W.localNetwork)}</div>
			</div>
		{:else if step === 'show'}
			<div class="flex flex-1 flex-col items-center gap-4 text-center">
				{@render backButton()}
				<h2 class="-mt-4 text-xl font-bold text-white">{W.showHeading}</h2>
				{#if showFailed}
					<p class="{LINE} pt-10">{joinError('').message}</p>
					<button type="button" class="{PRIMARY} mt-4 max-w-[280px]" onclick={startShow}>{W.newCode}</button>
				{:else if !shown}
					<div class="grid flex-1 place-items-center"><Icon icon={CircleNotchIcon} size={28} class="animate-spin text-text-subdued" /></div>
				{:else if outcome === 'expired' || outcome === 'tries'}
					<p class="text-[44px] leading-none font-extrabold tracking-[0.08em] text-white/25 tabular-nums line-through decoration-2">
						{formatCode(shown.code)}
					</p>
					<p class="{LINE} max-w-[340px]">{outcome === 'expired' ? W.expired : W.tooManyTries}</p>
					<button type="button" class="{PRIMARY} mt-2 max-w-[280px]" onclick={startShow}>{W.newCode}</button>
				{:else}
					<p
						class="text-[56px] leading-none font-extrabold tracking-[0.08em] text-white tabular-nums select-all"
						data-testid="pair-code"
						aria-label="Code {shown.code.split('').join(' ')}"
					>
						{formatCode(shown.code)}
					</p>
					<div class="flex flex-col items-center gap-2">
						<QrCode value={shown.qr} />
						<p class="text-[13px] text-text-subdued">{W.qrCaption}</p>
					</div>
					<div class="mt-1 flex items-center gap-2 text-sm text-white">
						<Icon icon={CircleNotchIcon} size={16} class="animate-spin text-accent" />
						{W.waiting}
					</div>
					<p class="text-[13px] text-text-subdued tabular-nums">Code expires in {countdown(shown.expiresAt, now)}</p>
					<button type="button" class="{QUIET} mt-auto" onclick={close}>{W.cancel}</button>
				{/if}
			</div>
		{:else if step === 'enter'}
			<form
				class="flex flex-1 flex-col items-center gap-5 text-center"
				onsubmit={(e) => {
					e.preventDefault();
					if (codeComplete(boxes)) joinWith(codeOf(boxes));
				}}
			>
				{#if !devices.pair.qr}{@render backButton()}{/if}
				<h2 class="text-xl font-bold text-white {devices.pair.qr ? 'pt-6' : '-mt-4'}">{W.enterHeading}</h2>
				<CodeInput
					bind:boxes
					disabled={joining}
					invalid={!!error && error.kind === 'wrong'}
					onComplete={(c) => joinWith(c)}
					onLink={(l) => joinWith(null, l)}
				/>
				{#if error}
					<div
						role="alert"
						class="flex w-full max-w-[380px] items-start gap-3 rounded-lg bg-negative/10 p-3 text-left text-sm text-white ring-1 ring-negative/30"
						data-testid="pair-error"
						data-kind={error.kind}
					>
						<Icon icon={WarningCircleIcon} size={20} fill class="mt-px shrink-0 text-text-negative" />
						<div class="flex min-w-0 flex-col items-start gap-2">
							<span>{error.message}</span>
							{#if error.kind === 'permission'}
								<button type="button" class="btn-outline" onclick={openNetworkSettings}>{W.openSystemSettings}</button>
							{/if}
						</div>
					</div>
				{/if}
				<div class="mt-auto flex w-full max-w-[360px] flex-col gap-3 pt-2">
					<button type="submit" class={PRIMARY} disabled={joining || !codeComplete(boxes)}>
						{#if joining}<Icon icon={CircleNotchIcon} size={18} class="animate-spin" />{W.linking}{:else}{W.link}{/if}
					</button>
					{@render note(W.localNetwork)}
				</div>
			</form>
		{:else}
			<div class="flex flex-1 flex-col items-center gap-3 pt-6 text-center" data-testid="pair-success">
				<div class="mb-3 grid size-20 place-items-center rounded-full bg-accent text-black shadow-[0_0_48px_rgb(255_45_122/0.45)]">
					<Icon icon={CheckIcon} size={40} weight="bold" />
				</div>
				<h2 class={TITLE}>{W.successTitle}</h2>
				<p class="{LINE} max-w-[380px]">{W.successBody}</p>
				<button type="button" class="{PRIMARY} mt-auto max-w-[360px]" onclick={() => (devices.pair.open = false)}>{W.done}</button>
			</div>
		{/if}
	</div>
</Modal>
