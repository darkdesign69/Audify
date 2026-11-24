// DEV ONLY: in a plain browser (`pnpm dev`, no Tauri) install the mock backend before any route
// module loads, so every `invoke`/`listen` has an answer (src/lib/devmock, docs/streams/devmock.md).
// `import.meta.env.DEV` is a build-time constant: production builds drop the branch and the whole
// devmock chunk with it.
export async function init() {
	if (import.meta.env.DEV && !('__TAURI_INTERNALS__' in window)) {
		const { installDevMock } = await import('$lib/devmock');
		installDevMock();
	}
}
