<script lang="ts">
	/**
	 * A row of the search "All" mixed result list (spec search-and-browse.md §4.4): 64px, 48px
	 * image (round for artists), title 16 white + subtitle 14 #b3b3b3 (▶ badge for videos), a grey
	 * type badge ("Song" / "Artist" / "Album" / "Playlist") in the middle column, and ⊕ (Follow for
	 * artists) at the right. Hover = white/10 plate + play glyph over the image. Songs play on
	 * double-click or the image's play glyph; other kinds open on click.
	 */
	import * as api from '$lib/api';
	import type { BrowseItem, SongItem } from '$lib/api';
	import { Icon, IconButton, Pill, actions, openItemMenu, openTrackMenu } from '$lib/kit';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import MusicNotesIcon from 'phosphor-svelte/lib/MusicNotesIcon';
	import { thumb } from '$lib/thumb';
	import { playback } from '$lib/player.svelte';
	import { creatorOf } from './format';
	import { exploreHref, isPlaying, openExplore, playBrowse, toggle } from './play.svelte';

	let {
		row,
		onPlaySong,
		onOpen
	}: {
		row: { kind: 'song'; song: SongItem } | { kind: 'item'; item: BrowseItem };
		/** Play this song in the result list's context. */
		onPlaySong?: () => void;
		onOpen?: () => void;
	} = $props();

	const song = $derived(row.kind === 'song' ? row.song : null);
	const item = $derived(row.kind === 'item' ? row.item : null);
	const title = $derived(song ? song.title : (item?.title ?? ''));
	const image = $derived(song ? song.thumbnail : item?.thumbnail);
	const round = $derived(item?.kind === 'artist');
	const isVideo = $derived(!!song?.is_video);
	const subtitle = $derived(song ? song.artists : item?.kind === 'artist' ? '' : creatorOf(item?.subtitle));
	const badge = $derived(
		song
			? 'Song'
			: ({ song: 'Song', artist: 'Artist', album: 'Album', playlist: 'Playlist', podcast: 'Podcast' } as Record<string, string>)[
					item!.kind
				] ?? ''
	);
	const current = $derived(song ? playback.now?.videoId === song.video_id : !!item && isPlaying(item.id));
	const playing = $derived(current && !playback.paused);
	const href = $derived(item ? exploreHref(item) : undefined);
	const liked = $derived(song ? actions.isLiked(song) : false);
	const saved = $derived(item ? actions.isInLibrary(item) : false);

	function play() {
		if (song) {
			if (current) toggleCurrent();
			else onPlaySong?.();
		} else if (item) toggle(item.id, () => playBrowse(item));
	}
	function toggleCurrent() {
		void api.togglePause();
	}
	function open(e: MouseEvent) {
		if ((e.target as HTMLElement).closest('a,button')) return;
		if (item) {
			onOpen?.();
			openExplore(item);
		}
	}
	function menu(e: MouseEvent) {
		e.preventDefault();
		if (song) openTrackMenu(e, [song]);
		else if (item) openItemMenu(e, item);
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group grid h-16 grid-cols-[minmax(0,1fr)_minmax(0,250px)] items-center gap-3 rounded-sm px-3 hover:bg-bg-tinted has-[[data-menu-open]]:bg-bg-tinted {item
		? 'cursor-pointer'
		: ''}"
	onclick={open}
	ondblclick={(e) => song && !(e.target as HTMLElement).closest('a,button') && play()}
	oncontextmenu={menu}
>
	<div class="flex min-w-0 items-center gap-3">
		<!-- Spotify (2026): a 16:9 85x48 box, the square art centred on a soft blurred copy of itself. -->
		<button
			type="button"
			class="relative grid h-12 w-[85px] shrink-0 place-items-center overflow-hidden rounded-[4px] bg-bg-base"
			aria-label={playing ? 'Pause' : `Play ${title}`}
			onclick={play}
		>
			{#if image}
				{#if !round}
					<img src={thumb(image, 120)} alt="" aria-hidden="true" loading="lazy" draggable="false" class="absolute inset-0 size-full scale-125 object-cover opacity-25 blur-lg" />
				{/if}
				<img src={thumb(image, 120)} alt="" loading="lazy" draggable="false" class="relative size-12 object-cover {round ? 'rounded-full' : ''}" />
			{:else}
				<span class="relative grid size-12 place-items-center bg-bg-menu text-text-subdued {round ? 'rounded-full' : ''}"><Icon icon={MusicNotesIcon} size={20} /></span>
			{/if}
			<span
				class="absolute inset-0 grid place-items-center bg-black/50 text-white {playing
					? 'opacity-100'
					: 'opacity-0 group-hover:opacity-100'}"
			>
				<Icon icon={playing ? PauseIcon : PlayIcon} size={20} fill />
			</span>
		</button>
		<div class="min-w-0">
			{#if href}
				<a {href} class="block truncate text-base {current ? 'text-accent-text' : 'text-white'} hover:underline" onclick={() => onOpen?.()}>{title}</a>
			{:else}
				<span class="block truncate text-base {current ? 'text-accent-text' : 'text-white'}">{title}</span>
			{/if}
			{#if subtitle || isVideo}
				<div class="flex min-w-0 items-center gap-1 text-sm text-text-subdued">
					{#if isVideo}
						<span class="grid h-3.5 w-4 shrink-0 place-items-center rounded-[2px] border border-current" aria-label="Video">
							<Icon icon={PlayIcon} size={8} fill />
						</span>
					{/if}
					<span class="truncate group-hover:text-white">{subtitle}</span>
				</div>
			{/if}
		</div>
	</div>
	<div class="flex min-w-0 items-center justify-between">
		<span class="inline-flex h-4 items-center rounded-sm bg-bg-tinted px-1 text-xs font-bold text-text-subdued">{badge}</span>
		{#if item?.kind === 'artist'}
			<Pill variant="outline" onclick={() => actions.toggleInLibrary(item!)}>{saved ? 'Following' : 'Follow'}</Pill>
		{:else if song}
			<IconButton
				icon={liked ? CheckCircleIcon : PlusCircleIcon}
				fill={liked}
				active={liked}
				label={liked ? 'Remove from Liked Songs' : 'Add to Liked Songs'}
				onclick={() => actions.toggleLike(song!)}
			/>
		{:else if item}
			<IconButton
				icon={saved ? CheckCircleIcon : PlusCircleIcon}
				fill={saved}
				active={saved}
				label={saved ? 'Remove from Your Library' : 'Save to Your Library'}
				onclick={() => actions.toggleInLibrary(item!)}
			/>
		{/if}
	</div>
</div>
