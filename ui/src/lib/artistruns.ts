// Clean artist runs for display. YouTube sends the joiners between collab names (", ", " & ",
// " and ", " x ") as runs of their own; the backend drops them now, but rows saved before that (the
// library, history, queues) still carry them, and every surface lists runs comma-separated. Left
// in, a credit read "Daft Punk, , , Pharrell Williams, & , Nile Rodgers".
//
// No imports beyond types: the self-check runs this file under plain Node.

export type Run = { text: string; id?: string | null };

const JOINERS = new Set(['and', 'x', 'feat', 'feat.', 'ft', 'ft.', 'featuring', 'with', 'vs', 'vs.']);

/** Text that only joins names: blank, punctuation, or a joining word. */
export function isSeparator(text: string | undefined | null): boolean {
	const t = (text ?? '').trim();
	if (!/[\p{L}\p{N}]/u.test(t)) return true;
	return JOINERS.has(t.toLowerCase());
}

/** The real names of a credit, trimmed. A linked run is always kept ("Simon & Garfunkel"). */
export function cleanRuns<R extends Run>(runs: R[] | undefined | null): R[] {
	return (runs ?? [])
		.filter((r) => !!r.id || !isSeparator(r.text))
		.map((r) => (r.text === r.text.trim() ? r : { ...r, text: r.text.trim() }));
}

/** Runs to render for a credit: the cleaned runs, else one run of the flat line (when non-empty). */
export function displayRuns(
	runs: Run[] | undefined | null,
	text: string | undefined | null,
	id?: string | null
): Run[] {
	const clean = cleanRuns(runs);
	if (clean.length) return clean;
	const t = (text ?? '').trim();
	return t ? [{ text: t, id: id ?? undefined }] : [];
}
