// ── lane magic-d6 ── Where the Now Playing header's context name links (MAGIC-GAPS #20). Spotify
// links whatever the queue came from: a playlist, an album, an artist, Liked Songs, a mix, a radio.
// The queue itself only keeps a playlist id (`sourceId`) and a title; the context card the core
// records when something starts (`home-changed` "played", `recent_play_contexts`) knows the rest.
// Pure, so `contextlink.check.ts` pins it.

export interface ContextCard {
	kind: string;
	id: string;
	title: string;
	subtitle?: string;
	thumbnail?: string;
	/** A radio's seed kind when it isn't a song. */
	params?: string;
}

const enc = encodeURIComponent;
const LIKED_ID = 'VLLM';
const RADIO_KINDS = ['song', 'artist', 'album', 'playlist'];

/** "Bohemian Rhapsody Radio" → "Bohemian Rhapsody" (the radio page adds " Radio" itself). */
const seedName = (title: string) => title.replace(/\s+Radio$/i, '');

function radio(kind: string, id: string, title: string, thumb?: string): string {
	const q = new URLSearchParams();
	if (title) q.set('name', seedName(title));
	if (thumb) q.set('thumb', thumb);
	const qs = q.toString();
	return `/radio/${RADIO_KINDS.includes(kind) ? kind : 'song'}/${enc(id)}${qs ? `?${qs}` : ''}`;
}

/** The page a context card opens, or null when it has none (a local-only list, say). */
export function contextHref(c: ContextCard): string | null {
	if (!c.id) return null;
	switch (c.kind) {
		case 'liked':
			return `/playlist/${LIKED_ID}`;
		case 'album':
			return `/album/${enc(c.id)}`;
		case 'artist':
			return `/artist/${enc(c.id)}`;
		case 'playlist':
			return c.id === 'EPISODES' ? '/collection/episodes' : `/playlist/${enc(c.id)}`;
		case 'podcast':
			return `/show/${enc(c.id)}`;
		case 'audiobook': // stream: audiobooks
			return `/audiobook/${enc(c.id)}`;
		case 'radio':
			return radio(c.params ?? 'song', c.id, c.title, c.thumbnail);
		case 'mix': // an artist radio (api.rs start_radio records it as a mix)
			return radio('artist', c.id, c.title, c.thumbnail);
		case 'daily_mix': {
			const q = new URLSearchParams({ title: c.title });
			if (c.subtitle) q.set('subtitle', c.subtitle);
			if (c.thumbnail) q.set('thumb', c.thumbnail);
			return `/mix/${enc(c.id)}?${q.toString()}`;
		}
		default:
			return null;
	}
}

/** Does the recorded card describe the queue now playing? Titles must agree: the card is only
 *  the latest one started, and a single pick or a restart can leave it behind the queue. */
export function cardMatches(c: ContextCard | null | undefined, sourceName: string | null | undefined): boolean {
	if (!c || !sourceName) return false;
	return c.title.trim().toLowerCase() === sourceName.trim().toLowerCase();
}

/** A song radio whose card is gone (restart, private session): its seed is the queue row whose
 *  title the radio is named after ("<song> Radio"). */
export function songRadioHref(
	sourceName: string | null | undefined,
	items: { video_id: string; title: string; thumbnail?: string | null }[]
): string | null {
	if (!sourceName || !/\sRadio$/i.test(sourceName)) return null;
	const name = seedName(sourceName).trim().toLowerCase();
	const seed = items.find((i) => i.title.trim().toLowerCase() === name);
	return seed ? radio('song', seed.video_id, seed.title, seed.thumbnail ?? undefined) : null;
}
