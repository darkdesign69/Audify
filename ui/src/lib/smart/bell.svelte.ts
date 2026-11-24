/** The What's New bell's "New" dot (stream smart2): the count of items since the page was last
 *  opened, kept fresh by the backend's `whats-new-changed` event and a 30-minute poll. */
import * as api from '$lib/api';

export const bell = $state({ count: 0 });

let started = false;
export function initBell() {
	if (started) return;
	started = true;
	const poll = () => api.whatsNewCount().then((n) => (bell.count = n)).catch(() => {});
	void poll();
	void api.onWhatsNewChanged((n) => (bell.count = n));
	setInterval(poll, 30 * 60_000);
}
