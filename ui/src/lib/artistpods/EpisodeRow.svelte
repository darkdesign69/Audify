<script lang="ts">
	/**
	 * Spotify's episode row (spec podcasts.md "Episode row"): show page, Your Episodes.
	 * Full width, padding 16px 12px, radius 6, hover white/10; 1px white/10 separator above.
	 * Thumb 112 square (176x99 for video podcasts), title 16 white (→ episode page), optional show
	 * name, 2-line description, meta "date • duration | N min left" + 85x4 progress bar, bottom row
	 * of 32px icons (Your Episodes, share, …) and the white 32px play circle at the right.
	 */
	import type { PodcastEpisode } from '$lib/api';
	import { episodeSong } from '$lib/api'; // stream: downloads
	import { Icon, IconButton, Tooltip } from '$lib/kit';
	import PlayIcon from 'phosphor-svelte/lib/PlayIcon';
	import PauseIcon from 'phosphor-svelte/lib/PauseIcon';
	import PlusCircleIcon from 'phosphor-svelte/lib/PlusCircleIcon';
	import DownloadButton from '$lib/downloads/DownloadButton.svelte'; // stream: downloads
	import { isDownloaded } from '$lib/downloads.svelte';
	import ArrowCircleDownIcon from 'phosphor-svelte/lib/ArrowCircleDownIcon';
	import CheckCircleIcon from 'phosphor-svelte/lib/CheckCircleIcon';
	import ExportIcon from 'phosphor-svelte/lib/ExportIcon';
	import DotsThreeIcon from 'phosphor-svelte/lib/DotsThreeIcon';
	import MicrophoneIcon from 'phosphor-svelte/lib/MicrophoneIcon';
	import { isSaved, progressOf, remainingSecs } from '$lib/podcasts.svelte';
	import { thumb } from '$lib/thumb';
	import {
		episodeDate,
		episodeHref,
		episodeShareUrl,
		isEpisodePlaying,
		isEpisodeCurrent,
		openEpisodeMenu,
		plainText,
		shareEpisode,
		spokenDuration,
		toggleSavedToast
	} from './pods';

	let {
		ep,
		showName = false,
		onPlay
	}: {
		ep: PodcastEpisode;
		/** true = "Episode • <show>" (Your Episodes); 'plain' = the show name only (show page, Spotify). */
		showName?: boolean | 'plain';
		onPlay: () => void;
	} = $props();

	const p = $derived(progressOf(ep));
	const left = $derived(remainingSecs(ep));
	const duration = $derived(p.durationSecs || ep.durationSecs || 0);
	const saved = $derived(isSaved(ep.id));
	const playing = $derived(isEpisodePlaying(ep.id));
	const current = $derived(isEpisodeCurrent(ep.id));
	const desc = $derived(plainText(ep.description));
	const wide = $derived(ep.isVideo);
	const href = $derived(episodeHref(ep.id));
	// Spotify's blue "new episode" dot: unplayed, unstarted and published in the last week.
	const fresh = $derived(
		!p.played && left === null && !!ep.publishedAt && Date.now() / 1000 - ep.publishedAt < 7 * 86_400
	);
	let failed = $state(false);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group/ep relative border-t border-white/10 first:border-t-0 [&:hover]:border-transparent [&:hover+*]:border-transparent"
	oncontextmenu={(e) => openEpisodeMenu(e, ep)}
