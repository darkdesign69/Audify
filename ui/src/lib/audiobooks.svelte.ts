// stream: audiobooks — LibriVox books (crates/core/src/audiobooks). Types, commands, the saved
// list for Your Library, and the one play path. A book's chapters are podcast episodes, so rows,
// progress, resume, speed and downloads reuse the podcast UI.
import { invoke } from '@tauri-apps/api/core';
import type { PodcastEpisode, Shelf } from './api';
import * as api from './api';
import { openPlayer, playback } from './player.svelte';
import { podcasts } from './podcasts.svelte';

export interface Audiobook {
	/** Internet Archive identifier. */
	id: string;
	title: string;
	author: string;
	narrator?: string | null;
	thumbnail?: string | null;
	description?: string | null;
	durationSecs?: number | null;
	chapterCount?: number | null;
	genres: string[];
	language?: string | null;
	saved: boolean;
	/** 0..1 of the book heard; absent before any listening. */
	progress?: number;
}
export interface AudiobookResume {
	index: number;
	started: boolean;
	finished: boolean;
}
export interface AudiobookPage {
	book: Audiobook;
	chapters: PodcastEpisode[];
	resume: AudiobookResume;
	librivoxUrl?: string | null;
}

export const audiobookSearch = (query: string) => invoke<Audiobook[]>('audiobook_search', { query });
export const audiobookHome = () => invoke<Shelf[]>('audiobook_home');
export const audiobookGenre = (genre: string, page?: number) => invoke<Audiobook[]>('audiobook_genre', { genre, page });
export const audiobookPage = (id: string) => invoke<AudiobookPage>('audiobook_page', { id });
export const audiobookForChapter = (id: string) => invoke<AudiobookPage | null>('audiobook_for_chapter', { id });
export const audiobookSave = (book: Audiobook, saved: boolean) => invoke<boolean>('audiobook_save', { book, saved });
export const audiobookSaved = () => invoke<Audiobook[]>('audiobook_saved');

export const bookHref = (id: string) => `/audiobook/${encodeURIComponent(id)}`;
export const genreHref = (subject: string, label: string) =>
	`/browse/audiobooks?genre=${encodeURIComponent(subject)}&title=${encodeURIComponent(label)}`;
/** Chapters are stored under this show id (core `audiobooks::SHOW_PREFIX`). */
export const isBookChapter = (ep: { showId?: string | null }) => !!ep.showId?.startsWith('abook:');

export const books = $state({ saved: [] as Audiobook[], loaded: false });

export async function reloadSavedBooks() {
	try {
		books.saved = await audiobookSaved();
	} finally {
		books.loaded = true;
	}
}

export const isBookSaved = (id: string) => books.saved.some((b) => b.id === id);

/** Add to / remove from Your Library; returns the new state. */
export async function toggleBookSaved(book: Audiobook): Promise<boolean> {
	const save = !isBookSaved(book.id);
	await audiobookSave(book, save);
	books.saved = save ? [{ ...book, saved: true }, ...books.saved] : books.saved.filter((b) => b.id !== book.id);
	return save;
}

/** The book is the playing context (any of its chapters is current). */
export const isBookPlaying = (page: AudiobookPage | null) =>
	!!page && page.chapters.some((c) => c.id === playback.now?.videoId);

/** Play the book from chapter `index` (default: where it was left). The chapter's saved
 *  position resumes in Rust, like any episode. The book is the queue and the context. */
export function playBook(page: AudiobookPage, index = page.resume.index) {
	const ch = page.chapters[index];
	if (!ch) return;
	if (playback.now?.videoId === ch.id) return api.togglePause();
	openPlayer();
	return api.playPlaylistFrom(page.chapters.map(api.episodeSong), index, {
		kind: 'audiobook',
		id: page.book.id,
		title: page.book.title,
		subtitle: page.book.author,
		thumbnail: page.book.thumbnail ?? undefined
	});
}

/** Play a book from a card (fetches its page first). */
export async function playBookById(id: string) {
	const page = await audiobookPage(id);
	return playBook(page);
}

/** Live progress of a chapter row: the podcast store's event data, else the loaded row. */
export const chapterProgress = (ep: PodcastEpisode) => podcasts.progress[ep.id] ?? ep.progress;
