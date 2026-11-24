// Self-check for the Now Playing header's context link (`contextlink.ts`, lane magic-d6).
import { cardMatches, contextHref, songRadioHref } from './contextlink.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}
const eq = (a: unknown, b: unknown, what: string) => ok(a === b, `${what}: got ${String(a)}, want ${String(b)}`);

eq(contextHref({ kind: 'liked', id: 'LM', title: 'Liked Songs' }), '/playlist/VLLM', 'liked');
eq(contextHref({ kind: 'album', id: 'MPREb_1', title: 'OK Computer' }), '/album/MPREb_1', 'album');
eq(contextHref({ kind: 'artist', id: 'UC1', title: 'Radiohead' }), '/artist/UC1', 'artist');
eq(contextHref({ kind: 'playlist', id: 'VLPL1', title: 'Road Trip' }), '/playlist/VLPL1', 'playlist');
eq(contextHref({ kind: 'podcast', id: 'pod:1', title: 'Show' }), '/show/pod%3A1', 'podcast');
eq(contextHref({ kind: 'audiobook', id: 'emma_librivox', title: 'Emma' }), '/audiobook/emma_librivox', 'audiobook (stream: audiobooks)');
eq(
	contextHref({ kind: 'radio', id: 'abc', title: 'Creep Radio', thumbnail: 'https://i/x.jpg' }),
	'/radio/song/abc?name=Creep&thumb=https%3A%2F%2Fi%2Fx.jpg',
	'song radio opens its page, name without " Radio"'
);
eq(contextHref({ kind: 'radio', id: 'MPREb_1', title: 'OK Computer Radio', params: 'album' }), '/radio/album/MPREb_1?name=OK+Computer', 'album radio');
eq(contextHref({ kind: 'mix', id: 'UC1', title: 'Radiohead Radio' }), '/radio/artist/UC1?name=Radiohead', 'artist radio (mix)');
eq(contextHref({ kind: 'daily_mix', id: 'dm1', title: 'Your Mix 1' }), '/mix/dm1?title=Your+Mix+1', 'mix');
eq(contextHref({ kind: 'mood', id: 'x', title: 'Chill' }), null, 'unknown kinds have no page');
eq(contextHref({ kind: 'album', id: '', title: 'x' }), null, 'no id, no link');

ok(cardMatches({ kind: 'radio', id: 'a', title: 'Creep Radio' }, 'creep radio '), 'titles match loosely');
ok(!cardMatches({ kind: 'radio', id: 'a', title: 'Creep Radio' }, 'Karma Police Radio'), 'a stale card does not');
ok(!cardMatches(null, 'x') && !cardMatches({ kind: 'album', id: 'a', title: 'x' }, null), 'nothing to match');

const items = [
	{ video_id: 'v0', title: 'Airbag' },
	{ video_id: 'v1', title: 'Creep', thumbnail: null },
	{ video_id: 'v2', title: 'Lucky' }
];
eq(songRadioHref('Creep Radio', items), '/radio/song/v1?name=Creep', 'the seed row names the radio');
eq(songRadioHref('Thom Radio', items), null, 'no row, no guess');
eq(songRadioHref('Creep', items), null, 'not a radio');

console.log('ok');
