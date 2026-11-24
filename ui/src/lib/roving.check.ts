// Self-check for the arrow-key arithmetic behind kit/roving.ts (`node src/lib/roving.check.ts`).
import { nextIndex } from './kit/roving.ts';

const eq = (got: number, want: number, what: string) => {
	if (got !== want) throw new Error(`${what}: expected ${want}, got ${got}`);
};
// vertical list of 5
eq(nextIndex('ArrowDown', 0, 5, 'vertical'), 1, 'down');
eq(nextIndex('ArrowDown', 4, 5, 'vertical'), 4, 'down clamps at end');
eq(nextIndex('ArrowUp', 0, 5, 'vertical'), 0, 'up clamps at start');
eq(nextIndex('ArrowRight', 2, 5, 'vertical'), -1, 'right ignored in a vertical list');
eq(nextIndex('Home', 3, 5, 'vertical'), 0, 'home');
eq(nextIndex('End', 1, 5, 'vertical'), 4, 'end');
eq(nextIndex('a', 1, 5, 'vertical'), -1, 'other keys ignored');
// horizontal (shelf / chips)
eq(nextIndex('ArrowRight', 2, 5, 'horizontal'), 3, 'right');
eq(nextIndex('ArrowLeft', 2, 5, 'horizontal'), 1, 'left');
eq(nextIndex('ArrowDown', 2, 5, 'horizontal'), -1, 'down ignored in a row');
// grid of 3 columns, 8 items
eq(nextIndex('ArrowDown', 1, 8, 'grid', 3), 4, 'grid down = next row');
eq(nextIndex('ArrowDown', 6, 8, 'grid', 3), 7, 'grid down past the end clamps');
eq(nextIndex('ArrowUp', 4, 8, 'grid', 3), 1, 'grid up');
eq(nextIndex('ArrowRight', 2, 8, 'grid', 3), 3, 'grid right wraps to the next row');
console.log('ok');
