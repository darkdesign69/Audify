// Self-check for imgguard.ts (run: node --experimental-strip-types src/lib/imgguard.check.ts).
import { BROKEN_ATTR, onImageEvent } from './imgguard.ts';

function fail(msg: string): never {
	throw new Error(msg);
}

function el(tagName: string) {
	const attrs = new Map<string, string>();
	return {
		tagName,
		attrs,
		setAttribute: (n: string, v: string) => void attrs.set(n, v),
		removeAttribute: (n: string) => void attrs.delete(n)
	};
}

const img = el('IMG');
onImageEvent({ type: 'error', target: img });
if (!img.attrs.has(BROKEN_ATTR)) fail('a failed image is hidden');
onImageEvent({ type: 'load', target: img });
if (img.attrs.has(BROKEN_ATTR)) fail('an image that loads (retry / new src) is shown again');

const script = el('SCRIPT');
onImageEvent({ type: 'error', target: script });
if (script.attrs.size) fail('only images are touched');
onImageEvent({ type: 'error', target: null });

console.log('ok imgguard');
