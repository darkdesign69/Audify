// ⌘F "filter in current list" (menu://find). Pages with a filter field watch `find.seq` and
// focus their field when it changes: `$effect(() => { if (find.seq) field?.focus(); })`.
export const find = $state({ seq: 0 });
export const requestFind = () => find.seq++;
