// Menus of the playback views, built on the kit's openMenu (no new menu styles):
//   - the Now Playing "…" menu: the track menu + Sleep timer ▸ (now-playing-panel.md, audio stream);
//   - the lyrics "…" menu: Lyrics source ▸ (Automatic + every enabled provider) and a timing nudge.
import * as api from '$lib/api';
import { openMenu, trackMenu, type MenuEntry, type MenuTarget } from '$lib/kit/menus';
import { audio, cancelSleep, formatSleepLeft, SLEEP_OPTIONS, startSleep } from '$lib/audio.svelte';
import { playback } from '$lib/player.svelte';
import { nowPlayingSong } from '$lib/shell/nowplaying';
import { lyr, nudgeLyricsOffset, pickLyricsSource } from './lyrics.svelte';
import MoonIcon from 'phosphor-svelte/lib/MoonIcon';
import MicrophoneStageIcon from 'phosphor-svelte/lib/MicrophoneStageIcon';
import TimerIcon from 'phosphor-svelte/lib/TimerIcon';

/** The timed option last picked here, to check it while its timer runs. */
let chosen: number | null = null;

/** Sleep timer entries: 5…60 min, End of track/episode, Turn off timer (when one is set). */
export function sleepEntries(): MenuEntry[] {
	const episode = !!playback.now?.isEpisode;
	const mode = audio.sleep.mode;
	const entries: MenuEntry[] = SLEEP_OPTIONS.map((o) => {
		const label = o.minutes === null ? (episode ? 'End of episode' : 'End of track') : o.label;
		const checked =
			o.minutes === null ? mode?.kind === 'endOfTrack' : mode?.kind === 'at' && chosen === o.minutes;
		return {
			label,
			checked,
			onSelect: () => {
				chosen = o.minutes;
				void startSleep(o.minutes).catch(() => {});
			}
		};
	});
	if (mode) {
		entries.unshift(
			{
				type: 'heading',
				label:
					mode.kind === 'endOfTrack'
						? `Stops at the end of this ${episode ? 'episode' : 'track'}`
						: `Stops in ${formatSleepLeft(audio.sleepLeft ?? 0)}`
			},
			{ label: 'Turn off timer', onSelect: () => void cancelSleep().catch(() => {}) },
			{ type: 'separator' }
		);
	}
	return entries;
}

/** The sleep timer's label for the more menu: "Sleep timer" or "Sleep timer · 12:05". */
export function sleepLabel(): string {
	const mode = audio.sleep.mode;
	if (!mode) return 'Sleep timer';
	if (mode.kind === 'endOfTrack') return 'Sleep timer · End of track';
	return `Sleep timer · ${formatSleepLeft(audio.sleepLeft ?? 0)}`;
}

/** Now Playing view "…": the playing track's menu, then Sleep timer ▸. */
export function openNowPlayingMenu(target: MenuTarget) {
	const song = nowPlayingSong();
	const base = song ? trackMenu([song], { playlistId: playback.queue.sourceId ?? undefined }) : [];
	openMenu(
		target,
		[
			...base,
			...(base.length ? [{ type: 'separator' } as MenuEntry] : []),
			{ type: 'sub', label: sleepLabel(), icon: MoonIcon, items: sleepEntries, width: 240 }
		],
		{ width: 328 }
	);
}

/** Just the sleep timer, for the countdown chip in the panel header. */
export function openSleepMenu(target: MenuTarget) {
	openMenu(target, sleepEntries(), { width: 240 });
}

let providers: api.LyricsProvider[] | null = null;
export function loadLyricsProviders() {
	if (providers) return;
	api
		.lyricsProviders()
		.then((p) => (providers = p))
		.catch(() => {});
}

/** The lyrics toolbar "…": Lyrics source ▸ and the timing nudge. */
export function openLyricsMenu(target: MenuTarget) {
	const l = lyr.lyrics;
	const pinned = !!l?.pinned;
	const offset = l?.offset_ms ?? 0;
	const sources: MenuEntry[] = [
		{ label: 'Automatic', checked: !pinned, onSelect: () => void pickLyricsSource(null) },
		{ type: 'separator' },
		...(providers ?? [])
			.filter((p) => p.on)
			.map(
				(p): MenuEntry => ({
					label: p.name,
					checked: pinned && l?.provider === p.id,
					onSelect: () => void pickLyricsSource(p.id)
				})
			)
	];
	const synced = !!l?.synced;
	const secs = (ms: number) => `${ms > 0 ? '+' : ''}${(ms / 1000).toFixed(2)} s`;
	openMenu(
		target,
		[
			{ type: 'sub', label: 'Lyrics source', icon: MicrophoneStageIcon, items: sources, width: 220 },
			...(synced
				? ([
						{ type: 'separator' },
						{
							label: 'Show lyrics earlier',
							icon: TimerIcon,
							hint: offset ? secs(offset) : undefined,
							onSelect: () => void nudgeLyricsOffset(-250)
						},
						{ label: 'Show lyrics later', onSelect: () => void nudgeLyricsOffset(250) },
						...(offset
							? [{ label: 'Reset timing', onSelect: () => void nudgeLyricsOffset(-offset) }]
							: [])
					] as MenuEntry[])
				: [])
		],
		{ width: 260 }
	);
}
