// Focus requests for the top-bar search field (⌘K / ⌘⇧L / menu://search).
export const searchFocus = $state({ seq: 0 });
export const focusSearch = () => searchFocus.seq++;
