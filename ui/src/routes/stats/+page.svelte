<script lang="ts" module>
	import type { Range } from '$lib/stats';
	// Kept for the session: whoever checks the month comes back to the month.
	let lastRange: Range = 'month';
</script>

<script lang="ts">
	// Listening stats (#396), in Spotify's profile-page language: a header, range chips, "Top
	// artists" as round cards, "Top tracks" as a track table with play counts, top albums, then the
	// activity charts. Everything is counted on this device (`listening_stats` in Rust); the only
	// network is the top artists' photos, through the artist-page cache. Reached from the account
	// menu ("Listening stats").
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '$lib/api';
	import type { ListeningStats, TopArtist } from '$lib/api';
	import {
		ActionBar,
		Card,
		EmptyState,
		ErrorState,
		Icon,
		PageHeader,
		Pill,
		Skeleton,
		StickyHeader,
		TrackTable,
		actions,
		confirmDialog,
		openItemMenu,
		toast,
		type Tint
	} from '$lib/kit';
	import ChartBarIcon from 'phosphor-svelte/lib/ChartBarIcon';
	import ListeningChart from '$lib/components/ListeningChart.svelte';
	import ListeningHours from '$lib/components/ListeningHours.svelte';
	import ListeningDayparts from '$lib/components/ListeningDayparts.svelte';
	import { artistPage, playItem } from '$lib/browse';
	import { openPlayer, playback } from '$lib/player.svelte';
	import { RANGES, bars as toBars, busiest, change, listened, rangeStart, tagOf, type Bar } from '$lib/stats';
	import { currentLocale } from '$lib/i18n.svelte';
	import CardRow from '$lib/artistpods/CardRow.svelte';
	import { isPlayingContext, playOrToggle } from '$lib/artistpods/playing';

	const LABEL: Record<Range, string> = { week: 'Last 7 days', month: 'Last 30 days', year: 'Last 12 months', all: 'All time' };
	const PHRASE: Record<Range, string> = { week: 'this week', month: 'this month', year: 'this year', all: 'of all time' };

	let range = $state<Range>(lastRange);
	let stats = $state.raw<ListeningStats | null>(null);
	let shown = $state<Range>(lastRange);
	let error = $state<string | null>(null);
	let allTracks = $state(false);

	let seq = 0;
	async function load(r: Range) {
		const mine = ++seq;
		try {
			const s = await api.listeningStats(rangeStart(r));
			if (mine !== seq) return;
			stats = s;
			shown = r;
			error = null;
		} catch (e) {
			if (mine === seq) error = String(e);
		}
	}

	// A song's time is settled when the next one starts: refresh on each new song and range switch.
	$effect(() => {
		playback.now?.videoId;
		const r = range;
		lastRange = r;
		untrack(() => load(r));
	});

	const tag = $derived(tagOf(currentLocale.id));
	const num = $derived(new Intl.NumberFormat(tag));
	const signed = $derived(new Intl.NumberFormat(tag, { style: 'percent', signDisplay: 'exceptZero' }));
	const fmt = (opts: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat(tag, opts);

	const chart = $derived(stats ? toBars(stats.days, shown) : []);
	const monthly = $derived(chart[0]?.key.length === 7);
	const average = $derived(chart.length ? (stats?.secs ?? 0) / chart.length : 0);
	const peak = $derived(chart.reduce<Bar | undefined>((top, b) => (b.secs > (top?.secs ?? 0) ? b : top), undefined));
	const delta = $derived(stats ? change(stats.secs, stats.prevSecs) : null);
	const when = $derived(stats ? busiest(stats.hours) : null);
	// Our own play count in the Plays column.
	const songs = $derived(stats?.topSongs.map((s) => ({ ...s.song, play_count: num.format(s.plays) })) ?? []);
	const cover = $derived(stats?.topSongs[0]?.song.thumbnail ?? stats?.topArtists[0]?.thumbnail);

	// Artist photos only live on their pages. Asked once each per visit.
	let photos = $state<Record<string, string>>({});
	const asked = new Set<string>();
	$effect(() => {
		for (const { id } of stats?.topArtists ?? []) {
			if (!id?.startsWith('UC') || asked.has(id)) continue;
			asked.add(id);
			artistPage(id).then((p) => {
				const src = p?.avatar ?? p?.thumbnail;
				if (src) photos = { ...photos, [id]: src };
			});
		}
	});
	const artistImage = (a: TopArtist) => (a.id ? photos[a.id] : undefined) ?? a.thumbnail ?? null;
	const headerImage = $derived(stats?.topArtists[0] ? artistImage(stats.topArtists[0]) : null);

	// Not a library context (nothing to reopen), so no sourceId: the queue is just named.
	const queueName = $derived(`Top tracks ${PHRASE[shown]}`);
	const ours = $derived(!!playback.now && playback.queue.sourceName === queueName && !playback.queue.sourceId);
	const playing = $derived(ours && !playback.paused);
	function playTop(i = 0) {
		if (!songs.length) return;
		if (ours && i === 0) return api.togglePause();
		openPlayer();
		api.playPlaylist(songs, i, undefined, queueName);
	}

	async function clearHistory() {
		const ok = await confirmDialog({
			title: 'Clear listening history?',
			body: 'This forgets every play counted on this device. Your stats, Your Rotation and recommendations start over. This can’t be undone.',
			confirmLabel: 'Clear'
		});
		if (!ok) return;
		try {
			await api.clearListeningHistory();
			toast('Listening history cleared');
			await load(range);
		} catch (e) {
			toast.error(String(e));
		}
	}

	const summary = $derived(
		stats?.firstAt
			? [
					`${listened(stats.secs, tag)} listened`,
					`${num.format(stats.plays)} plays`,
					`${num.format(stats.artists)} artist${stats.artists === 1 ? '' : 's'}`
				].join(' • ')
			: null
	);

	let tint = $state<Tint>();
	let sentinel = $state<HTMLElement>();
</script>

<svelte:head><title>Listening stats</title></svelte:head>

{#snippet stat(value: string, label: string, hint?: string | null)}
	<div class="flex min-w-0 flex-col gap-1 rounded-lg bg-bg-highlight p-4">
		<span class="font-title text-xl leading-7 font-bold text-white tabular-nums">{value}</span>
		<span class="truncate text-sm text-text-subdued">{label}{#if hint}<span class="text-text-meta">{' • '}{hint}</span>{/if}</span>
	</div>
{/snippet}

<StickyHeader title="Listening stats" tint={tint?.color} {playing} onPlay={() => playTop()} {sentinel} />
<PageHeader type="Stats" title="Listening stats" image={headerImage ?? cover} round={!!headerImage} bind:tintOut={tint} details={summary}>
	{#snippet imageSnippet()}
		{#if headerImage || cover}
			<img src={headerImage ?? cover} alt="" class="size-full object-cover" />
		{:else}
			<div class="grid size-full place-items-center bg-bg-menu text-text-subdued"><Icon icon={ChartBarIcon} size={64} /></div>
		{/if}
	{/snippet}
</PageHeader>

<ActionBar bind:sentinel tint={tint?.color} {playing} onPlay={() => playTop()} playLabel="Play your top tracks">
	{#snippet right()}
		<div class="flex gap-2" role="group" aria-label="Time range">
			{#each RANGES as r (r)}
				<Pill variant="chip" selected={range === r} onclick={() => (range = r)}>{LABEL[r]}</Pill>
			{/each}
		</div>
	{/snippet}

	<div class="flex flex-col gap-10 px-[var(--content-spacing)] pb-10">
		{#if error && !stats}
			<ErrorState onRetry={() => load(range)} />
		{:else if !stats}
			<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">{#each Array(4) as _, i (i)}<div class="h-[84px] rounded-lg bg-bg-highlight"></div>{/each}</div>
			<div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-4"><Skeleton variant="card" count={5} /></div>
			<Skeleton variant="row" count={4} />
		{:else if !stats.firstAt}
			<div class="py-16">
				<EmptyState
					icon={ChartBarIcon}
					title="Your stats will appear here"
					body="Play some music and we'll count your top artists, tracks and listening time. Everything stays on this device."
					actionLabel="Find something to play"
					onAction={() => goto('/search')}
				/>
			</div>
		{:else}
			<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
				{@render stat(
					listened(stats.secs, tag),
					'Listening time',
					delta !== null && shown !== 'all' ? `${signed.format(delta / 100)} vs before` : null
				)}
				{@render stat(num.format(stats.plays), 'Plays')}
				{@render stat(
					num.format(stats.songs),
					'Songs',
					shown !== 'all' && stats.newSongs ? `${num.format(stats.newSongs)} new` : null
				)}
				{@render stat(
					`${num.format(stats.streak)} day${stats.streak === 1 ? '' : 's'}`,
					'Streak',
					stats.bestStreak > stats.streak ? `best ${num.format(stats.bestStreak)}` : null
				)}
			</div>

			{#if !stats.plays}
				<p class="text-base text-text-subdued">Nothing played in this period. Try a longer range.</p>
			{/if}

			{#if stats.topArtists.length}
				<section>
					<h2 class="font-title text-xl font-bold text-white">Top artists {PHRASE[shown]}</h2>
					<p class="mb-2 text-sm text-text-subdued">Only visible to you</p>
					<CardRow items={stats.topArtists} key={(a) => a.id ?? a.name}>
						{#snippet card(a)}
							{@const item = { kind: 'artist' as const, id: a.id ?? '', title: a.name, thumbnail: artistImage(a) ?? undefined }}
							<Card
								title={a.name}
								subtitle={`${listened(a.secs, tag)} • ${num.format(a.plays)} plays`}
								image={artistImage(a)}
								kind="artist"
								href={a.id ? actions.artistHref(a.id) : undefined}
								onPlay={a.id ? () => playOrToggle([a.id], () => playItem(item)) : undefined}
								playing={a.id ? isPlayingContext(a.id) : false}
								oncontextmenu={a.id ? (e) => openItemMenu(e, item) : undefined}
							/>
						{/snippet}
					</CardRow>
				</section>
			{/if}

			{#if songs.length}
				<section>
					<div class="flex items-end justify-between gap-4">
						<div>
							<h2 class="font-title text-xl font-bold text-white">Top tracks {PHRASE[shown]}</h2>
							<p class="mb-2 text-sm text-text-subdued">Only visible to you</p>
						</div>
						{#if songs.length > 4}
							<button type="button" class="mb-2 shrink-0 text-sm font-bold text-text-subdued hover:text-white hover:underline" onclick={() => (allTracks = !allTracks)}>
								{allTracks ? 'Show less' : 'Show all'}
							</button>
						{/if}
					</div>
					<TrackTable
						tracks={allTracks ? songs : songs.slice(0, 4)}
						variant="artist"
						showHeader={false}
					/>
				</section>
			{/if}

			{#if stats.topAlbums.length}
				<section>
					<h2 class="mb-2 font-title text-xl font-bold text-white">Top albums {PHRASE[shown]}</h2>
					<CardRow items={stats.topAlbums} key={(al) => al.id}>
						{#snippet card(al)}
							{@const item = { kind: 'album' as const, id: al.id, title: al.title, subtitle: al.artist ?? undefined, thumbnail: al.thumbnail ?? undefined }}
							<Card
								title={al.title}
								subtitle={[al.artist, listened(al.secs, tag)].filter(Boolean).join(' • ')}
								image={al.thumbnail}
								kind="album"
								href={actions.albumHref(al.id)}
								playing={isPlayingContext(al.id)}
								onPlay={() => playOrToggle([al.id], () => playItem(item))}
								oncontextmenu={(e) => openItemMenu(e, item)}
							/>
						{/snippet}
					</CardRow>
				</section>
			{/if}

			{#if stats.topPodcasts.length}
				<section>
					<h2 class="font-title text-xl font-bold text-white">Top podcasts {PHRASE[shown]}</h2>
					<p class="mb-2 text-sm text-text-subdued">
						{listened(stats.podcastSecs, tag)} • {num.format(stats.episodes)} episode{stats.episodes === 1 ? '' : 's'}
					</p>
					<CardRow items={stats.topPodcasts} key={(p) => p.id ?? p.name}>
						{#snippet card(p)}
							<Card
								title={p.name}
								subtitle={`${listened(p.secs, tag)} • ${num.format(p.plays)} play${p.plays === 1 ? '' : 's'}`}
								image={p.thumbnail}
								kind="album"
								href={p.id ? actions.showHref(p.id) : undefined}
							/>
						{/snippet}
					</CardRow>
				</section>
			{/if}

			{#if stats.plays}
				<section>
					<h2 class="mb-1 font-title text-xl font-bold text-white">Activity</h2>
					<p class="mb-4 text-sm text-text-subdued">
						{monthly ? 'Monthly' : 'Daily'} average {listened(average, tag)}{#if peak}
							• busiest {monthly ? 'month' : 'day'} {fmt(monthly ? { month: 'long', year: 'numeric' } : { weekday: 'short', month: 'short', day: 'numeric' }).format(peak.date)} ({listened(peak.secs, tag)}){/if}
					</p>
					<ListeningChart bars={chart} {monthly} peak={peak?.key} />
				</section>

				<section>
					<h2 class="mb-1 font-title text-xl font-bold text-white">When you listen</h2>
					{#if when}
						<p class="mb-4 text-sm text-text-subdued">
							Mostly on {fmt({ weekday: 'long' }).format(new Date(2026, 0, 4 + when.day))}s, around {fmt({ hour: 'numeric' }).format(new Date(2026, 0, 4, when.hour))}
						</p>
					{/if}
					<div class="grid gap-8 lg:grid-cols-[16rem_minmax(0,1fr)]">
						<ListeningDayparts hours={stats.hours} />
						<div class="min-w-0"><ListeningHours hours={stats.hours} /></div>
					</div>
				</section>
			{/if}

			<footer class="flex flex-wrap items-center justify-between gap-3 border-t border-white/10 pt-6">
				<p class="text-sm text-text-subdued">
					Counted on this device only{stats.firstAt ? ` since ${fmt({ dateStyle: 'long' }).format(new Date(stats.firstAt * 1000))}` : ''}.
				</p>
				<Pill variant="outline" onclick={clearHistory}>Clear listening history</Pill>
			</footer>
		{/if}
	</div>
</ActionBar>
