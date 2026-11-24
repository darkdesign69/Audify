<script lang="ts">
	import { creatorOf } from '$lib/discover/format';
	// Recently played (Spotify: Home ▸ "Recently played" ▸ Show all): the contexts you played from
	// as a card grid, then the songs themselves by day. All local (`recent_play_contexts`,
	// `lib_recent_tracks`): works signed out, offline, and skips private sessions by construction.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api';
	import type { RecentPlayContext, RecentTrack, SongItem } from '$lib/api';
	import { Card, EmptyState, ErrorState, Skeleton, TrackTable, actions, openItemMenu } from '$lib/kit';
	import ClockCounterClockwiseIcon from 'phosphor-svelte/lib/ClockCounterClockwiseIcon';
	import { lib } from '$lib/library.svelte';
	import { playItem } from '$lib/browse';
	import { openPlayer, startRadio } from '$lib/player.svelte';
	import CardRow from '$lib/artistpods/CardRow.svelte';
	import { LIKED_COVER, isPlayingContext, playOrToggle } from '$lib/artistpods/playing';

	let contexts = $state.raw<RecentPlayContext[]>([]);
	let tracks = $state.raw<RecentTrack[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let showAllCards = $state(false);

	async function load() {
		loading = !contexts.length && !tracks.length;
		error = null;
		try {
			const [c, t] = await Promise.all([api.recentPlayContexts(50), lib.recentTracks(200)]);
			contexts = c;
			tracks = t;
		} catch (e) {
			error = String(e);
		} finally {
			loading = false;
		}
	}
	onMount(load);

	// ── Contexts → cards ─────────────────────────────────────────────────────────────────────
	const subtitleFor = (c: RecentPlayContext) =>
		c.kind === 'liked'
			? 'Playlist • You'
			: c.kind === 'artist'
				? 'Artist'
				: c.kind === 'podcast'
					? `Podcast${c.subtitle ? ` • ${c.subtitle}` : ''}`
					: c.kind === 'playlist'
						? // YouTube's own subtitle ("722K views • 50 tracks • 3 hours, …") keeps only its creator.
							`Playlist${creatorOf(c.subtitle) ? ` • ${creatorOf(c.subtitle)}` : ''}`
						: c.subtitle || (c.kind === 'album' ? 'Album' : c.kind === 'radio' || c.kind === 'mix' ? 'Radio' : 'Playlist');

	function hrefFor(c: RecentPlayContext): string | undefined {
		switch (c.kind) {
			case 'liked':
				return `/playlist/${api.LIKED_ID}`;
			case 'podcast':
				return `/show/${encodeURIComponent(c.id)}`;
			case 'audiobook': // stream: audiobooks
				return `/audiobook/${encodeURIComponent(c.id)}`;
			case 'album':
			case 'artist':
				return actions.hrefFor({ kind: c.kind, id: c.id });
			case 'playlist':
				return c.id === 'EPISODES' ? '/collection/episodes' : actions.playlistHref(c.id);
			default:
				return undefined; // radios and mixes have no page: the card plays them
		}
	}

	function playContext(c: RecentPlayContext) {
		if (c.kind === 'radio') return startRadio((c.params ?? 'song') as 'song' | 'artist' | 'album' | 'playlist', c.id, c.title);
		if (c.kind === 'mix') return startRadio('artist', c.id, c.title);
		const href = hrefFor(c);
		if (c.kind === 'podcast' || c.kind === 'audiobook' || c.id === 'EPISODES') return href && goto(href);
		const kind = c.kind === 'liked' || c.kind === 'daily_mix' ? 'playlist' : c.kind;
		return playOrToggle([c.id], () => playItem({ kind, id: c.kind === 'liked' ? api.LIKED_ID : c.id, title: c.title, thumbnail: c.thumbnail }));
	}

	const asItem = (c: RecentPlayContext) => ({
		kind: (c.kind === 'album' || c.kind === 'artist' ? c.kind : 'playlist') as 'album' | 'artist' | 'playlist',
		id: c.kind === 'liked' ? api.LIKED_ID : c.id,
		title: c.title,
		subtitle: c.subtitle,
		thumbnail: c.thumbnail
	});

	// ── Songs by day ─────────────────────────────────────────────────────────────────────────
	const DAY = new Intl.DateTimeFormat('en-US', { weekday: 'long', day: 'numeric', month: 'long' });
	function dayLabel(unix: number): string {
		const d = new Date(unix * 1000);
		const today = new Date();
		today.setHours(0, 0, 0, 0);
		const day = new Date(d);
		day.setHours(0, 0, 0, 0);
		const diff = Math.round((today.getTime() - day.getTime()) / 86_400_000);
		if (diff === 0) return 'Today';
		if (diff === 1) return 'Yesterday';
		return DAY.format(d) + (d.getFullYear() !== today.getFullYear() ? `, ${d.getFullYear()}` : '');
	}
	const groups = $derived.by(() => {
		const out: { label: string; songs: SongItem[] }[] = [];
		for (const t of tracks) {
			const label = dayLabel(t.playedAt);
			const last = out[out.length - 1];
			if (last?.label === label) last.songs.push(t.song);
			else out.push({ label, songs: [t.song] });
		}
		return out;
	});
	const allSongs = $derived(tracks.map((t) => t.song));
	// The whole history from the clicked song, as one queue with no library context (nothing to
	// reopen, so no recents entry and no playlist radio).
	function playAll(start: number) {
		if (!allSongs.length) return;
		openPlayer();
		api.playPlaylist(allSongs, Math.max(0, start), undefined, 'Recently played');
	}
</script>

<svelte:head><title>Recently played</title></svelte:head>

<!-- Spotify's "Recently played" page (measured): heading at y 138, the first card row at 208. -->
<div class="flex flex-col gap-10 px-[var(--content-spacing)] pt-[74px] pb-10">
	<h1 class="-mb-[9px] font-title text-2xl leading-normal font-bold text-white">Recently played</h1>

	{#if loading}
		<div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4"><Skeleton variant="card" count={6} /></div>
		<Skeleton variant="row" count={6} />
	{:else if error}
		<ErrorState onRetry={load} />
	{:else if !contexts.length && !tracks.length}
		<div class="py-16">
			<EmptyState
				icon={ClockCounterClockwiseIcon}
				title="Nothing played yet"
				body="Albums, playlists, artists and songs you play will show up here."
				actionLabel="Find something to play"
				onAction={() => goto('/search')}
			/>
		</div>
	{:else}
		{#if contexts.length}
			<section class="flex flex-col gap-2">
				<CardRow items={contexts} rows={showAllCards ? Infinity : 2} key={(c) => `${c.kind}:${c.id}`}>
					{#snippet card(c)}
						<Card
							title={c.title}
							subtitle={subtitleFor(c)}
							image={c.kind === 'liked' ? LIKED_COVER : c.thumbnail}
							kind={c.kind === 'artist' ? 'artist' : 'playlist'}
							href={hrefFor(c)}
							onclick={hrefFor(c) ? undefined : () => playContext(c)}
							playing={isPlayingContext(c.id)}
							onPlay={() => playContext(c)}
							oncontextmenu={c.kind === 'radio' || c.kind === 'mix' || c.kind === 'podcast' ? undefined : (e) => openItemMenu(e, asItem(c))}
						/>
					{/snippet}
				</CardRow>
				{#if contexts.length > 12 && !showAllCards}
					<button type="button" class="self-start text-sm font-bold text-text-subdued hover:text-white hover:underline" onclick={() => (showAllCards = true)}>Show all</button>
				{/if}
			</section>
		{/if}

		{#if groups.length}
			<section class="flex flex-col gap-6">
				<h2 class="font-title text-xl font-bold text-white">Songs</h2>
				{#each groups as g (g.label)}
					<div>
						<h3 class="mb-2 text-base font-bold text-white">{g.label}</h3>
						<TrackTable
							tracks={g.songs}
							variant="queue"
							showHeader={false}
							onPlay={(list, pos) => playAll(allSongs.indexOf(list[pos]))}
						/>
					</div>
				{/each}
				{#if allSongs.length >= 200}
					<p class="text-sm text-text-subdued">Showing your last 200 songs.</p>
				{/if}
			</section>
		{/if}
	{/if}
</div>
