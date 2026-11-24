<script lang="ts">
	/**
	 * Podcast playback speed ("1x"), Spotify's player-bar button while an episode plays and the
	 * Now Playing episode card's control. Opens a kit menu: 0.5x … 3x, the current one checked.
	 * The speed is the spoken-word speed (`setTempoPitch(..., 'episode')`): the core keeps it apart
	 * from the music's (speeds.rs), so a 2x episode never makes the next song 2x.
	 * Accent while off 1x.
	 */
	import { playback, setTempoPitch } from '$lib/player.svelte';
	import { openMenu, type MenuEntry } from '$lib/kit/menus';
	import Tooltip from '$lib/kit/Tooltip.svelte';

	let { variant = 'bar' }: { variant?: 'bar' | 'pill' } = $props();

	const SPEEDS = [0.5, 0.8, 1, 1.2, 1.5, 1.8, 2, 2.5, 3];
	const label = (s: number) => `${+s.toFixed(2)}x`;
	const speed = $derived(playback.speed || 1);

	function open(e: MouseEvent) {
		const entries: MenuEntry[] = [
			{ type: 'heading', label: 'Playback speed' },
			...SPEEDS.map(
				(s): MenuEntry => ({
					label: label(s),
					checked: Math.abs(s - speed) < 0.001,
					onSelect: () => setTempoPitch(s, 0, playback.reverb, 'episode')
				})
			)
		];
		openMenu(e, entries, { side: variant === 'bar' ? 'top' : 'bottom', align: 'start', width: 160 });
	}
</script>

<Tooltip text="Playback speed">
	{#snippet trigger(props)}
		<button
			{...props}
			type="button"
			aria-label="Playback speed {label(speed)}"
			class="{variant === 'bar'
				? 'h-8 min-w-8 px-1 text-xs'
				: 'h-8 rounded-full bg-bg-tinted px-3 text-sm hover:bg-bg-tinted-highlight'} font-bold tabular-nums transition-transform hover:scale-[1.04] active:scale-100 {speed !==
			1
				? 'text-accent-text'
				: variant === 'bar'
					? 'text-text-subdued hover:text-white'
					: 'text-white'}"
			onclick={open}
		>
			{label(speed)}
		</button>
	{/snippet}
</Tooltip>
