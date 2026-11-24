<script lang="ts">
	/** The "E" explicit badge (spec: 16x16, radius ~3, 10-12px/700, translucent white box, light
	 *  glyph). Put it before the artist line, 4px gap. */
	let { class: className = '' }: { class?: string } = $props();
</script>

<span
	class="inline-grid size-4 shrink-0 place-items-center rounded-[3px] bg-white/15 text-[10px] leading-none font-bold text-white/85 {className}"
	role="img"
	aria-label="Explicit"
	title="Explicit">E</span
>
