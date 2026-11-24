<script lang="ts">
	// DEV-ONLY kit gallery (pnpm dev → /dev-kit): renders the entity-page pattern with fixtures so
	// the kit can be compared against docs/spotify-spec/ref. Not linked from the app.
	import { onMount } from 'svelte';
	import * as api from '$lib/api';
	import type { SongItem } from '$lib/api';
	import { SvelteSet } from 'svelte/reactivity';
	import {
		ActionBar,
		Card,
		PageHeader,
		Shelf,
		ShortcutTile,
		StickyHeader,
		TrackTable,
		openItemMenu,
		type Tint
	} from '$lib/kit';

	let tracks = $state<SongItem[]>([]);
	let thumbnail = $state<string | undefined>();
	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();
	let selected = $state(new SvelteSet<number>());
	let shuffle = $state(false);
	onMount(async () => {
		const p = await api.getPlaylist('LOCALPLAYLIST:1');
		tracks = p.items;
		thumbnail = p.items[0]?.thumbnail;
	});
	const item = { kind: 'playlist' as const, id: 'LOCALPLAYLIST:1', title: 'No skip songs' };
</script>

<StickyHeader title="No skip songs" tint={tint?.color} {sentinel} onPlay={() => {}} />
<PageHeader type="Playlist" title="No skip songs" image={thumbnail} bind:tintOut={tint} owner={{ name: 'Spotless' }} details="30 songs, 1 hr 32 min" />
<ActionBar bind:sentinel tint={tint?.rgb} onPlay={() => {}} {shuffle} onShuffle={() => (shuffle = !shuffle)} onMore={(e) => openItemMenu(e, item)}>
	<div class="px-[var(--content-spacing)]">
		<TrackTable {tracks} variant="playlist" context={{ id: item.id, title: item.title }} menuContext={{ playlistId: item.id }} stickyTop={64} bleed="var(--content-spacing)" bind:selected />
	</div>
</ActionBar>
<div class="px-[var(--content-spacing)] py-8">
	<div class="mb-6 grid grid-cols-4 gap-2">
		{#each tracks.slice(0, 8) as t (t.video_id)}
			<ShortcutTile title={t.title} image={t.thumbnail} onPlay={() => {}} />
		{/each}
	</div>
	<Shelf title="Made for you" items={tracks.slice(0, 10)} showAll>
		{#snippet card(t: SongItem)}
			<Card title={t.title} subtitle={t.artists} image={t.thumbnail} onPlay={() => {}} />
		{/snippet}
	</Shelf>
</div>
