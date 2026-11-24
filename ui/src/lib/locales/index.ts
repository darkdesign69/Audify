// Catalogs are plain JSON. English is the source of truth and, for now, the only catalog: the i18n
// system (`t()`, typed keys, per-key fallback to English) stays so more languages can be added back.
import en from './en.json';

export type Translations = typeof en;

/** A catalog that has not been fully translated yet: every key optional, all the way down. */
type DeepPartial<T> = { [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K] };

export type LocaleId = 'en';

export interface LocaleInfo {
	id: LocaleId;
	/** Shown in the language picker, in the language itself. */
	nativeLabel: string;
	/** The same language named in English. */
	englishLabel: string;
}

export const LOCALES: LocaleInfo[] = [{ id: 'en', nativeLabel: 'English', englishLabel: 'English' }];

// The id is also what goes to YouTube as `hl` (#274): adding a locale means checking its id
// against music.youtube.com, not just landing the catalog.
export const translations: Record<LocaleId, DeepPartial<Translations>> = { en };
