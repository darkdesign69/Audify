// `/artist/<id>/discography` (the album page's "See discography") is Spotify's `…/discography/all`.
import { redirect } from '@sveltejs/kit';
import type { PageLoad } from './$types';

export const load: PageLoad = ({ params }) => {
	redirect(307, `/artist/${encodeURIComponent(params.id)}/discography/all`);
};
