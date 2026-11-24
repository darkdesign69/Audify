// DEV ONLY (docs/streams/devmock.md). The captured backend responses in ./fixtures, by file name
// (without `.json`). Real data from `src-tauri/src/devmock_capture.rs`, never hand-written.
/* eslint-disable @typescript-eslint/no-explicit-any */
const files = import.meta.glob('./fixtures/*.json', { eager: true, import: 'default' });

export const fx: Record<string, any> = Object.fromEntries(
	Object.entries(files).map(([path, data]) => [path.replace(/^.*\/(.*)\.json$/, '$1'), data])
);

/** A fixture by name, deep-copied so the caller may mutate it. */
export const get = <T = any>(name: string): T => structuredClone(fx[name]);
