// Control's composer queues what is typed while Control is mid-turn (review
// 2026-10-17, F4). The gate `blockWhileBusy` exists because two prompts
// pasted into one REPL mid-turn arrive as one mangled line; it used to answer
// that by refusing the send, so the person could not type to Control during
// a `[fleet events]` digest or a 10-minute `wait_for_session`. Now the line
// is HELD: it shows as a "queued · cancel" chip above the box and goes out,
// one line per turn, once the turn ends. The Stop button next to Send
// interrupts the turn (the REPL's own Escape).

import { get, writable } from 'svelte/store';

/** One line typed while the composer was gated. */
export interface HeldLine {
  id: number;
  text: string;
}

let nextId = 1;

/**
 * The held lines of a gated composer that went away (review w04-6). They
 * are typed to Control, not to one pane of it, and Control has one gated
 * composer: when its panel unmounts (the person leaves Control for a rail
 * item or "All sessions") its lines are parked here, and the next gated
 * panel takes them back — also when Control was moved to a new row (#923),
 * so the lines go to the Control that now runs instead of vanishing.
 */
export const parkedHeld = writable<readonly HeldLine[]>([]);

/** Park a gated composer's held lines as its panel goes. */
export function parkHeld(lines: readonly HeldLine[]): void {
  parkedHeld.update((p) => [...p, ...lines]);
}

/** Take the parked lines back (once). */
export function unparkHeld(): HeldLine[] {
  const out = [...get(parkedHeld)];
  parkedHeld.set([]);
  return out;
}

/** Test seam: nothing parked. */
export function resetHeldForTests(): void {
  parkedHeld.set([]);
}

/** Hold `text` behind what is already held. Blank text holds nothing. */
export function hold(held: readonly HeldLine[], text: string): HeldLine[] {
  const t = text.trim();
  if (!t) return [...held];
  return [...held, { id: nextId++, text: t }];
}

/** The line to send now, or null: the first held one, once the gate has
 *  opened (the turn ended and the last message was taken) and nothing else
 *  bars a write. */
export function due(held: readonly HeldLine[], gated: boolean, blocked: boolean): HeldLine | null {
  if (gated || blocked) return null;
  return held[0] ?? null;
}

/** Drop the held line `id`. */
export function drop(held: readonly HeldLine[], id: number): HeldLine[] {
  return held.filter((h) => h.id !== id);
}

/** The box after a held line is cancelled: its text comes back, ahead of
 *  whatever the person started meanwhile. */
export function cancelInto(draft: string, line: HeldLine): string {
  return draft.trim() ? `${line.text}\n\n${draft}` : line.text;
}

/** The chip's words for a held line: its first line, cut short. */
export function heldPreview(text: string, max = 60): string {
  const first = text.split('\n').map((l) => l.trim()).find((l) => l !== '') ?? '';
  return first.length <= max ? first : `${first.slice(0, max - 1).trimEnd()}…`;
}

/** The key that interrupts Claude Code's running turn. */
export const STOP_KEY = 'Escape';

/** Whether the Stop button shows: on a gated composer, while the session is
 *  busy (`busyNote` is the composer's own "working" sentence). */
export function canStop(gatedComposer: boolean, busyNote: string | null): boolean {
  return gatedComposer && busyNote !== null;
}
