// Two small rules for typed-ahead requests (stream: search-dropdown).

/** The last request wins: `take()` before each request, and drop the answer when its `current()`
 *  is false by the time it arrives (a slower, older query must not overwrite a newer one). */
export function lastWins() {
	let n = 0;
	return {
		take() {
			const my = ++n;
			return { current: () => my === n };
		},
		/** Nothing in flight counts any more (the field was emptied). */
		drop() {
			n++;
		}
	};
}

/** One request per key at a time: callers asking for the same key while it is in flight share it. */
export function sharedByKey<T>(fetch: (key: string) => Promise<T>): (key: string) => Promise<T> {
	const inflight = new Map<string, Promise<T>>();
	return (key) => {
		let p = inflight.get(key);
		if (!p) {
			p = fetch(key).finally(() => inflight.delete(key));
			inflight.set(key, p);
		}
		return p;
	};
}
