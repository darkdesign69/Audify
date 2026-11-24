// Self-check for Home's live updates (`homelive.ts`, stream magic-d4):
//
//     node --experimental-strip-types ui/src/lib/homelive.check.ts
import { contextTile, mergeLocal, promoteShortcut, recentsKey, type HomeShelfLike } from './homelive.ts';

const eq = (a: unknown, b: unknown, what: string) => {
	if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`);
};
const it = (kind: string, id: string, title = id) => ({ kind, id, title });
const shelf = (id: string, layout: string, items: ReturnType<typeof it>[], source = 'local'): HomeShelfLike =>
	({ id, title: id, layout, source, items });
const ids = (s: HomeShelfLike[]) => s.map((x) => x.id);
const tiles = (s: HomeShelfLike[]) => s.find((x) => x.layout === 'shortcuts')!.items.map((i) => i.id);

// The payload's context becomes a tile; Liked Songs and untitled contexts don't move anything.
eq(contextTile({ kind: 'playlist', id: 'VLPLa', title: 'Road trip', thumbnail: 'x' }), { kind: 'playlist', id: 'VLPLa', title: 'Road trip', explicit: false, thumbnail: 'x' }, 'tile');
eq(contextTile({ kind: 'liked', id: 'VLLM', title: 'Liked Songs' }), null, 'liked');
eq(contextTile({ kind: 'radio', id: 'abc', title: ' ' }), null, 'untitled');
eq(contextTile(undefined), null, 'none');

// Pressing play on the 4th tile moves it right behind Liked Songs; no duplicates, max 8.
const home = [
	shelf('shortcuts', 'shortcuts', [it('liked', 'liked'), it('album', 'A'), it('artist', 'B'), it('playlist', 'C')]),
	shelf('yt:1', 'cards', [it('album', 'Y')], 'youtube'),
	shelf('recently_played', 'cards', [it('song', 's1')])
];
const moved = promoteShortcut(home, it('playlist', 'C', 'New title'));
eq(tiles(moved), ['liked', 'C', 'A', 'B'], 'moved behind liked');
eq(moved[0].items[1].title, 'New title', 'title refreshed');
eq(tiles(home), ['liked', 'A', 'B', 'C'], 'input untouched');
// A new context with no Liked Songs leads; the grid stays 8.
const eight = [shelf('shortcuts', 'shortcuts', 'abcdefgh'.split('').map((x) => it('album', x)))];
const added = promoteShortcut(eight, it('playlist', 'Z'));
eq(tiles(added), ['Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g'], 'new first, capped');
// No grid yet: one is made.
eq(tiles(promoteShortcut([shelf('yt:1', 'cards', [])], it('album', 'Q'))), ['Q'], 'grid created');

// The local revalidate replaces local shelves in place and keeps YouTube's.
const local = [
	shelf('shortcuts', 'shortcuts', [it('playlist', 'C'), it('album', 'A')]),
	shelf('recently_played', 'cards', [it('song', 's2'), it('song', 's1')]),
	shelf('top_mixes', 'cards', [it('mix', 'UC1')])
];
const merged = mergeLocal(home, local);
eq(ids(merged), ['shortcuts', 'yt:1', 'recently_played', 'top_mixes'], 'merge order');
eq(merged[2].items.map((i) => i.id), ['s2', 's1'], 'fresh recents');
// A local shelf that came back empty goes; a missing grid comes back first.
eq(ids(mergeLocal(home, [local[1]])), ['yt:1', 'recently_played'], 'dropped grid');
eq(ids(mergeLocal([shelf('yt:1', 'cards', [])], local)), ['shortcuts', 'recently_played', 'top_mixes', 'yt:1'], 'grid back first');

// Recents: the later of played / touched, ms or s.
eq(recentsKey(100, 300), 300, 'touched wins');
eq(recentsKey(500_000, null, undefined), 500_000, 'played wins');
eq(recentsKey(1_700_000_000_000), 1_700_000_000, 'ms');
eq(recentsKey(), 0, 'nothing');

console.log('ok');
