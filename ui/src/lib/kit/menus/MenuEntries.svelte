<script lang="ts">
	/**
	 * Renders MenuEntry[] inside a bits-ui DropdownMenu/ContextMenu Content or SubContent.
	 * Spotify look (menus.md): items 40px, padding 8/8/8/12, gap 12, radius 2, 14px white/90,
	 * 16px icons; hover/focus bg white/10 + white text; checked = accent + check at the right;
	 * submenu caret at the right; separators 1px white/10; headings 12/700 white/70.
	 */
	import { DropdownMenu } from 'bits-ui';
	import Icon from '../Icon.svelte';
	import CaretRightIcon from 'phosphor-svelte/lib/CaretRightIcon';
	import CheckIcon from 'phosphor-svelte/lib/CheckIcon';
	import PlusIcon from 'phosphor-svelte/lib/PlusIcon';
	import type { MenuEntry } from './types';
	import PlaylistPicker from './PlaylistPicker.svelte';
	import SavedInPicker from './SavedInPicker.svelte';
	import Self from './MenuEntries.svelte';
	import { closeMenu } from './state.svelte';

	let { entries }: { entries: MenuEntry[] } = $props();

	const ITEM =
		'flex h-10 w-full cursor-default items-center gap-3 rounded-[2px] py-2 pr-2 pl-3 text-left text-sm text-white/90 outline-none select-none data-highlighted:bg-white/10 data-highlighted:text-white data-disabled:pointer-events-none data-disabled:opacity-30 data-[state=open]:bg-white/10';
	const SUB = 'menu-surface min-w-[160px] max-w-[350px]';

	// a11y: Esc in a submenu closes just that level and puts focus back on its trigger (menus.md:
	// "Esc closes the deepest level"); bits-ui's default closes the whole menu.
	let subOpen = $state<Record<number, boolean>>({});
	let triggers = $state<Record<number, HTMLElement | null>>({});
	function escapeSub(ev: KeyboardEvent, i: number) {
		ev.preventDefault();
		// `escapeKeydownBehavior="close"` makes this sub the responsible layer (bits-ui otherwise
		// hands Escape to the root, which closes everything).
		setTimeout(() => {
			subOpen[i] = false;
			triggers[i]?.focus();
		});
	}
</script>

{#each entries as e, i (i)}
	{#if e.type === 'separator'}
		<DropdownMenu.Separator class="mx-0 my-1 h-px bg-white/10" />
	{:else if e.type === 'heading'}
		<div role="presentation" class="py-3 pr-2 pl-3 text-xs font-bold text-white/70">{e.label}</div>
	{:else if e.type === 'custom'}
		<!-- a11y: custom rows (e.g. the view-as buttons) sit in a group so the menu only owns
		     menu-allowed children. -->
		<div role="group">{@render e.snippet(closeMenu)}</div>
	{:else if e.type === 'saved-in'}
		<SavedInPicker song={e.song} close={closeMenu} />
	{:else if e.type === 'sub'}
		<DropdownMenu.Sub bind:open={() => subOpen[i] ?? false, (o) => (subOpen[i] = o)}>
			<DropdownMenu.SubTrigger class={ITEM} bind:ref={() => triggers[i] ?? null, (r) => (triggers[i] = r)}>
				{#if e.icon}<Icon icon={e.icon} size={16} />{/if}
				<span class="min-w-0 flex-1 truncate">{e.label}</span>
				<Icon icon={CaretRightIcon} size={16} fill />
			</DropdownMenu.SubTrigger>
			<DropdownMenu.SubContent
				class={SUB}
				style={e.width ? `width:${e.width}px` : undefined}
				sideOffset={4}
				alignOffset={-4}
				collisionPadding={12}
				escapeKeydownBehavior="close"
				onEscapeKeydown={(ev) => escapeSub(ev, i)}
			>
				<Self entries={typeof e.items === 'function' ? e.items() : e.items} />
			</DropdownMenu.SubContent>
		</DropdownMenu.Sub>
	{:else if e.type === 'playlist-picker'}
		<DropdownMenu.Sub bind:open={() => subOpen[i] ?? false, (o) => (subOpen[i] = o)}>
			<DropdownMenu.SubTrigger class={ITEM} bind:ref={() => triggers[i] ?? null, (r) => (triggers[i] = r)}>
				<Icon icon={PlusIcon} size={16} />
				<span class="min-w-0 flex-1 truncate">{e.label ?? 'Add to playlist'}</span>
				<Icon icon={CaretRightIcon} size={16} fill />
			</DropdownMenu.SubTrigger>
			<DropdownMenu.SubContent
				class="{SUB} w-[350px]"
				sideOffset={4}
				alignOffset={-4}
				collisionPadding={12}
				escapeKeydownBehavior="close"
				onEscapeKeydown={(ev) => escapeSub(ev, i)}
			>
				<PlaylistPicker songs={e.songs} itemClass={ITEM} />
			</DropdownMenu.SubContent>
		</DropdownMenu.Sub>
	{:else if e.checked !== undefined}
		<!-- a11y: a checked/unchecked entry (sort order, settings choices) is a menuitemcheckbox
		     with aria-checked; same look as a plain item. -->
		<DropdownMenu.CheckboxItem
			class="{ITEM} {e.checked ? '!text-accent-text' : ''}"
			checked={!!e.checked}
			disabled={e.disabled}
			onSelect={() => {
				closeMenu();
				e.onSelect();
			}}
		>
			{#if e.icon}<Icon icon={e.icon} size={16} />{/if}
			<span class="min-w-0 flex-1 truncate">{e.label}</span>
			{#if e.hint}<span class="text-xs text-white/50">{e.hint}</span>{/if}
			{#if e.checked}<Icon icon={CheckIcon} size={16} class="text-accent" />{/if}
		</DropdownMenu.CheckboxItem>
	{:else}
		<DropdownMenu.Item
			class="{ITEM} {e.checked ? '!text-accent-text' : ''}"
			disabled={e.disabled}
			onSelect={() => {
				closeMenu();
				e.onSelect();
			}}
		>
			{#if e.icon}<Icon icon={e.icon} size={16} />{/if}
			<span class="min-w-0 flex-1 truncate">{e.label}</span>
			{#if e.hint}<span class="text-xs text-white/50">{e.hint}</span>{/if}
			{#if e.checked}<Icon icon={CheckIcon} size={16} class="text-accent" />{/if}
		</DropdownMenu.Item>
	{/if}
{/each}
