<script lang="ts">
	/**
	 * <ConfirmHost/>: renders the pending `confirmDialog()` (see confirm.svelte.ts). Mount once in
	 * the root layout. Look: modal spec (backdrop .7, #282828, radius 8, padding 24), title 24/700,
	 * body 14px #b3b3b3, right-aligned "Cancel" text button (16/700 white) + white 48px pill.
	 * Enter confirms (focus starts on the confirm button); Esc / backdrop cancel.
	 */
	import { AlertDialog } from 'bits-ui';
	import { confirmState, settleConfirm } from './confirm.svelte';

	const p = $derived(confirmState.pending);
	let open = $derived(!!p);
</script>

<AlertDialog.Root
	bind:open
	onOpenChange={(o) => {
		if (!o && confirmState.pending) settleConfirm(false);
	}}
>
	<AlertDialog.Portal>
		<AlertDialog.Overlay
			class="fixed inset-0 z-[1050] bg-scrim data-[state=open]:animate-in data-[state=open]:fade-in-0"
			onclick={() => settleConfirm(false)}
		/>
		<AlertDialog.Content
			class="fixed top-1/2 left-1/2 z-[1050] w-[420px] max-w-[calc(100vw-32px)] -translate-x-1/2 -translate-y-1/2 rounded-lg bg-bg-menu p-6 text-white shadow-modal outline-none data-[state=open]:animate-in data-[state=open]:fade-in-0"
		>
			{#if p}
				<AlertDialog.Title class="text-xl font-bold text-white">{p.title}</AlertDialog.Title>
				{#if p.body}
					<AlertDialog.Description class="mt-2 text-sm text-text-subdued">
						{p.body}
					</AlertDialog.Description>
				{/if}
				<div class="mt-8 flex items-center justify-end gap-6">
					<AlertDialog.Cancel
						class="text-base font-bold text-white transition-transform duration-50 hover:scale-[1.04]"
						onclick={() => settleConfirm(false)}
					>
						{p.cancelLabel}
					</AlertDialog.Cancel>
					<AlertDialog.Action class="btn-white" onclick={() => settleConfirm(true)}>
						{p.confirmLabel}
					</AlertDialog.Action>
				</div>
			{/if}
		</AlertDialog.Content>
	</AlertDialog.Portal>
</AlertDialog.Root>
