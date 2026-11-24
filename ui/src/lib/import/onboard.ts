// stream: onboard — the calm onboarding look shared by the first-run step (Welcome.svelte), its
// start screens (ImportStart.svelte) and the import dialog (ImportDialog.svelte): one big accent
// primary, an outlined second choice, a quiet text button. Same roles as the iOS Onboard* views.

export const PRIMARY =
	'flex min-h-12 w-full items-center justify-center gap-2 rounded-full bg-accent px-8 text-base font-bold text-black transition-transform duration-50 hover:scale-[1.02] hover:bg-accent-hover active:scale-100 disabled:pointer-events-none disabled:opacity-40';
export const SECONDARY =
	'flex min-h-12 w-full flex-col items-center justify-center rounded-full border border-border-control px-8 py-2 text-base font-bold text-white transition-transform duration-50 hover:scale-[1.02] hover:border-white active:scale-100';
export const QUIET =
	'mx-auto flex h-10 items-center justify-center px-4 text-sm font-bold text-text-subdued transition-colors hover:text-white';
export const TITLE = 'text-[28px] leading-tight font-bold text-white';
export const LINE = 'text-base text-text-subdued';
