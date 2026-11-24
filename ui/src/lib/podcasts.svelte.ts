// Podcast state for the UI (stream E). State and loaders only — the pages (search tab, show page,
// episode page, Your Episodes, New episodes, Continue listening) are stream C's.
//
// Usage:
//   import { podcasts, initPodcasts, toggleFollow, progressOf } from '$lib/podcasts.svelte';
//   onMount(() => { initPodcasts(); });              // once, at app start (idempotent)
//   {#each podcasts.follows as show} …               // library sidebar / Your Library filter
//   progressOf(ep).fraction                          // live progress bar for any episode row
//
// Everything updates from Rust events: `podcasts:progress` (position saved every 5 s / on pause),
// `podcasts:new` (background refresh found episodes), `podcasts:skipped` (SponsorBlock).
import {
	onPodcastNew,
	onPodcastProgress,
	onPodcastSkipped,
	podcastContinueListening,
	podcastFollow,
	podcastFollows,
	podcastMarkPlayed,
	podcastNewEpisodes,
	podcastSavedEpisodes,
	podcastSaveEpisode,
	podcastUnfollow,
	type EpisodeProgress,
	type PodcastEpisode,
	type PodcastShow
} from './api';

export const podcasts = $state({
	/** Followed shows, most recently followed first. */
	follows: [] as PodcastShow[],
	/** Unplayed episodes of followed shows, last 30 days, newest first. */
	newEpisodes: [] as PodcastEpisode[],
	/** Started, unfinished, most recent first. */
	continueListening: [] as PodcastEpisode[],
	/** "Your Episodes". */
	saved: [] as PodcastEpisode[],
	/** Live progress by episode id, newer than whatever a list was loaded with. */
	progress: {} as Record<string, EpisodeProgress>,
	/** Count of new episodes since the user last looked (for a dot on the Podcasts entry). */
	unseenNew: 0,
	/** The last SponsorBlock skip, for a transient toast; null once shown. */
	lastSkip: null as {
		id: string;
		category: string;
		from: number;
		to: number;
	} | null,
	loaded: false
});

let started = false;

/** Load the lists and subscribe to the Rust events. Safe to call more than once. */
export async function initPodcasts() {
	if (started) return;
	started = true;
	await onPodcastProgress(({ id, progress }) => {
		podcasts.progress[id] = progress;
		// A finished or freshly started episode moves in / out of Continue listening.
		const inList = podcasts.continueListening.some((e) => e.id === id);
		if (progress.played ? inList : !inList && progress.positionSecs >= 10) {
			void reloadContinueListening();
		}
	});
	await onPodcastNew((r) => {
		podcasts.unseenNew += r.newEpisodes.length;
		void reloadNewEpisodes();
	});
	await onPodcastSkipped((s) => {
		podcasts.lastSkip = s;
	});
	await reloadAll();
}

export async function reloadAll() {
	const [follows, fresh, cont, saved] = await Promise.all([
		podcastFollows(),
		podcastNewEpisodes(),
		podcastContinueListening(),
		podcastSavedEpisodes()
	]);
	podcasts.follows = follows;
	podcasts.newEpisodes = fresh;
	podcasts.continueListening = cont;
	podcasts.saved = saved;
	podcasts.loaded = true;
}

export const reloadFollows = async () => (podcasts.follows = await podcastFollows());
export const reloadNewEpisodes = async () => (podcasts.newEpisodes = await podcastNewEpisodes());
export const reloadContinueListening = async () =>
	(podcasts.continueListening = await podcastContinueListening());
export const reloadSaved = async () => (podcasts.saved = await podcastSavedEpisodes());

export const isFollowed = (id: string) => podcasts.follows.some((s) => s.id === id);

/** Follow / unfollow; returns the new state. */
export async function toggleFollow(show: PodcastShow): Promise<boolean> {
	if (isFollowed(show.id)) {
		await podcastUnfollow(show.id);
		podcasts.follows = podcasts.follows.filter((s) => s.id !== show.id);
		podcasts.newEpisodes = podcasts.newEpisodes.filter((e) => e.showId !== show.id);
		return false;
	}
	const followed = await podcastFollow(show);
	podcasts.follows = [followed, ...podcasts.follows];
	// The first sync runs in the background; its episodes don't count as "new".
	return true;
}

export const isSaved = (id: string) => podcasts.saved.some((e) => e.id === id);

/** Add to / remove from Your Episodes; returns the new state. */
export async function toggleSaved(ep: PodcastEpisode): Promise<boolean> {
	const save = !isSaved(ep.id);
	await podcastSaveEpisode(ep, save);
	podcasts.saved = save
		? [{ ...ep, saved: true }, ...podcasts.saved]
		: podcasts.saved.filter((e) => e.id !== ep.id);
	return save;
}

export async function setPlayed(ep: PodcastEpisode, played: boolean) {
	podcasts.progress[ep.id] = await podcastMarkPlayed(ep.id, played);
	await Promise.all([reloadContinueListening(), reloadNewEpisodes()]);
}

/** The freshest progress for an episode row: live event data, else what the row was loaded with. */
export const progressOf = (ep: PodcastEpisode): EpisodeProgress =>
	podcasts.progress[ep.id] ?? ep.progress;

/** "45 min left" style remaining time, or null when not started / played. */
export function remainingSecs(ep: PodcastEpisode): number | null {
	const p = progressOf(ep);
	const dur = p.durationSecs || ep.durationSecs || 0;
	if (p.played || p.positionSecs < 10 || dur <= 0) return null;
	return Math.max(0, dur - p.positionSecs);
}

export const markNewSeen = () => (podcasts.unseenNew = 0);
