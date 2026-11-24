<script lang="ts">
	// stream: audiobooks — the book page (Spotify's audiobook layout, our wording): cover header
	// with author, narrator and length; Play/Resume, save, download, more; About; Chapters.
	import { page } from '$app/state';
	import { ActionBar, EmptyState, ErrorState, PageHeader, Skeleton, StickyHeader, openMenu, toast, type Tint } from '$lib/kit';
	import type { MenuEntry } from '$lib/kit/menus';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import LinkIcon from 'phosphor-svelte/lib/LinkIcon';
	import ShareFatIcon from 'phosphor-svelte/lib/ShareFatIcon';
	import BookOpenIcon from 'phosphor-svelte/lib/BookOpenIcon';
	import * as api from '$lib/api';
	import { playback } from '$lib/player.svelte';
	import { getCached, putCached } from '$lib/pagecache';
	import { copyText } from '$lib/clipboard';
	import { spokenDuration } from '$lib/artistpods/pods';
	import {
		audiobookPage,
		isBookPlaying,
		isBookSaved,
		playBook,
		toggleBookSaved,
		type AudiobookPage
	} from '$lib/audiobooks.svelte';
	import ChapterRow from '$lib/audiobooks/ChapterRow.svelte';

	const id = $derived(page.params.id ?? '');
	let data = $state.raw<AudiobookPage | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let aboutOpen = $state(false);

	async function load(cid: string) {
		const key = `audiobook:${cid}`;
		const hit = getCached<AudiobookPage>(key);
		if (hit) data = hit;
		loading = !hit;
		error = null;
		try {
			const fresh = await audiobookPage(cid);
			if (cid !== id) return;
			data = fresh;
			putCached(key, fresh);
		} catch (e) {
			if (cid === id && !hit) error = String(e);
		} finally {
			if (cid === id) loading = false;
		}
	}

	let lastId = '';
	$effect(() => {
		const cid = id;
		if (cid && cid !== lastId) {
			lastId = cid;
			data = null;
			aboutOpen = false;
			load(cid);
		}
	});

	// Progress moves while a chapter plays: refresh Play/Resume when the playing chapter changes.
	let lastNow = '';
	$effect(() => {
		const now = playback.now?.videoId ?? '';
		if (now !== lastNow) {
			const prev = lastNow;
			lastNow = now;
			if (data && prev && data.chapters.some((c) => c.id === prev || c.id === now)) load(id);
		}
	});

	const book = $derived(data?.book);
	const saved = $derived(book ? isBookSaved(book.id) || book.saved : false);
	const playing = $derived(isBookPlaying(data) && !playback.paused);
	const resumeLabel = $derived(data?.resume.started && !data.resume.finished ? 'Resume' : 'Play');
	const resumeChapter = $derived(data && data.resume.started && !data.resume.finished ? data.chapters[data.resume.index] : null);

	function play() {
		if (!data) return;
		if (isBookPlaying(data)) return api.togglePause();
		playBook(data);
	}

	async function save() {
		if (!book) return;
		try {
			const now = await toggleBookSaved(book);
			toast(now ? 'Added to Your Library' : 'Removed from Your Library');
		} catch (e) {
			toast.error(String(e));
		}
	}

	function more(e: MouseEvent) {
		const entries: MenuEntry[] = [
			{ label: saved ? 'Remove from Your Library' : 'Add to Your Library', icon: saved ? CheckCircleIcon : PlusCircleIcon, onSelect: save }
		];
		const url = data?.librivoxUrl ?? (book ? `https://archive.org/details/${book.id}` : null);
		if (url)
			entries.push(
				{ type: 'separator' },
				{
					type: 'sub',
					label: 'Share',
					icon: ShareFatIcon,
					items: [
						{
							label: 'Copy link to audiobook',
							icon: LinkIcon,
							onSelect: async () => {
								await copyText(url);
								toast('Link copied to clipboard');
							}
						}
					]
				}
			);
		openMenu(e, entries);
	}

	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();
</script>

<svelte:head><title>{book?.title ?? 'Audiobook'}</title></svelte:head>

