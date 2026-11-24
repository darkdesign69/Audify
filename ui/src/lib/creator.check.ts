// Self-check for creatorOf (discover/format.ts): run with `node --experimental-strip-types`.
import { cardSubtitle, creatorOf } from './discover/format.ts';

const assert = {
	equal(a: string, b: string) {
		if (a !== b) throw new Error(`FAIL: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`);
	}
};

assert.equal(creatorOf('Album • Daft Punk • 2013'), 'Daft Punk');
assert.equal(creatorOf('Single • Daft Punk • 2023'), 'Daft Punk');
assert.equal(creatorOf('Song • Daft Punk & Julian Casablancas'), 'Daft Punk & Julian Casablancas');
assert.equal(creatorOf('Playlist • YouTube Music • 100 songs'), 'YouTube Music');
assert.equal(creatorOf('Daft Punk • 1.2B views • 4:25'), 'Daft Punk');
assert.equal(creatorOf('EP • Justice'), 'Justice');
assert.equal(creatorOf('722K views • 50 tracks • 3 hours, 30 minutes'), '');
assert.equal(creatorOf('Playlist • You'), 'You');
assert.equal(creatorOf(null), '');
// parity3: card subtitles
assert.equal(cardSubtitle('playlist', '722K views • 50 tracks • 3 hours, 30 minutes'), 'Playlist');
assert.equal(cardSubtitle('playlist', 'Playlist • Christian Isbej • 633 views', 'by'), 'By Christian Isbej');
assert.equal(cardSubtitle('playlist', 'Playlist • Christian Isbej • 633 views'), 'Playlist • Christian Isbej');
assert.equal(cardSubtitle('playlist', 'Playlist • You'), 'Playlist • You');
assert.equal(cardSubtitle('playlist', 'Chill beats for a rainy day'), 'Chill beats for a rainy day');
assert.equal(cardSubtitle('song', 'Daft Punk • 1.2B views • 4:25'), 'Daft Punk');
assert.equal(cardSubtitle('album', 'Daft Punk'), 'Daft Punk');
assert.equal(cardSubtitle('playlist', null), 'Playlist');
console.log('ok');
