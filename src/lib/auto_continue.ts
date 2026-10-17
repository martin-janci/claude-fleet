// Control's chat goes on by itself (2026-10-11; moved to the fleet 2026-10-17,
// review F17): when the agent's last reply ends on a `choices` card with one
// option marked `recommended` (docs/chat-blocks.md) and nobody answers for
// `control.auto_continue_minutes`, fleet sends that option for the person.
//
// The clock is NOT here any more. `service::control_continue` (fleet-core)
// records the pick at Control's Stop hook and sends it from the tick, once,
// with no window open. This module only reads its state for the countdown
// and settles it: Cancel (or the person typing) and Send now (`take`, whose
// answer carries the prompt the window sends as the person's own words).
// The phone reads the same `control_auto_continue` tool.
//
// What keeps it from acting where a person must lives there too: the
// setting, "Ask first" on the autonomy dial, risky words, a newer turn, a
// busy agent, and cards older than a day.
import { invokeCmd, type Result } from './result';

export type PickState = 'waiting' | 'sent' | 'taken' | 'cancelled' | 'superseded' | 'failed';

export interface PendingPick {
  session_id: number;
  /** The card's identity: Control's turn_seq after the turn that ended on it. */
  turn_seq: number;
  label: string;
  prompt: string;
  ended_at: number;
  state: PickState;
  settled_at?: number | null;
}

export interface AutoContinueView {
  enabled: boolean;
  minutes: number;
  pending?: PendingPick | null;
  /** Unix seconds the fleet sends it at. */
  due_at?: number | null;
}

export function autoContinue(action: 'status' | 'cancel' | 'take', turnSeq?: number): Promise<Result<AutoContinueView>> {
  return invokeCmd<AutoContinueView>('control_auto_continue', { action, turnSeq: turnSeq ?? null });
}

/** The pick to draw a countdown for in `sessionId`'s chat, or null. */
export function liveCountdown(v: AutoContinueView | null, sessionId: number): { pick: PendingPick; dueAt: number } | null {
  const p = v?.pending;
  if (!v?.enabled || !p || p.state !== 'waiting' || p.session_id !== sessionId || v.due_at == null) return null;
  return { pick: p, dueAt: v.due_at };
}

/** `m:ss` left, for the countdown. */
export function countdown(msLeft: number): string {
  const s = Math.max(0, Math.ceil(msLeft / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}
