// a11y: honour "Reduce motion" for JS-driven scrolling (CSS motion is handled globally in
// layout.css). Use as `el.scrollBy({ left, behavior: scrollBehavior() })`.
export function reducedMotion(): boolean {
	return typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
}
export const scrollBehavior = (): ScrollBehavior => (reducedMotion() ? 'auto' : 'smooth');
