// Run: node --experimental-strip-types src/lib/collections-format.check.ts (no test runner needed)
const assert = {
	equal(a: unknown, b: unknown) {
		if (a !== b) throw new Error(`expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`);
	},
	deepEqual(a: unknown, b: unknown) {
		if (JSON.stringify(a) !== JSON.stringify(b))
			throw new Error(`expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`);
	}
};
import { formatTotal, albumKind, playlistAuthor, mosaicCovers, songsMeta } from './collections/format.ts';
assert.equal(formatTotal(25 * 60 + 6, 7), '25 min 6 sec');
assert.equal(formatTotal(92 * 60, 30), '1 hr 32 min');
assert.equal(formatTotal(11 * 3600 + 5 * 60, 227), 'about 11 hr');
assert.equal(formatTotal(3 * 3600 + 30 * 60, 50, true), 'about 3 hr 30 min');
assert.equal(formatTotal(3 * 3600 + 30 * 60, 50), '3 hr 30 min');
assert.equal(formatTotal(25 * 60 + 6, 7, true), '25 min 6 sec');
assert.equal(formatTotal(0, 0), '');
assert.deepEqual(albumKind('Album • 2018'), { type: 'Album', year: '2018' });
assert.deepEqual(albumKind('EP • 2020'), { type: 'EP', year: '2020' });
assert.deepEqual(albumKind('Single • Artist • 2021'), { type: 'Single', year: '2021' });
assert.equal(playlistAuthor('Playlist • Catherine • 2024'), 'Catherine');
assert.equal(playlistAuthor('Catherine • 1.2K views • 50 tracks'), 'Catherine');
const s = (a: string) => ({ video_id: a, title: a, artists: '', album_id: a, thumbnail: a, duration: '3:00' });
assert.deepEqual(mosaicCovers([s('a'), s('a'), s('b'), s('c'), s('d')] as never), ['a', 'b', 'c', 'd']);
assert.deepEqual(mosaicCovers([s('a'), s('b')] as never), []);
assert.equal(songsMeta([s('a')] as never), '1 song, 3 min');
console.log('ok');
