<script lang="ts">
	/**
	 * Spotify modal (spec: overlays.md "Modal — Edit details"). bits-ui Dialog; backdrop
	 * rgba(0,0,0,.7); #282828 panel, radius 8, padding 24, title 24/700, 32px close X top-right.
	 * Esc / X / backdrop click = cancel (`onClose`).
	 *
	 * Props
	 *   open: boolean (bindable)
	 *   title?: string
	 *   width?: number          px, default 524
	 *   panelClass?: string     extra classes on the panel (e.g. 'bg-bg-elevated' for the shortcuts sheet)
	 *   onClose?: () => void    called on any dismissal (after open=false)
	 *   closeButton?: boolean   default true
	 *   children: Snippet       body
	 *   footer?: Snippet        right-aligned row under the body
	 *
	 * Usage
	 *   <Modal bind:open title="Edit details">…{#snippet footer()}<button class="btn-white">Save</button>{/snippet}</Modal>
	 */
	import type { Snippet } from 'svelte';
	import { Dialog } from 'bits-ui';
	import Icon from './Icon.svelte';
	import XIcon from 'phosphor-svelte/lib/XIcon';

	let {
		open = $bindable(false),
		title,
		width = 524,
		panelClass = '',
		onClose,
		closeButton = true,
		children,
		footer
	}: {
		open?: boolean;
		title?: string;
		width?: number;
		panelClass?: string;
		onClose?: () => void;
		closeButton?: boolean;
		children: Snippet;
		footer?: Snippet;
	} = $props();
	let panel = $state<HTMLElement | null>(null);
</script>

<Dialog.Root
	bind:open
	onOpenChange={(o) => {
		if (!o) onClose?.();
	}}
>
	<Dialog.Portal>
		<Dialog.Overlay
			class="fixed inset-0 z-[1050] bg-scrim data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=closed]:animate-out data-[state=closed]:fade-out-0"
		/>
		<Dialog.Content
			bind:ref={panel}
			onOpenAutoFocus={(e) => {
				// Spotify opens with no ring on the X: focus the first field (Edit details' Name),
				// else the panel itself, never the close button.
				e.preventDefault();
				const field = panel?.querySelector<HTMLElement>('input:not([type=hidden]), textarea, select');
				(field ?? panel)?.focus({ preventScroll: true });
			}}
			tabindex={-1}
			class="fixed top-1/2 left-1/2 z-[1050] flex max-h-[calc(100vh-48px)] max-w-[calc(100vw-32px)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg bg-bg-menu p-6 text-white shadow-modal outline-none data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-[0.98] data-[state=closed]:animate-out data-[state=closed]:fade-out-0 {panelClass}"
			style="width:{width}px"
		>
			{#if title || closeButton}
				<div class="mb-6 flex items-start justify-between gap-4">
					{#if title}
						<Dialog.Title class="text-xl font-bold text-white">{title}</Dialog.Title>
					{:else}
						<span></span>
					{/if}
					{#if closeButton}
						<Dialog.Close
							class="-mt-1 -mr-2 grid size-8 shrink-0 place-items-center rounded-full text-text-subdued transition-[color,transform] duration-100 hover:scale-[1.04] hover:text-white"
							aria-label="Close"
						>
							<Icon icon={XIcon} size={18} />
						</Dialog.Close>
					{/if}
				</div>
			{/if}
			<div class="min-h-0 overflow-y-auto">{@render children()}</div>
			{#if footer}
				<div class="mt-6 flex items-center justify-end gap-6">{@render footer()}</div>
			{/if}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>
