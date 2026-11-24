<script lang="ts">
	// Phone layout (Android build): a search header, the page, a compact player and a bottom
	// navigation bar. The desktop shell's three panels, resize handles and window controls do not
	// fit a 400px screen. Everything it shows is the same pages and stores the desktop uses.
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import type { Snippet } from 'svelte';
	import { playback, auth, ui } from '$lib/player.svelte';
	import * as api from '$lib/api';
	import { thumb } from '$lib/thumb';
	import { shell } from './layout.svelte';
	import { openSettings } from './settings';
	import LibrarySidebar from './LibrarySidebar.svelte';
	import NowPlayingPanel from '$lib/nowplaying/NowPlayingPanel.svelte';
	import HouseIcon from 'phosphor-svelte/lib/HouseIcon';
	import MagnifyingGlassIcon from 'phosphor-svelte/lib/MagnifyingGlassIcon';
	import BooksIcon from 'phosphor-svelte/lib/BooksIcon';
	import GearIcon from 'phosphor-svelte/lib/GearIcon';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import SkipForwardIcon from 'phosphor-svelte/lib/SkipForwardIcon';
	import SkipBackIcon from 'phosphor-svelte/lib/SkipBackIcon';
	import CaretDownIcon from 'phosphor-svelte/lib/CaretDownIcon';
	import ArrowLeftIcon from 'phosphor-svelte/lib/ArrowLeftIcon';

	let { children, scroller = $bindable(null) }: { children: Snippet; scroller?: HTMLElement | null } =
		$props();

	let full = $state(false);
	let query = $state('');

	const path = $derived(page.url.pathname);
	const onHome = $derived(path === '/' && !shell.libraryExpanded);
	const onSearch = $derived(path.startsWith('/search') && !shell.libraryExpanded);
	const onLibrary = $derived(shell.libraryExpanded);
	const now = $derived(playback.now);

	function tab(where: 'home' | 'search' | 'library') {
		full = false;
		shell.lyricsOpen = false;
		if (where === 'library') {
			shell.libraryExpanded = true;
			return;
		}
		shell.libraryExpanded = false;
		goto(where === 'home' ? '/' : '/search');
	}

	function submit(e: Event) {
		e.preventDefault();
		const q = query.trim();
		shell.libraryExpanded = false;
		goto(q ? `/search?q=${encodeURIComponent(q)}` : '/search');
		(document.activeElement as HTMLElement | null)?.blur?.();
	}

	// Android's back button / swipe: close the player sheet first, then the library page.
	$effect(() => {
		const onBack = () => {
			if (full) full = false;
		};
		window.addEventListener('popstate', onBack);
		return () => window.removeEventListener('popstate', onBack);
	});
</script>

<div
	data-app-shell
	class="flex h-[100dvh] w-screen flex-col overflow-hidden bg-bg-app text-white"
	style="padding-top:env(safe-area-inset-top)"
