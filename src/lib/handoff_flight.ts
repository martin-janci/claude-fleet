// The comet onto a "Sent to a session" chip (Orbit Fleet redesign step
// 9.13): when Control's agent hands work to a session while the chat is
// open, a comet flies from the message to the chip, then the chip settles.
// Only a receipt written since the chat opened flies (one read on open is
// history, not news), and only at full motion: reduced and off motion show
// the chip at once.
import type { ControlHandoff } from './handoffs';
import type { Motion } from './motion';

/** Whether this receipt arrives with a flight. `sinceSec` is when the chat opened. */
export function fliesIn(h: ControlHandoff, sinceSec: number, motion: Motion): boolean {
  return h.kind === 'session' && motion === 'full' && h.at >= sinceSec;
}
