<script lang="ts">
	/**
	 * 16:9 card for "Music videos" (spec artist.md §3.5): same card chrome as the kit Card
	 * (padding 12, radius 6, #1f1f1f hover, accent play rising on hover), wide image.
	 */
	import { PlayButton, Icon } from '$lib/kit';
	import VideoIcon from 'phosphor-svelte/lib/VideoIcon';

	let {
		title,
		subtitle,
		image,
		playing = false,
		onPlay,
		oncontextmenu
	}: {
		title: string;
		subtitle?: string | null;
		image?: string | null;
		playing?: boolean;
		onPlay: () => void;
		oncontextmenu?: (e: MouseEvent) => void;
	} = $props();
	let loaded = $state(false);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class="group relative flex min-w-0 cursor-pointer flex-col gap-2 rounded-md p-3 transition-colors duration-200 ease-productive hover:bg-bg-highlight active:bg-black"
	onclick={() => onPlay()}
	oncontextmenu={oncontextmenu
		? (e) => {
				e.preventDefault();
				oncontextmenu(e);
			}
		: undefined}
>
	<div class="relative aspect-video w-full">
		<div class="size-full overflow-hidden rounded-md bg-bg-menu shadow-card-image">
			{#if image}
				<img
					src={image}
					alt=""
					loading="lazy"
					draggable="false"
					class="size-full object-cover transition-opacity duration-300 {loaded ? 'opacity-100' : 'opacity-0'}"
					onload={() => (loaded = true)}
				/>
			{:else}
				<div class="grid size-full place-items-center text-text-subdued"><Icon icon={VideoIcon} size={48} /></div>
			{/if}
		</div>
		<div
			class="absolute right-2 bottom-2 rounded-full shadow-play transition-[opacity,transform] duration-200 ease-out {playing
				? 'translate-y-0 opacity-100'
				: 'translate-y-2 opacity-0 group-hover:translate-y-0 group-hover:opacity-100'}"
		>
			<PlayButton size="md" {playing} label={`Play ${title}`} onclick={() => onPlay()} />
		</div>
	</div>
	<div class="min-w-0">
		<span class="line-clamp-1 text-base leading-[22px] text-white" {title}>{title}</span>
		{#if subtitle}<p class="mt-0.5 line-clamp-1 text-sm leading-[19px] text-text-subdued">{subtitle}</p>{/if}
	</div>
</div>
