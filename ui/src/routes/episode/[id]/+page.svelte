<script lang="ts">
	// Podcast episode page (spec: spotify-spec/screens/podcasts.md "Episode page"): header, date •
	// duration + progress, play / Your Episodes / share / …, the show notes with clickable
	// timestamps, and "See all episodes".
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api';
	import type { PodcastEpisode } from '$lib/api';
	import { ErrorState, IconButton, PageHeader, PlayButton, Skeleton, StickyHeader, channels, type Tint } from '$lib/kit';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import DownloadButton from '$lib/downloads/DownloadButton.svelte'; // stream: downloads
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import ExportIcon from 'phosphor-svelte/lib/ExportIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import { Icon } from '$lib/kit';
	import { isSaved, progressOf, remainingSecs } from '$lib/podcasts.svelte';
	import { playback, seekTo } from '$lib/player.svelte';
	import { getCached, putCached } from '$lib/pagecache';
	import ChapterRows from '$lib/nowplaying/ChapterRows.svelte';
	import { loadChapters } from '$lib/chapters.svelte';
	import { chapterAt } from '$lib/chapters';
	import {
		episodeDate,
		episodeShareUrl,
		isEpisodeCurrent,
		isEpisodePlaying,
		openEpisodeMenu,
		playEpisodes,
		shareEpisode,
		spokenDuration,
		toggleSavedToast
	} from '$lib/artistpods/pods';

	const id = $derived(page.params.id ?? '');
	let ep = $state.raw<PodcastEpisode | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let expanded = $state(false);
	let notesEl = $state<HTMLElement>();
	let overflowing = $state(false);

	async function load(cid: string) {
		const key = `episode:${cid}`;
		const hit = getCached<PodcastEpisode>(key);
		ep = hit ?? null;
		loading = !hit;
		error = null;
		expanded = false;
		try {
			const fresh = await api.podcastEpisode(cid);
			if (cid !== id) return;
			ep = fresh;
			putCached(key, fresh);
		} catch (e) {
			if (cid !== id) return;
			if (!hit) error = String(e);
		} finally {
			if (cid === id) loading = false;
		}
	}
	$effect(() => {
		if (id) load(id);
	});

	$effect(() => {
		void ep?.description;
		const el = notesEl;
		if (!el) return;
		const check = () => (overflowing = el.scrollHeight > el.clientHeight + 2);
		check();
		const ro = new ResizeObserver(check);
		ro.observe(el);
		return () => ro.disconnect();
	});

	const p = $derived(ep ? progressOf(ep) : null);
	const left = $derived(ep ? remainingSecs(ep) : null);
	const duration = $derived(p?.durationSecs || ep?.durationSecs || 0);
	const saved = $derived(ep ? isSaved(ep.id) : false);
	const playing = $derived(ep ? isEpisodePlaying(ep.id) : false);
	const showHref = $derived(ep?.showId ? `/show/${encodeURIComponent(ep.showId)}` : null);

	function play() {
		if (!ep) return;
		playEpisodes([ep], 0, { id: ep.showId ?? ep.id, title: ep.showTitle, thumbnail: ep.thumbnail });
	}

	// Show-note timestamps are `<a data-seek="SECS">`: seek when this episode is the one playing,
	// else start it at that point.
	async function onNotesClick(e: MouseEvent) {
		const a = (e.target as HTMLElement).closest('a[data-seek]') as HTMLAnchorElement | null;
		if (!a || !ep) return;
		e.preventDefault();
		const secs = Number(a.dataset.seek);
		if (!Number.isFinite(secs)) return;
		playAt(secs);
	}

	/** Seek when this episode is the one playing, else start it there: one play call carrying the
	 *  start, so it holds while the stream resolves (stream: pod-fixes). */
	function playAt(secs: number) {
		if (!ep) return;
		if (isEpisodeCurrent(ep.id)) return seekTo(secs);
		const ctx = { kind: 'podcast' as const, id: ep.showId ?? ep.id, title: ep.showTitle, thumbnail: ep.thumbnail ?? undefined };
		return api.playPlaylistFrom([api.episodeSong(ep)], 0, ctx, undefined, undefined, undefined, secs);
	}

	// stream: chapters — loaded beside the page, shown under the description when there are any.
	let chapters = $state.raw<api.PodcastChapter[]>([]);
	let allChapters = $state(false);
	$effect(() => {
		const cid = id;
		chapters = [];
		allChapters = false;
		if (!cid) return;
		loadChapters(cid).then((list) => cid === id && (chapters = list));
	});
	const activeChapter = $derived(ep && isEpisodeCurrent(ep.id) ? chapterAt(chapters, playback.position) : -1);
	const playChapter = (c: api.PodcastChapter) => playAt(c.startSecs);

	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();
</script>

<svelte:head><title>{ep?.title ?? 'Episode'}</title></svelte:head>

