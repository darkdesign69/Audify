<script lang="ts">
	/**
	 * The main window's half of "Sign in to YouTube Music": a dimmed sheet behind the Google
	 * sign-in window (attached to this one, see src-tauri/src/session.rs), saying what is going on
	 * and offering Cancel. Closes itself once Spotless is signed in; shows the reason if it isn't.
	 */
	import Modal from '$lib/kit/Modal.svelte';
	import { cancelSignIn, startSignIn, ui } from '$lib/player.svelte';

	let open = $state(false);
	$effect(() => {
		open = ui.signIn.kind !== 'idle';
	});
</script>

<!-- Bottom-left: the sign-in window sits over the middle of this one. -->
<Modal
	bind:open
	title="Sign in to YouTube Music"
	width={360}
	panelClass="!top-auto !bottom-6 !left-6 !translate-x-0 !translate-y-0"
	onClose={cancelSignIn}
>
	{#if ui.signIn.kind === 'failed'}
		<p class="text-sm text-text-subdued">Spotless isn't signed in.</p>
		<p class="mt-2 text-sm text-white">{ui.signIn.message}</p>
	{:else}
		<div class="flex items-center gap-3">
			<span
				class="size-5 shrink-0 animate-spin rounded-full border-2 border-white/25 border-t-white"
				aria-hidden="true"
			></span>
			<p class="text-sm text-white">Signing in…</p>
		</div>
		<p class="mt-3 text-sm text-text-subdued">
			Sign in to Google in the window in front. Spotless picks up your YouTube Music account as soon
			as you're in, and closes it.
		</p>
	{/if}
	{#snippet footer()}
		{#if ui.signIn.kind === 'failed'}
			<button class="btn-outline" onclick={cancelSignIn}>Close</button>
			<button class="btn-white" onclick={() => startSignIn()}>Try again</button>
		{:else}
			<button class="btn-outline" onclick={() => startSignIn()}>Show sign-in window</button>
			<button class="btn-white" onclick={cancelSignIn}>Cancel</button>
		{/if}
	{/snippet}
</Modal>
