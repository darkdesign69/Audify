// DEV ONLY (lane magic-d1): radio pages, song credits, taste exclusions and the song-link single
// pick for the browser preview. Mirrors crates/core/src/radio_page.rs, credits.rs and taste.rs.
/* eslint-disable @typescript-eslint/no-explicit-any */
import type { SongItem } from '$lib/api';
import { findSong, pool, poolSlice } from './state';

const excluded = new Map<string, string[]>();

function seedOf(a: any): { seed?: SongItem; name: string } {
	const seed = a.kind === 'song' ? findSong(a.id) : undefined;
	return { seed, name: a.name ?? seed?.title ?? 'Song' };
}

export function magicD1Handlers(play: (item: SongItem, from: string | null) => void): Record<string, (a: any) => any> {
	return {
		radio_tracks: (a) => {
			if (String(a.id).startsWith('LOCALPLAYLIST:')) throw 'This has no radio behind it.';
			const { seed, name } = seedOf(a);
			const start = Math.abs([...String(a.id)].reduce((h, c) => h * 31 + c.charCodeAt(0), 7)) % pool.length;
			const rest = poolSlice(start, 49).filter((s) => s.video_id !== seed?.video_id);
			const items = seed ? [seed, ...rest] : rest;
			const artists = [...new Set(rest.map((s) => s.artists.split(/[,&]/)[0].trim()))].slice(0, 3);
			return {
				title: `${name.replace(/ Radio$/, '')} Radio`,
				playlistId: a.kind === 'song' ? `RDAMVM${a.id}` : `RDAMPL${a.id}`,
				seed: seed ?? null,
				thumbnail: seed?.thumbnail ?? null,
				description: artists.length ? `With ${artists.join(', ')} and more` : null,
				items
			};
		},
		play_single_at: (a) => play(a.item, a.from ?? null),
		song_credits: (a) => {
			const s: SongItem = a.item;
			const main = s.artists.split(/[,&]/)[0].trim();
			const id = s.artist_id ?? s.artist_runs?.find((r) => r.id)?.id ?? undefined;
			return {
				sections: [
					{ title: 'Performed by', people: [{ name: main, artistId: id }] },
					{ title: 'Written by', people: [{ name: main }, { name: 'Jane Writer' }] },
					{ title: 'Produced by', people: [{ name: 'Sam Producer' }] }
				],
				source: 'Mock Records',
				// The core merges these (`credits::credit_people`); the preview has no core.
				people: [
					{ name: main, artistId: id, roles: ['Main Artist', 'Writer'] },
					{ name: 'Jane Writer', roles: ['Writer'] },
					{ name: 'Sam Producer', roles: ['Producer'] }
				]
			};
		},
		taste_excluded: () => [...excluded.keys()],
		taste_set_excluded: (a) => {
			if (a.excluded) excluded.set(a.id, a.videoIds ?? []);
			else excluded.delete(a.id);
		}
	};
}
