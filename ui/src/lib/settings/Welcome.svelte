<script lang="ts">
	/**
	 * First run (empty library): "Bring your music from Spotify?", the one optional onboarding step
	 * (stream import2; redesigned by stream onboard to match iOS: one idea per screen, one big
	 * button). The screens themselves are `import/ImportStart.svelte`; a read hands over to the
	 * import dialog (pick → matching → review → done). Shown once; "Not now" or any read marks it
	 * done (the core's `import_onboarding_done`). Settings › Import library and the avatar menu bring
	 * the same screens back.
	 */
	import Modal from '$lib/kit/Modal.svelte';
	import { lib } from '$lib/library.svelte';
	import { imp } from '$lib/import.svelte';
	import ImportStart from '$lib/import/ImportStart.svelte';
	import { importOffer, importOfferDone } from '$lib/api';
	import { uiPrefs } from './prefs.svelte';
	import { offerWelcome } from '$lib/devices.svelte';

	let open = $state(false);

	// Decide once, when the library's first snapshot is in. (`?welcome` in the dev preview.) The
	// rule is the core's, shared with iOS: an empty library and `import_onboarding_done` unset.
	let decided = false;
	$effect(() => {
		if (decided || !lib.ready) return;
		decided = true;
		if (import.meta.env.DEV && location.search.includes('welcome')) {
			open = true;
			return;
		}
		// Answered before the core kept it (the old localStorage flag): carry that over.
		if (uiPrefs.onboarded) return void importOfferDone().catch(() => {});
		importOffer()
			.then((offer) => {
				if (offer && !imp.snapshot) open = true;
			})
			.catch(() => {});
	});

	function done() {
		open = false;
		importOfferDone().catch(() => {});
		offerWelcome(); // "Using Spotless on your iPhone too?" comes next, once the import is through
	}

	/** Close this and let the import dialog take over, reading what was handed in. */
	function handOver(go: () => void) {
		done();
		imp.preview = null;
		imp.open = true;
		go();
	}
</script>

<Modal bind:open width={480} onClose={done}>
	<ImportStart read={handOver} onNotNow={done} />
</Modal>
