// The music video's state, and the handoff of the one live <video> element.
//
// It lives outside NowPlaying.svelte because +layout unmounts that component whenever the player
// view is closed. Rebuilding the element on every reopen meant opening the stream at byte 0 and
// then seeking minutes into a WebM, which has to fetch the container index first and takes seconds
// that no amount of tuning the sync loop could recover. So the element is owned by VideoSurface,
// which is always mounted, and the view borrows it.
import * as api from './api';
import { playback, prefs } from './player.svelte';

/** Session-sticky on purpose: someone who hits "show artwork" wants artwork now and almost
 *  certainly on the next video too, but a permanent no is what the setting is for. */
export const video = $state({
	want: true,
	/** Whether the player view is holding the element, i.e. someone can actually see the picture.
	 *  VideoSurface releases the stream when nobody can and the music is not moving. */
	shown: false,
	/** The loopback proxy URL for the current track, or null. Owned by VideoSurface's fetch. */
	url: null as string | null,
	/**
	 * Theater mode shows the picture edge to edge instead of inside the stage. Reset when theater
	 *  closes.
	 */
	cinema: false,
	/** The Song/Video side just picked, held while the other version resolves (a second or two). */
	switching: null as boolean | null,
	/** The box holding the picture now (see `claimVideo`), so a holder can tell it got it. */
	holder: null as HTMLElement | null
});

// ── stream: video-pods ── A podcast episode's picture follows the podcast rules (the core's
// `podcasts::video`): the episode's own YouTube video, an RSS episode's YouTube twin, or a video
// enclosure, never on Data Saver or offline. Fetched per episode by VideoSurface.
export const episodeVideo = $state({
	/** The episode `src` was fetched for. */
	for: null as string | null,
	src: null as api.EpisodeVideo | null,
	/** "Switch to audio" / "Switch to video" on an episode. Session-sticky like `video.want`;
	 *  null follows the setting (`src.auto`). */
	want: null as boolean | null
});

const isEpisode = () => !!playback.now?.isEpisode;
/** The playing episode's picture, or null (a song, or no video for this episode). */
export const episodeSrc = () =>
	isEpisode() && episodeVideo.for === playback.now?.videoId ? episodeVideo.src : null;
/** Whether the picture is wanted: the user's choice, else for an episode the setting. */
export const wantNow = () => (isEpisode() ? (episodeVideo.want ?? !!episodeSrc()?.auto) : video.want);
/** Which video id the picture is fetched for: an episode's may be its YouTube twin. Null for a
 *  video enclosure, whose URL the `<video>` element loads directly. */
export const videoKey = () => {
	const src = episodeSrc();
	return src ? (src.videoId ?? null) : (playback.now?.videoId ?? null);
};
// ── end stream: video-pods ──

export const canVideo = () =>
	isEpisode() ? !!episodeSrc() : prefs.musicVideos && !!playback.now?.isVideo;
/** The playing track's other version (#442), read off its queue row: a radio learns it after the
 *  track has started, so the now-playing event can't carry it. */
export const counterpart = () => {
	const row = playback.queue.items[playback.queue.currentIndex];
	return row && row.video_id === playback.now?.videoId ? row.counterpart : undefined;
};
/** Whether the player view offers Song/Video: a video to show or hide, or a version to switch to. */
export const canSwitch = () =>
	isEpisode()
		? !!episodeSrc()
		: prefs.musicVideos && (!!playback.now?.isVideo || !!counterpart());
export const hasVideo = () =>
	canVideo() && !!video.url;
export const showVideo = () => hasVideo() && wantNow();
/** Cinema only makes sense while a video is actually the picture. */
export const cinema = () => video.cinema && showVideo();

/** Which side of Song/Video is picked: the user's choice, not what is on screen. "Video" stays
 *  picked while the picture is still loading, and the artwork stands in until it arrives. */
export const videoSide = () =>
	isEpisode()
		? !!episodeSrc() && wantNow()
		: (video.switching ?? (!!playback.now?.isVideo && video.want));

/** Song/Video. With a twin to switch to (#442) it changes what plays, the way YouTube Music's does;
 *  without one it shows or hides the video this track already is. An episode only shows or hides
 *  its picture: the sound is the same either way. */
export async function pickVersion(on: boolean) {
	if (on === videoSide()) return;
	if (isEpisode()) {
		episodeVideo.want = on;
		return;
	}
	if (!counterpart() || on === !!playback.now?.isVideo) {
		video.want = on;
		return;
	}
	if (on) video.want = true;
	video.switching = on;
	try {
		await api.switchVersion();
	} finally {
		video.switching = null;
	}
}

// The live element and where it waits when nothing is showing it. Plain module lets: this is DOM
// identity, nothing renders off it.
let node: HTMLVideoElement | null = null;
let parking: HTMLElement | null = null;

/** VideoSurface, once, at mount. */
export function registerVideo(v: HTMLVideoElement, park: HTMLElement) {
	node = v;
	parking = park;
	park.appendChild(v);
}

/** The live element, for the ambient light to read frames from. */
export const videoElement = () => node;

// Who holds the picture. Several views can be mounted at once (the Now Playing panel and the
// player bar's thumbnail, theater over either), so each claim is a holder with a rank, and the
// picture lives in the highest-ranked, most recent one. Letting go moves it to the next holder
// rather than the parking box. The player bar ranks lowest: it only shows the picture when no
// bigger view does.
let holders: { box: HTMLElement; rank: number }[] = [];

function place() {
	let top: (typeof holders)[number] | undefined;
	for (const h of holders) if (!top || h.rank >= top.rank) top = h;
	if (node) (top?.box ?? parking)?.appendChild(node);
	video.shown = !!top;
	video.holder = top?.box ?? null;
}

/** Put the picture in `box` (or the top holder, see above). Synchronous on purpose: a media
 *  element that is out of the document across a microtask gets paused by the spec's removal
 *  steps, and a paused picture is exactly the desync this whole thing exists to avoid. */
export function claimVideo(box: HTMLElement, rank = 1) {
	holders = holders.filter((h) => h.box !== box);
	holders.push({ box, rank });
	place();
}

/** Let go of the picture. Same rule: synchronous, never from an effect. Without `box`, every
 *  holder lets go and it goes back to parking. */
export function parkVideo(box?: HTMLElement) {
	holders = box ? holders.filter((h) => h.box !== box) : [];
	place();
}
