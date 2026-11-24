// Self-check for titled() (discover/format.ts): Home never renders a card without a title.
import { titled } from './discover/format.ts';
import type { Shelf } from './api.ts';

const shelf = (items: Shelf['items']): Shelf[] => [
	{ id: 'shortcuts', title: '', layout: 'shortcuts', source: 'local', items }
];
const out = titled(
	shelf([
		{ kind: 'radio', id: 'dQw4w9WgXcQ', title: '', subtitle: 'Rick Astley', explicit: false },
		{ kind: 'song', id: 's', title: ' ', explicit: false, song: { title: 'One More Time' } as never },
		{ kind: 'album', id: 'a', title: 'Discovery', explicit: false }
	])
)[0].items.map((i) => i.title);
if (JSON.stringify(out) !== JSON.stringify(['One More Time', 'Discovery']))
	throw new Error(`FAIL: ${JSON.stringify(out)}`);
console.log('ok');
