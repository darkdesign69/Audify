// ── lane magic-d1 ── Commands behind the track actions and radio pages (MAGIC-GAPS #1, #6, #16)
// and the song-link single pick. Kept out of api.ts so the parallel lanes' merges stay small.
import { invoke } from '@tauri-apps/api/core';
import type { SongItem } from './api';

export type RadioKind = 'song' | 'artist' | 'album' | 'playlist';

/** A radio as a browsable playlist (`radio_tracks`, crates/core/src/radio_page.rs). */
export interface RadioPage {
	/** "<seed> Radio". */
	title: string;
	/** The YouTube radio list that answered (`RDAMVM…`): the queue's source when it plays, so
	 *  Autoplay carries on with this same radio. */
	playlistId: string;
	seed?: SongItem | null;
	thumbnail?: string | null;
	/** "With A, B, C and more". */
	description?: string | null;
	items: SongItem[];
}

/** The radio's tracks, without playing anything. */
export const radioTracks = (kind: RadioKind, id: string, name?: string) =>
	invoke<RadioPage>('radio_tracks', { kind, id, name });

/** A single pick that starts `start` seconds in (a song link's `t=`), then Autoplay. */
export const playSingleAt = (item: SongItem, from?: string | null, start?: number | null) =>
	invoke<void>('play_single_at', { item, from: from ?? null, start: start ?? null });

export interface CreditPerson {
	name: string;
	/** The artist's channel (`UC…`) when YouTube links it. */
	artistId?: string;
}
export interface CreditSection {
	/** The role as YouTube words it: "Written by". */
	title: string;
	people: CreditPerson[];
}
/** One row per person with every role they had (the core's `credits::credit_people`). */
export interface CreditRow {
	name: string;
	artistId?: string;
	/** Spotify's role words, in credit order: "Writer", "Producer". */
	roles: string[];
}
export interface SongCredits {
	sections: CreditSection[];
	/** The label that supplied the data, Spotify's "Source". */
	source?: string;
	/** The Now Playing card's rows. */
	people: CreditRow[];
}

/** YouTube Music's song credits (a music video asks its song). */
export const songCredits = (item: SongItem) => invoke<SongCredits>('song_credits', { item });

/** Exclude a song or playlist from the taste profile, or include it again. */
export const tasteSetExcluded = (
	id: string,
	kind: 'song' | 'playlist',
	title: string | undefined,
	videoIds: string[],
	excluded: boolean
) => invoke<void>('taste_set_excluded', { id, kind, title, videoIds, excluded });

export const tasteExcluded = () => invoke<string[]>('taste_excluded');
