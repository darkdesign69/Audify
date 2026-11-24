// Imperative confirm dialog (spec: overlays.md "Confirm dialog").
//
//   import { confirmDialog } from '$lib/kit/confirm.svelte';
//   if (await confirmDialog({ title: 'Delete from Your Library?', body: 'This will delete My Playlist #3 from Your Library.' })) …
//
// Requires <ConfirmHost/> (default export of ConfirmDialog.svelte) mounted once in the root layout.
// A second call while one is open resolves the first with `false`.

export type ConfirmOptions = {
	title: string;
	body?: string;
	confirmLabel?: string;
	cancelLabel?: string;
};

type Pending = Required<Pick<ConfirmOptions, 'title' | 'confirmLabel' | 'cancelLabel'>> & {
	body?: string;
	resolve: (ok: boolean) => void;
};

export const confirmState = $state<{ pending: Pending | null }>({ pending: null });

export function confirmDialog(opts: ConfirmOptions): Promise<boolean> {
	confirmState.pending?.resolve(false);
	return new Promise<boolean>((resolve) => {
		confirmState.pending = {
			title: opts.title,
			body: opts.body,
			confirmLabel: opts.confirmLabel ?? 'Delete',
			cancelLabel: opts.cancelLabel ?? 'Cancel',
			resolve
		};
	});
}

/** Settle the open confirm (used by the host). */
export function settleConfirm(ok: boolean) {
	const p = confirmState.pending;
	confirmState.pending = null;
	p?.resolve(ok);
}