{#if loading && !book}
	<div aria-busy="true">
		<div class="px-[var(--content-spacing)] pt-[60px] pb-6"><Skeleton variant="header" /></div>
		<div class="px-[var(--content-spacing)] py-6"><Skeleton variant="row" count={6} /></div>
	</div>
{:else if error && !book}
	<div class="px-[var(--content-spacing)] py-16">
		<ErrorState message="Couldn't load this audiobook. Check your connection and try again." onRetry={() => load(id)} />
	</div>
{:else if book && data}
	<StickyHeader title={book.title} tint={tint?.color} {sentinel} {playing} onPlay={play} />
	<PageHeader type="Audiobook" title={book.title} image={book.thumbnail ?? undefined} bind:tintOut={tint}>
		{#snippet meta()}
			<div class="flex flex-col gap-1">
				<span class="text-xl font-bold text-white">{book.author}</span>
				{#if book.narrator}<span class="text-sm text-text-subdued">Read by {book.narrator}</span>{/if}
				<span class="text-sm text-[#cdcdcd]"
					>{[book.durationSecs ? spokenDuration(book.durationSecs) : '', book.chapterCount ? `${book.chapterCount} chapters` : ''].filter(Boolean).join(' • ')}</span
				>
			</div>
		{/snippet}
	</PageHeader>

	<ActionBar
		bind:sentinel
		tint={tint?.rgb}
		{playing}
		playLabel={playing ? `Pause ${book.title}` : `${resumeLabel} ${book.title}`}
		onPlay={play}
		{saved}
		onSave={save}
		saveLabel={saved ? 'Remove from Your Library' : 'Add to Your Library'}
		download={{ id: `abook:${book.id}`, kind: 'audiobook', title: book.title, subtitle: book.author, thumbnail: book.thumbnail ?? null }}
		downloadSongs={data.chapters.map(api.episodeSong)}
		onMore={more}
		moreLabel={`More options for ${book.title}`}
	>
		<div class="flex flex-col gap-8 px-[var(--content-spacing)] pb-10">
			{#if resumeChapter}
				<button type="button" class="-mt-2 self-start text-sm text-text-subdued hover:text-white" onclick={play}>
					{resumeLabel}: <span class="font-bold text-white">{resumeChapter.title}</span>
					{#if book.progress != null && book.progress >= 0.01}<span aria-hidden="true">{' • '}</span>{Math.round(book.progress * 100)}% heard{/if}
				</button>
			{/if}

			{#if book.description}
				<section>
					<h2 class="mb-4 font-title text-xl font-bold text-white">About this audiobook</h2>
					<p class="max-w-[720px] text-base whitespace-pre-line text-text-subdued {aboutOpen ? '' : 'line-clamp-4'}">{book.description}</p>
					{#if book.description.length > 300}
						<button type="button" class="mt-2 text-base font-bold text-white hover:underline" onclick={() => (aboutOpen = !aboutOpen)}>
							{aboutOpen ? 'Show less' : '... Show more'}
						</button>
					{/if}
					<div class="mt-4 flex flex-wrap gap-2">
						{#each book.genres as g (g)}
							<span class="inline-flex h-8 items-center rounded-full bg-bg-tinted px-3 text-sm text-white">{g}</span>
						{/each}
						{#if book.language}<span class="inline-flex h-8 items-center rounded-full bg-bg-tinted px-3 text-sm text-white">{book.language}</span>{/if}
						<span class="inline-flex h-8 items-center rounded-full bg-bg-tinted px-3 text-sm text-white">Public domain • LibriVox</span>
					</div>
				</section>
			{/if}

			<section>
				<h2 class="mb-2 font-title text-xl font-bold text-white">Chapters</h2>
				{#if data.chapters.length}
					<div>
						{#each data.chapters as ch, i (ch.id)}
							<ChapterRow ep={ch} n={i + 1} onPlay={() => (isBookPlaying(data) && playback.now?.videoId === ch.id ? api.togglePause() : playBook(data!, i))} />
						{/each}
					</div>
				{:else}
					<EmptyState icon={BookOpenIcon} size="sm" title="No chapters" body="This recording has no chapters to play." />
				{/if}
			</section>
		</div>
	</ActionBar>
{/if}