{#if loading && !ep}
	<div aria-busy="true">
		<div class="px-[var(--content-spacing)] pt-[60px] pb-6"><Skeleton variant="header" /></div>
		<div class="flex flex-col gap-3 px-[var(--content-spacing)] py-6"><Skeleton width="30%" /><Skeleton width="70%" /><Skeleton width="65%" /></div>
	</div>
{:else if error && !ep}
	<div class="px-[var(--content-spacing)] py-16">
		<ErrorState message="Couldn't load this episode. Try reloading the page." onRetry={() => load(id)} />
	</div>
{:else if ep}
	<StickyHeader title={ep.title} tint={tint?.color} {playing} onPlay={play} {sentinel} />
	<PageHeader type="Podcast Episode" title={ep.title} image={ep.thumbnail} maxTitle={48} bind:tintOut={tint}>
		{#snippet meta()}
			{#if showHref}
				<a href={showHref} class="-mt-1 text-xl font-bold text-white hover:underline">{ep!.showTitle}</a>
			{:else}
				<span class="-mt-1 text-xl font-bold text-white">{ep!.showTitle}</span>
			{/if}
		{/snippet}
	</PageHeader>

	<div class="relative">
		{#if tint}
			<div
				class="pointer-events-none absolute inset-x-0 top-0 h-[232px]"
				style="background:linear-gradient(rgb({channels(tint.color)} / 0.7), transparent)"
			></div>
			<div class="noise pointer-events-none absolute inset-x-0 top-0 h-[232px]"></div>
		{/if}
		<div class="relative px-[var(--content-spacing)] pt-6">
			<div class="flex items-center gap-2 text-sm text-text-subdued">
				<span>
					{episodeDate(ep.publishedAt)}{#if duration || left !== null}<span aria-hidden="true">{' • '}</span>{/if}{#if left !== null}{spokenDuration(left)} left{:else if duration}{spokenDuration(duration)}{/if}
				</span>
				{#if left !== null && p}
					<div class="h-1 w-[85px] overflow-hidden rounded-sm bg-white/30">
						<div class="h-full rounded-sm bg-white" style="width:{Math.min(100, p.fraction * 100)}%"></div>
					</div>
				{:else if p?.played}
					<span class="text-accent"><Icon icon={CheckCircleIcon} size={16} fill /></span><span>Played</span>
				{/if}
			</div>
		</div>
		<div class="relative flex items-center gap-5 px-[var(--content-spacing)] py-4">
			<div bind:this={sentinel}><PlayButton size="lg" {playing} label={playing ? 'Pause' : `Play ${ep.title}`} onclick={play} /></div>
			<IconButton
				icon={saved ? CheckCircleIcon : PlusCircleIcon}
				fill={saved}
				active={saved}
				size="lg"
				class="!size-8"
				label={saved ? 'Remove from Your Episodes' : 'Add to Your Episodes'}
				onclick={() => toggleSavedToast(ep!)}
			/>
			<DownloadButton target={{ id: ep.id, kind: 'episode', title: ep.title, subtitle: ep.showTitle ?? null, thumbnail: ep.thumbnail ?? null }} songs={[api.episodeSong(ep)]} size="md" iconSize={32} />
			{#if episodeShareUrl(ep)}
				<IconButton icon={ExportIcon} size="lg" class="!size-8" label="Share" onclick={() => shareEpisode(ep!)} />
			{/if}
			<IconButton icon={DotsThreeIcon} size="lg" class="!size-8" label={`More options for ${ep.title}`} onclick={(e) => openEpisodeMenu(e, ep!)} />
		</div>

		<div class="relative flex max-w-[800px] flex-col gap-6 px-[var(--content-spacing)] pb-10">
			<div class="flex gap-6 border-b border-white/10" role="tablist">
				<span role="tab" aria-selected="true" class="border-b-2 border-accent pb-3 text-sm font-bold text-white">Description</span>
			</div>
			{#if ep.description}
				<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
				<div
					bind:this={notesEl}
					class="episode-notes text-base text-text-subdued {expanded ? '' : 'max-h-[120px] overflow-hidden'}"
					onclick={onNotesClick}
				>
					{@html ep.description}
				</div>
				{#if overflowing || expanded}
					<button type="button" class="self-start text-base font-bold text-white hover:underline" onclick={() => (expanded = !expanded)}>
						{expanded ? 'Show less' : '... Show more'}
					</button>
				{/if}
			{:else}
				<p class="text-base text-text-subdued">This episode has no description.</p>
			{/if}
			{#if chapters.length}
				<section class="flex flex-col gap-2" aria-labelledby="chapters-heading" data-testid="episode-chapters">
					<h2 id="chapters-heading" class="text-2xl font-bold text-white">Chapters</h2>
					<div class="-mx-2">
						<ChapterRows chapters={allChapters ? chapters : chapters.slice(0, 5)} active={activeChapter} onpick={playChapter} />
					</div>
					{#if chapters.length > 5}
						<button type="button" class="self-start text-base font-bold text-white hover:underline" onclick={() => (allChapters = !allChapters)}>
							{allChapters ? 'Show fewer chapters' : `Show all ${chapters.length} chapters`}
						</button>
					{/if}
				</section>
			{/if}
			{#if showHref}
				<button type="button" class="btn-outline self-start" onclick={() => goto(showHref!)}>See all episodes</button>
			{/if}
		</div>
	</div>
{/if}

<style>
	.episode-notes :global(a) {
		color: white;
		text-decoration: underline;
	}
	.episode-notes :global(a[data-seek]) {
		color: var(--color-accent-text);
		text-decoration: none;
		font-variant-numeric: tabular-nums;
	}
	.episode-notes :global(a[data-seek]:hover) {
		text-decoration: underline;
	}
	.episode-notes :global(p) {
		margin-bottom: 0.75em;
	}
	.episode-notes :global(ul),
	.episode-notes :global(ol) {
		margin: 0 0 0.75em 1.25em;
		list-style: disc;
	}
</style>
