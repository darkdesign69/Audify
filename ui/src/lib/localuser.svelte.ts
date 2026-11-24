// stream: parity3. Who "Made for <name>" names: the YouTube Music account when signed in, else
// the Mac user's first name (asked once), else "you".
import * as api from '$lib/api';
import { auth } from '$lib/player.svelte';

let macName = $state<string | null>(null);
let asked = false;

export function madeForName(): string {
	if (auth.account?.signedIn && auth.account.name) return auth.account.name;
	if (!asked) {
		asked = true;
		api.localUserName().then(
			(n) => (macName = n?.trim() || null),
			() => {}
		);
	}
	return macName ?? 'you';
}

/** The owner line of your own playlists (Spotify shows your user name, not "You"). */
export function ownerName(): string {
	const n = madeForName();
	return n === 'you' ? 'You' : n;
}
