<script lang="ts">
	/** Renders the one app-wide menu opened with `openMenu()`. Mount once in the root layout. */
	import { DropdownMenu } from 'bits-ui';
	import MenuEntries from './MenuEntries.svelte';
	import { closeMenu, menu, restoreMenuFocus } from './state.svelte';
</script>

{#key menu.seq}
	<DropdownMenu.Root
		open={menu.open}
		onOpenChange={(o) => {
			if (!o) closeMenu();
		}}
	>
		<DropdownMenu.Portal>
			<DropdownMenu.Content
				customAnchor={menu.anchor}
				side={menu.side}
				align={menu.align}
				sideOffset={menu.anchor instanceof HTMLElement ? 4 : 0}
				collisionPadding={12}
				class="menu-surface min-w-[160px] max-w-[350px]"
				style={menu.width ? `width:${menu.width}px` : undefined}
				onCloseAutoFocus={(e) => {
					e.preventDefault();
					restoreMenuFocus();
				}}
			>
				<MenuEntries entries={menu.entries} />
			</DropdownMenu.Content>
		</DropdownMenu.Portal>
	</DropdownMenu.Root>
{/key}
