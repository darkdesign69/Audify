<script lang="ts">
	/**
	 * Six digit boxes: typing moves on, Backspace moves back, arrows move, and a paste anywhere
	 * spreads the code over the boxes. Pasting a whole `spotless://pair` link hands it to `onLink`.
	 * The rules are pure functions in devices.ts (typeInto, backspace).
	 */
	import { onMount, tick } from 'svelte';
	import { backspace, codeComplete, isPairLink, typeInto } from '$lib/devices';

	let {
		boxes = $bindable(['', '', '', '', '', '']),
		disabled = false,
		invalid = false,
		onComplete,
		onLink
	}: {
		boxes?: string[];
		disabled?: boolean;
		invalid?: boolean;
		onComplete?: (code: string) => void;
		onLink?: (link: string) => void;
	} = $props();

	let inputs: HTMLInputElement[] = $state([]);

	async function focus(i: number) {
		await tick();
		inputs[i]?.focus();
		inputs[i]?.select();
	}

	function put(i: number, text: string) {
		const r = typeInto(boxes, i, text);
		boxes = r.boxes;
		focus(r.focus);
		if (codeComplete(boxes)) onComplete?.(boxes.join(''));
	}

	function oninput(i: number, e: Event) {
		const el = e.currentTarget as HTMLInputElement;
		const v = el.value;
		el.value = boxes[i] ?? '';
		put(i, v.replace(boxes[i] ?? '', '') || v);
	}

	function onkeydown(i: number, e: KeyboardEvent) {
		if (e.key === 'Backspace') {
			e.preventDefault();
			const r = backspace(boxes, i);
			boxes = r.boxes;
			focus(r.focus);
		} else if (e.key === 'ArrowLeft' && i > 0) {
			e.preventDefault();
			focus(i - 1);
		} else if (e.key === 'ArrowRight' && i < 5) {
			e.preventDefault();
			focus(i + 1);
		}
	}

	function onpaste(i: number, e: ClipboardEvent) {
		const text = e.clipboardData?.getData('text') ?? '';
		e.preventDefault();
		if (isPairLink(text)) return onLink?.(text.trim());
		put(i, text);
	}

	onMount(() => {
		const first = boxes.findIndex((b) => !b);
		focus(first < 0 ? 5 : first);
	});
</script>

<div class="flex items-center justify-center gap-2" role="group" aria-label="Six-digit code">
	{#each boxes as b, i (i)}
		{#if i === 3}<span class="w-2" aria-hidden="true"></span>{/if}
		<input
			bind:this={inputs[i]}
			value={b}
			{disabled}
			inputmode="numeric"
			autocomplete={i === 0 ? 'one-time-code' : 'off'}
			maxlength="6"
			aria-label="Digit {i + 1}"
			class="h-16 w-12 rounded-lg border-2 bg-bg-elevated-highlight text-center text-[28px] font-bold text-white caret-accent outline-none transition-colors
				{invalid ? 'border-text-negative' : b ? 'border-white/40' : 'border-transparent'} focus:border-accent disabled:opacity-60"
			oninput={(e) => oninput(i, e)}
			onkeydown={(e) => onkeydown(i, e)}
			onpaste={(e) => onpaste(i, e)}
			onfocus={(e) => (e.currentTarget as HTMLInputElement).select()}
		/>
	{/each}
</div>
