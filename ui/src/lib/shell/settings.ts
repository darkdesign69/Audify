// "Settings" everywhere (avatar menu, ⌘, native menu): the full-page /settings route (settings.md).
// Change it here only.
import { goSettings } from '$lib/settings/actions';
export function openSettings() {
	goSettings();
}