>
	<header class="flex items-center gap-2 px-3 py-2">
		{#if path !== '/' && !onLibrary && !onSearch}
			<button class="p-2" aria-label="Back" onclick={() => history.back()}>
				<ArrowLeftIcon size={22} />
			</button>
		{/if}
		<form class="min-w-0 flex-1" onsubmit={submit}>
			<label class="flex items-center gap-2 rounded-full bg-white/10 px-4 py-2">
				<MagnifyingGlassIcon size={18} />
				<input
					class="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-white/50"
					type="search"
					enterkeyhint="search"
					placeholder="Search songs, artists, podcasts"
					bind:value={query}
				/>
			</label>
		</form>
		<button class="p-2" aria-label="Settings" onclick={() => { full = false; shell.libraryExpanded = false; openSettings(); }}>
			<GearIcon size={22} />
		</button>
	</header>

	<main
		id="main-view"
		data-app-main
		data-main-scroll
		class="scroll-area relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto rounded-t-lg bg-bg-base [container-type:inline-size]"
		style="--content-spacing:16px"
		bind:this={scroller}
	>
		{#if shell.libraryExpanded}
			<div class="h-full"><LibrarySidebar mode="page" /></div>
		{:else}
			{#key `${auth.epoch}:${ui.epoch}`}
				{@render children()}
			{/key}
		{/if}
	</main>

	{#if now}
		<button
			class="mx-2 mt-1 flex items-center gap-3 rounded-lg bg-white/10 p-2 text-left"
			onclick={() => (full = true)}
			aria-label="Open player"
		>
			{#if now.thumbnail}
				<img class="h-10 w-10 rounded object-cover" src={thumb(now.thumbnail, 96)} alt="" />
			{/if}
			<span class="min-w-0 flex-1">
				<span class="block truncate text-sm font-medium">{now.title}</span>
				<span class="block truncate text-xs text-white/60">{now.artists}</span>
			</span>
			<span
				role="button"
				tabindex="0"
				class="p-2"
				aria-label={playback.paused ? 'Play' : 'Pause'}
				onclick={(e) => { e.stopPropagation(); api.togglePause(); }}
				onkeydown={(e) => e.key === 'Enter' && api.togglePause()}
			>
				{#if playback.paused}<PlayIcon size={26} weight="fill" />{:else}<PauseIcon size={26} weight="fill" />{/if}
			</span>
			<span
				role="button"
				tabindex="0"
				class="p-2"
				aria-label="Next"
				onclick={(e) => { e.stopPropagation(); api.nextTrack(); }}
				onkeydown={(e) => e.key === 'Enter' && api.nextTrack()}
			>
				<SkipForwardIcon size={24} weight="fill" />
			</span>
		</button>
		<div
			class="mx-2 h-0.5 overflow-hidden rounded bg-white/10"
			aria-hidden="true"
		>
			<div
				class="h-full bg-white/70"
				style="width:{playback.duration ? Math.min(100, (playback.position / playback.duration) * 100) : 0}%"
			></div>
		</div>
	{/if}

	<nav
		class="grid grid-cols-3 border-t border-white/10 bg-bg-app"
		style="padding-bottom:env(safe-area-inset-bottom)"
		aria-label="Main"
	>
		<button class="flex flex-col items-center gap-0.5 py-2 text-xs {onHome ? 'text-white' : 'text-white/60'}" onclick={() => tab('home')}>
			<HouseIcon size={24} weight={onHome ? 'fill' : 'regular'} />Home
		</button>
		<button class="flex flex-col items-center gap-0.5 py-2 text-xs {onSearch ? 'text-white' : 'text-white/60'}" onclick={() => tab('search')}>
			<MagnifyingGlassIcon size={24} weight={onSearch ? 'fill' : 'regular'} />Search
		</button>
		<button class="flex flex-col items-center gap-0.5 py-2 text-xs {onLibrary ? 'text-white' : 'text-white/60'}" onclick={() => tab('library')}>
			<BooksIcon size={24} weight={onLibrary ? 'fill' : 'regular'} />Library
		</button>
	</nav>
</div>

{#if full && now}
	<div class="fixed inset-0 z-50 flex flex-col bg-bg-app" style="padding-top:env(safe-area-inset-top);padding-bottom:env(safe-area-inset-bottom)">
		<div class="flex items-center justify-between px-2 py-1">
			<button class="p-3" aria-label="Close player" onclick={() => (full = false)}>
				<CaretDownIcon size={26} />
			</button>
			<span class="text-sm text-white/70">Now playing</span>
			<span class="w-12"></span>
		</div>
		<div class="min-h-0 flex-1 overflow-y-auto"><NowPlayingPanel /></div>
		<div class="flex items-center justify-center gap-8 py-3">
			<button class="p-2" aria-label="Previous" onclick={() => api.prevTrack()}><SkipBackIcon size={32} weight="fill" /></button>
			<button class="rounded-full bg-white p-4 text-black" aria-label={playback.paused ? 'Play' : 'Pause'} onclick={() => api.togglePause()}>
				{#if playback.paused}<PlayIcon size={30} weight="fill" />{:else}<PauseIcon size={30} weight="fill" />{/if}
			</button>
			<button class="p-2" aria-label="Next" onclick={() => api.nextTrack()}><SkipForwardIcon size={32} weight="fill" /></button>
		</div>
		<div class="flex items-center gap-3 px-6 pb-3">
			<input
				class="w-full accent-white"
				type="range"
				min="0"
				max={Math.max(1, Math.floor(playback.duration))}
				value={Math.floor(playback.position)}
				aria-label="Seek"
				onchange={(e) => api.seek(Number((e.currentTarget as HTMLInputElement).value))}
			/>
		</div>
	</div>
{/if}
