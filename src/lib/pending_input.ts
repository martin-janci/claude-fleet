// The permission / question dialog a blocked session is showing, turned
// into something a client can draw buttons from.
//
// Two sources say what is on the pane, and they disagree by design:
//   - `sessions.pending_input`, written by the 20 s reconcile tick, so up to
//     a tick stale;
//   - `ActivityProbe.pending_input`, one `capture-pane` read from a couple
//     of seconds ago.
// A fresh probe is therefore the authority whenever there is one, INCLUDING
// when it says there is no dialog: the row outliving a dialog answered in
// the terminal is exactly the case where a leftover button would press a
// key into whatever came next.
//
// Everything here is side-effect free, so the card's decisions are testable
// without mounting a component.
import type { ActivityProbe } from './conversation';
import type { ClaudeStatus, SessionRow, StuckKind } from './sessions';

export type PendingInput = NonNullable<SessionRow['pending_input']>;
export type PendingOption = PendingInput['options'][number];

/** Highest ordinal the REPL has a single keystroke for. A dialog may carry
 *  more options than this (the backend caps at 16), but there is no way to
 *  press "10" that a select dialog will not read as "1". */
export const ANSWER_MAX_DIGIT = 9;

/** The tmux key that picks option `n`, or `null` when there is none. */
export function answerKeyFor(n: number): string | null {
  if (!Number.isInteger(n) || n < 1 || n > ANSWER_MAX_DIGIT) return null;
  return String(n);
}

export interface AnswerOption extends PendingOption {
  /** The key to send, or `null` for an option only the terminal can pick. */
  key: string | null;
}

export interface AnswerView {
  kind: PendingInput['kind'];
  question: string | null;
  options: AnswerOption[];
  /** True when this came from a probe (seconds old) rather than the row. */
  live: boolean;
}

/**
 * The dialog to draw, or `null` for "show no answer UI".
 *
 * `probe` is the *fresh* probe only — the caller passes `null` once its
 * reading has gone stale, the same gate the live indicator uses.
 */
export function pendingInputFor(a: {
  rowStatus: ClaudeStatus | null;
  rowStuck: StuckKind | null;
  rowPending: PendingInput | null;
  probe: ActivityProbe | null;
}): AnswerView | null {
  // Stuck outranks a dialog everywhere else (the stuck chip, the Press Enter
  // chip), so it does here: a pane at an auth menu or an OOM message is not
  // answering numbered choices.
  if (a.probe?.stuck_kind ?? a.rowStuck) return null;
  const status = a.probe?.claude_status ?? a.rowStatus;
  if (status !== 'blocked') return null;
  const live = a.probe !== null;
  const dialog = live ? a.probe!.pending_input : a.rowPending;
  if (!dialog || dialog.options.length === 0) return null;
  return {
    kind: dialog.kind,
    question: dialog.question,
    options: dialog.options.map((o) => ({ ...o, key: answerKeyFor(o.n) })),
    live,
  };
}

/**
 * A stable identity for "the question being asked", for the check made
 * immediately before a key goes out: if the pane is no longer showing this
 * dialog, the click answers something the user never read.
 *
 * Which option is *highlighted* is deliberately not part of it — arrow keys
 * move the `❯` glyph without changing the question.
 */
export function answerFingerprint(p: {
  kind: string;
  question: string | null;
  options: { n: number; label: string }[];
}): string {
  return JSON.stringify([p.kind, p.question, p.options.map((o) => [o.n, o.label])]);
}
