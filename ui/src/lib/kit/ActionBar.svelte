<script lang="ts">
	/**
	 * Entity action bar (spec: playlist.md §2, album.md, artist.md §2, liked-songs.md).
	 * Left → right, gap ≈ gutter: 56px accent Play · Shuffle · Save/like · `extra` · More (…). Right: `right`.
	 *
	 * Props
	 *   playing?: boolean            pause glyph when this context is playing
	 *   onPlay?: () => void          omit to hide the play button (an empty playlist, as on Spotify)
	 *   playLabel?: string           tooltip/aria, e.g. `Play ${title}`
	 *   shuffle?: boolean            omit (undefined) to hide; true = accent + dot
	 *   onShuffle?: () => void
 *   smartShuffle?: boolean       shuffle shows Spotify's Smart Shuffle glyph (shuffle + sparkle)
 *   shuffleLabel?: string        tooltip override (e.g. "Enable Smart Shuffle")
	 *   saved?: boolean              omit to hide; PlusCircle (32px) ↔ accent CheckCircle fill
	 *   onSave?: () => void
	 *   saveLabel?: string           default "Save to Your Library" / "Remove from Your Library"
	 *   onMore?: (e: MouseEvent) => void   omit to hide "…"; open your menu anchored at e.currentTarget
	 *   moreLabel?: string           default "More options"
	 *   download?: DownloadCollection   Spotify's download toggle (after Save, before More)
	 *   downloadSongs?: SongItem[]   only for fixed items (an episode); lists are read by Rust
	 *   extra?: Snippet              e.g. Follow outline pill (artist)
	 *   right?: Snippet              e.g. view toggle "List ☰"
	 *   tint?: string                header tint ('rgb(r g b)' / hex); draws the rgba(tint,.7)→transparent
	 *                                232px gradient behind the bar and the start of `children`
	 *   sentinel?: HTMLElement       bindable: the play button's wrapper, for <StickyHeader sentinel>
	 *   children?: Snippet           content under the bar that sits on the gradient (track table…)
	 *
	 * Usage
	 *   <ActionBar {playing} onPlay={play} shuffle={on} onShuffle={toggle} saved={s} onSave={save}
	 *     onMore={(e) => openItemMenu(e.currentTarget, item)} tint={tint.color} bind:sentinel>
	 *     <TrackTable … />
	 *   </ActionBar>
	 */
	import type { Snippet } from 'svelte';
	import PlayButton from './PlayButton.svelte';
	import IconButton from './IconButton.svelte';
	import ShuffleIcon from 'phosphor-svelte/lib/ShuffleIcon';
	import SmartShuffleIcon from '$lib/smart/SmartShuffleIcon.svelte';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import { channels } from './tint.svelte';
	import type { DownloadCollection, SongItem } from '$lib/api';
	import DownloadButton from '$lib/downloads/DownloadButton.svelte';

	let {
		playing = false,
		onPlay,
		playLabel,
		shuffle,
		onShuffle,
		smartShuffle = false,
		shuffleLabel,
		saved,
		onSave,
		saveLabel,
		download,
		downloadSongs,
		onMore,
		moreLabel = 'More options',
		extra,
		right,
		tint,
		sentinel = $bindable(),
		children
	}: {
		playing?: boolean;
		onPlay?: () => void;
		playLabel?: string;
		shuffle?: boolean;
		onShuffle?: () => void;
		smartShuffle?: boolean;
		shuffleLabel?: string;
		saved?: boolean;
		onSave?: () => void;
		saveLabel?: string;
		download?: DownloadCollection;
		downloadSongs?: SongItem[];
		onMore?: (e: MouseEvent) => void;
		moreLabel?: string;
		extra?: Snippet;
		right?: Snippet;
		tint?: string | null;
		sentinel?: HTMLElement;
		children?: Snippet;
	} = $props();
</script>

<div class="relative">
	{#if tint}
		<div
			class="pointer-events-none absolute inset-x-0 top-0 h-[232px]"
			style="background:linear-gradient(rgb({channels(tint)} / 0.7), transparent)"
		></div>
		<div class="noise pointer-events-none absolute inset-x-0 top-0 h-[232px]"></div>
	{/if}
	<div
		class="relative flex items-center gap-5 p-[var(--content-spacing)]"
	>
		<!-- Without a play button the sentinel stays where it would be (out of the flow), so the
		     sticky header still appears at the same scroll point. -->
		<div bind:this={sentinel} class={onPlay ? 'shrink-0' : 'pointer-events-none absolute h-14 w-0'}>
			{#if onPlay}
				<PlayButton size="lg" {playing} label={playLabel} onclick={() => onPlay()} />
			{/if}
		</div>
		{#if shuffle !== undefined}
			<IconButton
				icon={smartShuffle ? SmartShuffleIcon : ShuffleIcon}
				label={shuffleLabel ?? (shuffle ? 'Disable shuffle' : 'Enable shuffle')}
				size="lg"
				class="!size-8"
				active={shuffle}
				dot
				onclick={() => onShuffle?.()}
			/>
		{/if}
		{#if saved !== undefined}
			<IconButton
				icon={saved ? CheckCircleIcon : PlusCircleIcon}
				fill={saved}
				label={saveLabel ?? (saved ? 'Remove from Your Library' : 'Save to Your Library')}
				size="lg"
				class="!size-8"
				active={saved}
				onclick={() => onSave?.()}
			/>
		{/if}
		{#if download}<DownloadButton target={download} songs={downloadSongs} size="md" iconSize={32} />{/if}
		{#if extra}{@render extra()}{/if}
		{#if onMore}
			<IconButton
				icon={DotsThreeIcon}
				label={moreLabel}
				size="lg"
				class="!size-8"
				onclick={(e) => onMore(e)}
			/>
		{/if}
		{#if right}
			<div class="ml-auto flex items-center">{@render right()}</div>
		{/if}
	</div>
	{#if children}
		<div class="relative">{@render children()}</div>
	{/if}
</div>
