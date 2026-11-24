// Self-check for the current-chapter rule (stream: chapters).
//
//     node --experimental-strip-types ui/src/lib/chapters.check.ts
import { chapterAt } from './chapters.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

const ch = (startSecs: number, title: string) => ({ startSecs, title, startLabel: '' });
const list = [ch(10, 'A'), ch(60, 'B'), ch(300, 'C')];
ok(chapterAt(list, 0) === -1, 'before the first chapter');
ok(chapterAt(list, 10) === 0, 'at a start');
ok(chapterAt(list, 59.5) === 0, 'just before the next');
ok(chapterAt(list, 59.9) === 1, 'a seek that lands a hair early counts as the new chapter');
ok(chapterAt(list, 5000) === 2, 'the last runs to the end');
ok(chapterAt([], 5) === -1, 'no chapters');

console.log('ok');
