<script lang="ts">
	import { Toaster } from 'svelte-sonner';
	/** `bottom` offset: Spotify's toast ends 4px below the main view's bottom edge (measured on
	 *  open.spotify.com: a 251x48 box, padding 12, radius 6, 16px text). */
	let { offset = 84 }: { offset?: number } = $props();
</script>

<Toaster
	position="bottom-center"
	visibleToasts={1}
	gap={8}
	offset={{ bottom: offset }}
	toastOptions={{
		unstyled: true,
		classes: {
			toast:
				'encore-toast pointer-events-auto flex min-h-12 max-w-[480px] items-center gap-4 rounded-md bg-white p-3 text-base text-black shadow-[0_4px_12px_rgba(0,0,0,.3)]',
			title: 'font-normal leading-6',
			actionButton:
				'ml-auto shrink-0 text-base font-bold text-black hover:scale-[1.04] transition-transform duration-50',
			icon: 'hidden'
		}
	}}
/>

<style>
	:global(.encore-toast[data-mounted='true']) {
		animation: encore-toast-in 250ms cubic-bezier(0, 0, 0.2, 1);
	}
	/* sonner lays toasts out left-aligned in its 356px list; Spotify's fit their text and sit
	   centred (bottom centre of the main view, which is the window centre at the default layout). */
	:global([data-sonner-toaster][data-x-position='center'] .encore-toast) {
		left: 0;
		right: 0;
		width: fit-content;
		margin-inline: auto;
	}
	:global(.encore-toast-error) {
		background: #1278f2 !important;
		color: #fff !important;
	}
	:global(.encore-toast-error button) {
		color: #fff !important;
	}
	@keyframes encore-toast-in {
		from {
			opacity: 0;
			transform: translateY(8px);
		}
	}
</style>
