<script lang="ts">
	/**
	 * A release on the What's New page (stream smart2), in Spotify's layout (measured on
	 * open.spotify.com/content-feed): a 176x99 (16:9) thumbnail with the square art centred,
	 * title 16/400, artists 14 white, "Single · 2 weeks ago", then ⊕ bottom-left under the text
	 * and the white 32px play bottom-right. Rows are ~200px. A blue dot marks it new.
	 */
	import type { WhatsNewRelease } from '$lib/api';
	import { goto } from '$app/navigation';
	import { thumb } from '$lib/thumb';
	import { IconButton, PlayButton, actions } from '$lib/kit';
	import { isPlaying, playBrowse, toggle } from '$lib/discover/play.svelte';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import { releaseWhen } from './whatsnew';

	let { release }: { release: WhatsNewRelease } = $props();
	const href = $derived(actions.albumHref(release.id));
	const card = $derived({
		kind: 'album' as const,
		id: release.id,
		title: release.title,
		subtitle: release.artist,
		thumbnail: release.thumbnail ?? undefined
	});
	const saved = $derived(actions.isInLibrary(card));
	const playing = $derived(isPlaying(release.id));
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group flex cursor-pointer gap-6 rounded-md px-3 py-4 hover:bg-bg-tinted"
	onclick={(e) => {
		if ((e.target as HTMLElement).closest('button,a')) return;
		goto(href);
	}}
>
	<!-- Spotify: a 176x99 (16:9) box, the square art centred on a grey fade. -->
	<div
		class="relative flex h-[99px] w-[176px] shrink-0 justify-center overflow-hidden rounded-sm"
		style="background-image:linear-gradient(#535353,#121212)"
	>
		<img
			src={thumb(release.thumbnail, 240)}
			alt=""
			loading="lazy"
			class="size-[99px] bg-bg-menu object-cover"
		/>
	</div>
	<div class="flex min-h-[167px] min-w-0 flex-1 flex-col">
		<a {href} class="line-clamp-2 text-base leading-[22px] text-white hover:underline">{release.title}</a>
		<a
			href={actions.artistHref(release.artistId)}
			class="mt-1 truncate text-sm leading-[19px] text-white hover:underline">{release.artist}</a
		>
		<div class="mt-2 flex items-center gap-2 leading-[22px]">
			{#if release.isNew}
				<span class="size-2 shrink-0 rounded-full bg-announcement" role="img" aria-label="New"></span>
			{/if}
			<span class="truncate text-base text-white"
				>{release.releaseType}<span class="mx-2 text-sm">·</span><span class="text-sm"
					>{releaseWhen(release)}</span
				></span
			>
		</div>
		<div class="mt-auto flex h-12 items-center justify-between">
			<IconButton
				icon={saved ? CheckCircleIcon : PlusCircleIcon}
				fill={saved}
				active={saved}
				label={saved ? 'Remove from Your Library' : 'Save to Your Library'}
				size="sm"
				iconSize={24}
				onclick={() => actions.toggleInLibrary(card)}
			/>
			<PlayButton
				size="sm"
				tone="white"
				{playing}
				label={playing ? `Pause ${release.title}` : `Play ${release.title}`}
				onclick={() => toggle(release.id, () => playBrowse(card))}
			/>
		</div>
	</div>
</div>
