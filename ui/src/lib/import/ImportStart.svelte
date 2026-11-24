<script lang="ts">
	/**
	 * stream: onboard — the first screens of the Spotify import, one idea each (same flow and copy
	 * as iOS `Import.swift`):
	 *
	 *   start   "Bring your music from Spotify?"
	 *           [Paste your Spotify profile link] all your public playlists
	 *           [Use your Spotify data file] also private playlists and Liked Songs     Not now
	 *   link    "Paste your profile link" (or any playlist or album link): one field + Continue;
	 *           "Show me how" expands 4 numbered steps for copying the profile link in Spotify
	 *   export  three numbered steps to get the Spotify data file + Choose file (or drop it)
	 *
	 * Used by the first-run step (Welcome.svelte, which hands the read over to the import dialog)
	 * and by the import dialog itself when nothing has been read yet. The user signs in to Spotify
	 * in their own browser; Spotless never sees it.
	 */
	import { open as pickFile } from '@tauri-apps/plugin-dialog';
	import Icon from '$lib/kit/Icon.svelte';
	import Logo from '$lib/kit/Logo.svelte';
	import * as api from '$lib/api';
	import { toast } from '$lib/player.svelte';
	import { t } from '$lib/i18n.svelte';
	import { imp, readFile, readLink } from '$lib/import.svelte';
	import CaretLeftIcon from 'phosphor-svelte/lib/CaretLeftIcon';
	import ArrowSquareOutIcon from 'phosphor-svelte/lib/ArrowSquareOutIcon';
	import InfoIcon from 'phosphor-svelte/lib/InfoIcon';
	import UserCircleIcon from 'phosphor-svelte/lib/UserCircleIcon';
	import UserIcon from 'phosphor-svelte/lib/UserIcon';
	import ShareNetworkIcon from 'phosphor-svelte/lib/ShareNetworkIcon';
	import LinkIcon from 'phosphor-svelte/lib/LinkIcon';
	import { LINE, PRIMARY, QUIET, SECONDARY, TITLE } from './onboard';

	let {
		read = (go: () => void) => go(),
		onNotNow
	}: {
		/** Runs a read (link or file). The first-run step closes itself first. */
		read?: (go: () => void) => void;
		onNotNow: () => void;
	} = $props();

	const SPOTIFY_PRIVACY = 'https://www.spotify.com/account/privacy/';

	let step = $state<'start' | 'link' | 'export'>('start');
	let link = $state('');
	let dragging = $state(false);
	let showHow = $state(false);

	// "Show me how": copying your profile link in Spotify desktop (iOS has the phone version).
	const HOW = [
		{ icon: UserCircleIcon, title: 'onboard.how1_title', body: 'onboard.how1_body' },
		{ icon: UserIcon, title: 'onboard.how2_title', body: 'onboard.how2_body' },
		{ icon: ShareNetworkIcon, title: 'onboard.how3_title', body: 'onboard.how3_body' },
		{ icon: LinkIcon, title: 'onboard.how4_title', body: 'onboard.how4_body' }
	] as const;

	async function choose() {
		const path = await pickFile({
			multiple: false,
			title: t('onboard.pick_file'),
			filters: [{ name: t('import.file_filter'), extensions: ['zip', 'json', 'csv'] }]
		}).catch(() => null);
		if (typeof path === 'string') read(() => readFile(path));
	}

	function submit(e: Event) {
		e.preventDefault();
		const l = link.trim();
		if (l) read(() => readLink(l));
	}

	function openSpotify() {
		api.openExternal(SPOTIFY_PRIVACY).catch((e) => toast.error(t('toasts.browser_failed', { error: String(e) })));
	}

	// The data file dropped on this step. `stopPropagation` keeps the window's own drop handler
	// from reading it a second time.
	function drop(e: DragEvent) {
		dragging = false;
		const file = Array.from(e.dataTransfer?.files ?? []).find((f) => /\.(zip|json|csv)$/i.test(f.name));
		if (!file) return;
		e.preventDefault();
		e.stopPropagation();
		read(() => readFile(file));
	}
</script>

