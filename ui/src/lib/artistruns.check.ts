// Self-check for `artistruns.ts`, run by `pnpm test`. Prints "ok" or throws.
import { cleanRuns, displayRuns, isSeparator } from './artistruns.ts';

function eq(got: unknown, want: unknown, what: string): void {
	const g = JSON.stringify(got);
	const w = JSON.stringify(want);
	if (g !== w) throw new Error(`FAIL: ${what}: got ${g}, want ${w}`);
}

const names = (runs: { text: string }[]) => runs.map((r) => r.text);

eq(
	names(
		cleanRuns([
			{ text: 'Daft Punk', id: 'UC1' },
			{ text: ', ' },
			{ text: ', ' },
			{ text: 'Pharrell Williams', id: 'UC2' },
			{ text: ', & ' },
			{ text: 'Nile Rodgers' }
		])
	),
	['Daft Punk', 'Pharrell Williams', 'Nile Rodgers'],
	'collab with separator runs'
);
eq(names(cleanRuns([{ text: 'Aaron Smith', id: 'a' }, { text: ' & ' }, { text: 'Krono', id: 'k' }])), ['Aaron Smith', 'Krono'], '& joiner');
eq(names(cleanRuns([{ text: 'Simon & Garfunkel', id: 'sg' }])), ['Simon & Garfunkel'], 'linked name with &');
eq(names(cleanRuns([{ text: 'A', id: 'a' }, { text: ' x ' }, { text: 'B', id: 'b' }, { text: ' and ' }, { text: 'C' }])), ['A', 'B', 'C'], 'word joiners');
for (const s of ['', ' ', ', ', ' & ', ' • ', ' and ', ' X ', ' feat. ', ' / ']) eq(isSeparator(s), true, `separator ${JSON.stringify(s)}`);
for (const s of ['X Ambassadors', 'Ex', 'Andrés', '2Pac', 'Sigur Rós']) eq(isSeparator(s), false, `name ${s}`);
eq(displayRuns([{ text: ' , ' }], 'Daft Punk', 'UC1'), [{ text: 'Daft Punk', id: 'UC1' }], 'all junk falls back to text');
eq(displayRuns(undefined, '  ', undefined), [], 'nothing at all');
eq(displayRuns([], 'Delara'), [{ text: 'Delara' }], 'no runs');
console.log('ok');
