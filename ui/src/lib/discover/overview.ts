// One `searchOverview` per query for the top bar's dropdown and the results page (stream:
// search-dropdown). Both ask at the same moment, 250 ms after typing settles; they share the request
// and its answer, so the dropdown and the page always show the same search.
import * as api from '$lib/api';
import type { SearchOverview } from '$lib/api';
import { getCached, putCached } from '$lib/pagecache';
import { sharedByKey } from '$lib/lastwins';

const key = (q: string) => `search:ov:${q}`;
const fetchOnce = sharedByKey((q) =>
	api.searchOverview(q, false).then((o) => {
		putCached(key(q), o);
		return o;
	})
);

/** The answer if it is already here (no request). */
export const overviewNow = (q: string) => getCached<SearchOverview>(key(q));

export const overview = (q: string): Promise<SearchOverview> => {
	const hit = overviewNow(q);
	return hit ? Promise.resolve(hit) : fetchOnce(q);
};