{#snippet back()}
	<button
		type="button"
		class="-ml-2 grid size-8 place-items-center rounded-full text-text-subdued transition-colors hover:text-white"
		aria-label={t('common.back')}
		onclick={() => (step = 'start')}
	>
		<Icon icon={CaretLeftIcon} size={20} />
	</button>
{/snippet}

{#snippet numbered(n: number, title: string, body: string, last: boolean)}
	<li class="flex gap-4">
		<div class="flex flex-col items-center">
			<span class="grid size-8 shrink-0 place-items-center rounded-full bg-white text-sm font-bold text-black">{n}</span>
			{#if !last}<span class="w-0.5 flex-1 bg-bg-elevated-highlight"></span>{/if}
		</div>
		<div class="flex min-w-0 flex-col items-start gap-1 {last ? '' : 'pb-6'}">
			<span class="pt-1 text-base font-bold text-white">{title}</span>
			<span class="text-sm text-text-subdued">{body}</span>
			{#if n === 1}
				<button type="button" class="btn-outline mt-2" onclick={openSpotify}>
					{t('onboard.open_spotify')}
					<Icon icon={ArrowSquareOutIcon} size={16} />
				</button>
			{/if}
		</div>
	</li>
{/snippet}

{#if step === 'start'}
	<div class="flex flex-col items-center gap-3 pt-2 text-center">
		<!-- Logo spot: the logo stream owns the mark (kit/Logo.svelte); keep this 56px slot. -->
		<div class="mb-3 grid size-14 place-items-center text-accent" data-logo-spot><Logo size={56} /></div>
		<h2 class={TITLE}>{t('onboard.title')}</h2>
		<p class="{LINE} max-w-[360px]">{t('onboard.line')}</p>
		<div class="mt-6 flex w-full max-w-[360px] flex-col gap-3">
			<button type="button" class="{PRIMARY} py-2" onclick={() => (step = 'link')}>
				<span class="flex flex-col">
					<span>{t('onboard.link_choice')}</span>
					<span class="text-[13px] font-normal text-black/70">{t('onboard.link_detail')}</span>
				</span>
			</button>
			<button type="button" class={SECONDARY} onclick={() => (step = 'export')}>
				<span>{t('onboard.everything_choice')}</span>
				<span class="text-[13px] font-normal text-text-subdued">{t('onboard.everything_detail')}</span>
			</button>
			<button type="button" class={QUIET} onclick={onNotNow}>{t('onboard.not_now')}</button>
		</div>
	</div>
{:else if step === 'link'}
	<form class="flex flex-col gap-6" onsubmit={submit}>
		<div class="flex flex-col gap-2">
			{@render back()}
			<h2 class={TITLE}>{t('onboard.link_title')}</h2>
			<p class={LINE}>{t('onboard.link_line')}</p>
		</div>
		<!-- svelte-ignore a11y_autofocus -->
		<input
			bind:value={link}
			autofocus
			spellcheck="false"
			aria-label={t('onboard.link_label')}
			placeholder="open.spotify.com/…"
			class="h-12 w-full min-w-0 rounded-md bg-bg-tinted px-4 text-base text-white outline-none placeholder:text-text-subdued focus:bg-bg-tinted-highlight focus:shadow-[inset_0_0_0_1px_#535353]"
		/>
		<div class="flex flex-col gap-4">
			<button
				type="button"
				class="btn-outline self-start"
				aria-expanded={showHow}
				onclick={() => (showHow = !showHow)}
			>
				<Icon icon={InfoIcon} size={16} />
				{showHow ? t('onboard.hide_how') : t('onboard.show_how')}
			</button>
			{#if showHow}
				<ol class="flex flex-col gap-4">
					{#each HOW as { icon, title, body }, i (title)}
						<li class="flex items-start gap-4">
							<span class="relative grid size-12 shrink-0 place-items-center rounded-md bg-bg-elevated-highlight text-white">
								<Icon {icon} size={24} />
								<span
									class="absolute -right-1.5 -bottom-1.5 grid size-5 place-items-center rounded-full bg-accent text-xs font-bold text-black"
									>{i + 1}</span
								>
							</span>
							<span class="flex min-w-0 flex-col gap-1 pt-0.5">
								<span class="text-base font-bold text-white">{t(title)}</span>
								<span class="text-sm text-text-subdued">{t(body)}</span>
							</span>
						</li>
					{/each}
				</ol>
			{/if}
		</div>
		<button type="submit" class={PRIMARY} disabled={!link.trim() || imp.reading}>
			{imp.reading ? t('import.reading') : t('import.continue')}
		</button>
	</form>
{:else}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="flex flex-col gap-6 rounded-md transition-shadow {dragging ? 'shadow-[0_0_0_2px_var(--color-accent)]' : ''}"
		ondragenter={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragover={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragleave={() => (dragging = false)}
		ondrop={drop}
	>
		<div class="flex flex-col gap-2">
			{@render back()}
			<h2 class={TITLE}>{t('onboard.export_title')}</h2>
			<p class={LINE}>{t('onboard.export_line')}</p>
		</div>
		<ol class="flex flex-col">
			{@render numbered(1, t('onboard.step1_title'), t('onboard.step1_body'), false)}
			{@render numbered(2, t('onboard.step2_title'), t('onboard.step2_body'), false)}
			{@render numbered(3, t('onboard.step3_title'), t('onboard.step3_body'), true)}
		</ol>
		<div class="flex flex-col gap-1">
			<button type="button" class={PRIMARY} onclick={choose} disabled={imp.reading}>
				{imp.reading ? t('import.reading') : t('import.choose_file')}
			</button>
			<p class="text-center text-xs text-text-subdued">{t('onboard.drop_hint')}</p>
		</div>
	</div>
{/if}