>
	<div
		class="flex gap-x-6 gap-y-2 rounded-md px-3 py-4 transition-colors duration-100 hover:bg-bg-tinted"
		ondblclick={(e) => !(e.target as HTMLElement).closest('a,button') && onPlay()}
	>
		<a {href} class="shrink-0 self-start" tabindex="-1" aria-hidden="true">
			<div
				class="overflow-hidden rounded-lg bg-bg-menu shadow-[0_4px_28px_rgba(0,0,0,.8)]"
				style={wide ? 'width:176px;height:99px' : 'width:112px;height:112px'}
			>
				{#if ep.thumbnail && !failed}
					<img
						src={thumb(ep.thumbnail, 240)}
						alt=""
						loading="lazy"
						draggable="false"
						class="size-full object-cover"
						onerror={() => (failed = true)}
					/>
				{:else}
					<div class="grid size-full place-items-center text-text-subdued"><Icon icon={MicrophoneIcon} size={40} /></div>
				{/if}
			</div>
		</a>
		<div class="flex min-w-0 flex-1 flex-col gap-2">
			<div class="min-w-0">
				<!-- Spotify: 16/400 title, the blue "new" dot inline in front of it -->
				<a {href} class="line-clamp-2 text-base leading-[22px] hover:underline {current ? 'text-accent-text' : 'text-white'}"
					>{#if fresh}<span class="mr-2 mb-px inline-block size-2 rounded-full bg-[#4cb3ff] align-middle" role="img" aria-label="New episode"></span>{/if}{ep.title}</a
				>
				{#if showName === 'plain'}
					<div class="mt-1 truncate text-sm text-text-subdued">
						{#if ep.showId}
							<a href={`/show/${encodeURIComponent(ep.showId)}`} class="hover:text-white hover:underline">{ep.showTitle}</a>
						{:else}{ep.showTitle}{/if}
					</div>
				{:else if showName}
					<div class="mt-1 truncate text-sm text-text-subdued">
						Episode<span aria-hidden="true">{' • '}</span>{#if ep.showId}
							<a href={`/show/${encodeURIComponent(ep.showId)}`} class="hover:text-white hover:underline">{ep.showTitle}</a>
						{:else}{ep.showTitle}{/if}
					</div>
				{/if}
			</div>
			{#if desc}
				<p class="line-clamp-2 max-w-[672px] text-sm text-text-subdued">{desc}</p>
			{/if}
			<div class="flex min-w-0 items-center gap-2 text-sm text-text-subdued">
				{#if isDownloaded(ep.id)}<span class="grid shrink-0 place-items-center text-accent" aria-label="Downloaded"><Icon icon={ArrowCircleDownIcon} size={14} fill /></span>{/if}
				{#if p.played}
					<Tooltip text="Played">
						{#snippet trigger(props)}
							<span {...props} class="shrink-0 text-accent"><Icon icon={CheckCircleIcon} size={16} fill /></span>
						{/snippet}
					</Tooltip>
				{/if}
				<span class="truncate">
					{episodeDate(ep.publishedAt)}{#if duration || left !== null}<span aria-hidden="true">{' • '}</span>{/if}{#if left !== null}{spokenDuration(left)} left{:else if duration}{spokenDuration(duration)}{/if}
				</span>
				{#if left !== null}
					<div class="h-1 w-[85px] shrink-0 overflow-hidden rounded-sm bg-white/30" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(p.fraction * 100)} aria-label={`${Math.round(p.fraction * 100)}% played`}>
						<div class="h-full rounded-sm bg-white" style="width:{Math.min(100, p.fraction * 100)}%"></div>
					</div>
				{/if}
			</div>
			<div class="flex items-center gap-2">
				<IconButton
					icon={saved ? CheckCircleIcon : PlusCircleIcon}
					fill={saved}
					active={saved}
					iconSize={24}
					label={saved ? 'Remove from Your Episodes' : 'Add to Your Episodes'}
					onclick={() => toggleSavedToast(ep)}
				/>
				<DownloadButton size="md" iconSize={24} target={{ id: ep.id, kind: 'episode', title: ep.title, subtitle: ep.showTitle ?? null, thumbnail: ep.thumbnail ?? null }} songs={[episodeSong(ep)]} />
				<!-- Secondary actions show on row hover only (Spotify); kept visible while focused or its menu is open. -->
				<div class="flex items-center gap-2 opacity-0 group-hover/ep:opacity-100 focus-within:opacity-100 has-[[data-menu-open]]:opacity-100">
					{#if episodeShareUrl(ep)}
						<IconButton icon={ExportIcon} iconSize={24} label="Share" onclick={() => shareEpisode(ep)} />
					{/if}
					<IconButton icon={DotsThreeIcon} iconSize={24} label={`More options for ${ep.title}`} onclick={(e) => openEpisodeMenu(e, ep)} />
				</div>
				<button
					type="button"
					class="ml-auto grid size-8 shrink-0 place-items-center rounded-full bg-white text-black transition-transform duration-50 hover:scale-[1.04]"
					aria-label={playing ? 'Pause' : `Play ${ep.title}`}
					onclick={() => onPlay()}
				>
					<Icon icon={playing ? PauseIcon : PlayIcon} size={16} fill />
				</button>
			</div>
		</div>
	</div>
</div>
