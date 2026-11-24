// stream: audiobooks — a book's chapters are episodes of the show `abook:<id>`; every "go to
// show" link (episode rows, the episode page, Now Playing) lands on the book page instead.
import { redirect } from '@sveltejs/kit';

export function load({ params }: { params: { id: string } }) {
	const id = decodeURIComponent(params.id ?? '');
	if (id.startsWith('abook:')) redirect(307, `/audiobook/${encodeURIComponent(id.slice(6))}`);
}
