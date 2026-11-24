// Library sidebar UI state (library-sidebar.md): filter chip, search, sort + view per filter,
// open folder. Sort/view are remembered per filter in localStorage.
import { browser } from '$app/environment';

export type LibFilter = 'playlist' | 'artist' | 'album' | 'podcast' | 'audiobook'; // audiobook: stream audiobooks
export type LibSort = 'recents' | 'added' | 'alpha' | 'creator' | 'custom';
export type LibView = 'compact' | 'list' | 'compact-grid' | 'grid';

const KEY = 'encore:library-ui';

type Prefs = Record<string, { sort: LibSort; view: LibView }>;
function loadPrefs(): Prefs {
	if (!browser) return {};
	try {
		return JSON.parse(localStorage.getItem(KEY) ?? '{}') ?? {};
	} catch {
		return {};
	}
}

export const libraryUi = $state({
	filter: null as LibFilter | null,
	/** Sub-filter under Playlists: "By you". */
	byYou: false,
	/** stream: downloads — the "Downloaded" chip (combines with a type filter). */
	downloaded: false,
	query: '',
	searchOpen: false,
	/** Folder stack: the folder opened in place (last = current). */
	folderPath: [] as string[],
	prefs: loadPrefs(),
	/** Bumped by ⇧⌘⌥F to focus the search field. */
	searchSeq: 0
});

const prefKey = () => (libraryUi.folderPath.length ? 'folder' : (libraryUi.filter ?? 'all'));

export const currentSort = (): LibSort =>
	libraryUi.prefs[prefKey()]?.sort ?? (libraryUi.folderPath.length ? 'custom' : 'recents');
export const currentView = (): LibView => libraryUi.prefs[prefKey()]?.view ?? 'list';

function save() {
	if (browser) localStorage.setItem(KEY, JSON.stringify(libraryUi.prefs));
}
export function setSort(sort: LibSort) {
	libraryUi.prefs[prefKey()] = { sort, view: currentView() };
	save();
}
export function setView(view: LibView) {
	libraryUi.prefs[prefKey()] = { sort: currentSort(), view };
	save();
}
export function setFilter(f: LibFilter | null) {
	libraryUi.filter = f;
	libraryUi.byYou = false;
	if (f === null) libraryUi.downloaded = false; // stream: downloads
	libraryUi.folderPath = [];
}
export function openSearch() {
	libraryUi.searchOpen = true;
	libraryUi.searchSeq++;
}
