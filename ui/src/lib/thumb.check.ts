// Self-check for the URL rewrites in `thumb.ts`. Plain node, like the other checks here:
//
//     node --experimental-strip-types ui/src/lib/thumb.check.ts
//
// Prints "ok" and exits 0, or throws on the first broken invariant. Not imported by the app.
import { localPath, thumb } from './thumb.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

// The shape WEB_REMIX hands out for a video, /player included: 400x225 or 426x240, and the `sqp`
// crop is signed, so it can't be asked for bigger.
const video =
	'https://i.ytimg.com/vi/PtHEr7siapo/sddefault.jpg?sqp=-oaymwEWCJADEOEBIAQqCghqEJQEGHgg6AJIWg&rs=x';
ok(
	thumb(video, 720) === 'https://i.ytimg.com/vi/PtHEr7siapo/maxresdefault.jpg',
	'the big player asks for the 1280x720 video thumb (#425)'
);
ok(thumb(video, 400) === video, 'every smaller slot keeps the thumb YouTube sent');
ok(
	thumb('https://lh3.googleusercontent.com/x=w120-h120-l90-rj', 720) ===
		'https://lh3.googleusercontent.com/x=w720-h720-l90-rj',
	'album art is still resized in its URL'
);
ok(thumb('https://yt3.ggpht.com/y=s176', 400) === 'https://yt3.ggpht.com/y=s400', '=s art too');

// stream: coverfix — Made for you covers: stable names, the cache-buster kept out of the path.
const dir = '/Users/x/Library/Application Support/app.spotless.desktop/covers/mfy';
ok(
	JSON.stringify(localPath(`${dir}/MFY_DAILY_1.svg?v=1794747288123`)) ===
		JSON.stringify({ path: `${dir}/MFY_DAILY_1.svg`, query: '?v=1794747288123' }),
	'the ?v= stamp stays out of the converted path'
);
ok(
	localPath(`${dir}/MFY_DAILY_1-1794747288.svg`).path === `${dir}/MFY_DAILY_1.svg`,
	'a dated pre-coverfix cover from a cached Home maps to the stable file'
);
ok(
	localPath('C:\\Data\\covers\\mfy\\MFY_ON_REPEAT-12.svg').path === 'C:\\Data\\covers\\mfy\\MFY_ON_REPEAT.svg',
	'Windows paths too'
);
ok(localPath('/music/Why?.jpg').path === '/music/Why?.jpg', 'a ? in a file name is not a query');
ok(localPath('/music/Band - 1999.svg').path === '/music/Band - 1999.svg', 'other files untouched');
ok(localPath('/covers/playlists/a-12.jpg').path === '/covers/playlists/a-12.jpg', 'only mfy svgs');

console.log('ok');
