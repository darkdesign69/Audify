// Self-check for the sign-in sheet's states in `signin.ts`:
//
//     node --experimental-strip-types ui/src/lib/signin.check.ts
import { nextSignIn, type SignInPhase } from './signin.ts';

function ok(cond: boolean, what: string): void {
	if (!cond) throw new Error(`FAIL: ${what}`);
}

const idle: SignInPhase = { kind: 'idle' };
const waiting = nextSignIn(idle, { type: 'start' });
ok(waiting.kind === 'waiting', 'pressing Sign in shows "Signing in…"');
ok(nextSignIn(waiting, { type: 'done' }).kind === 'idle', 'login-done closes the sheet');
ok(nextSignIn(waiting, { type: 'picker' }).kind === 'idle', 'the channel picker takes over');
ok(nextSignIn(waiting, { type: 'cancelled' }).kind === 'idle', 'closing the window closes the sheet');

const failed = nextSignIn(waiting, { type: 'error', message: 'Sign-in timed out. Try again.' });
ok(failed.kind === 'failed' && failed.message === 'Sign-in timed out. Try again.', 'errors are shown');
ok(nextSignIn(failed, { type: 'cancelled' }) === failed, 'a late cancel keeps the error on screen');
ok(nextSignIn(failed, { type: 'start' }).kind === 'waiting', 'Try again starts over');

console.log('ok');
