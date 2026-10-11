// Control's chat goes on by itself (2026-10-11): when the agent's last reply
// ends on a `choices` card with one option marked `recommended`
// (docs/chat-blocks.md) and nobody answers for `control.auto_continue_minutes`,
// the chat sends that option's prompt as if it had been pressed and sent.
//
// What keeps it from acting where a person must:
// * the setting `control.auto_continue` (on by default) turns it off;
// * an option or a question naming a push, a delete or another step that is
//   hard to undo (`quick_answer.ts`'s `risky`) is never sent;
// * the clock starts only once the agent is quiet with nothing in the box,
//   and typing, Cancel, or any other message stops it for that card;
// * a card from a reply older than STALE_MS is left alone, so opening the
//   app after a weekend does not answer last week's question.
import { blockKey, splitRich, type UiChoice } from './rich_blocks';
import { risky, riskyQuestion } from './quick_answer';
import type { Conversation } from './conversation';

/** A reply older than this is not answered for the person. */
export const STALE_MS = 24 * 60 * 60 * 1000;

export interface RecommendedPick {
  /** The card's identity, for "this card's clock was stopped". */
  key: string;
  option: UiChoice;
}

/**
 * The recommended option of the `choices` card the conversation ends on, or
 * `null`. The card must be in the last turn's last text, the turn must have
 * no later prompt, and neither the option nor the card's question may name a
 * risky step.
 */
export function recommendedPick(conv: Conversation | null, now = Date.now()): RecommendedPick | null {
  const turn = conv?.turns.at(-1);
  if (!turn) return null;
  const endedAt = Date.parse(turn.ended_at ?? turn.at ?? '');
  if (Number.isFinite(endedAt) && now - endedAt > STALE_MS) return null;
  const text = [...turn.items].reverse().find((i) => i.kind === 'text');
  if (!text || text.kind !== 'text') return null;
  const cards = splitRich(text.text).flatMap((s) => (s.t === 'ui' && s.block.kind === 'choices' ? [{ block: s.block, raw: s.raw }] : []));
  const last = cards.at(-1);
  if (!last || last.block.kind !== 'choices') return null;
  const option = last.block.options.find((o) => o.recommended);
  if (!option) return null;
  if (riskyQuestion(last.block.question) || riskyQuestion(last.block.title)) return null;
  if (risky(option.label) || risky(option.prompt)) return null;
  return { key: blockKey(last.raw), option };
}

/** Cards whose clock a person stopped or that already went on, by
 *  `<session>:<card>`. Per window: a restart starts no card that was sent. */
const settled = new Set<string>();
/** When each card's clock started (the agent quiet, the box empty). */
const startedAt = new Map<string, number>();

const id = (sessionId: number, key: string) => `${sessionId}:${key}`;

export function settle(sessionId: number, key: string): void {
  settled.add(id(sessionId, key));
  startedAt.delete(id(sessionId, key));
}

export function isSettled(sessionId: number, key: string): boolean {
  return settled.has(id(sessionId, key));
}

/** The clock's start for a card, started now when it has none. */
export function clockStart(sessionId: number, key: string, now = Date.now()): number {
  const k = id(sessionId, key);
  const at = startedAt.get(k);
  if (at !== undefined) return at;
  startedAt.set(k, now);
  return now;
}

/** Stop a card's clock without settling it: the agent went busy again. */
export function pauseClock(sessionId: number, key: string): void {
  startedAt.delete(id(sessionId, key));
}

/** `m:ss` left, for the countdown. */
export function countdown(msLeft: number): string {
  const s = Math.max(0, Math.ceil(msLeft / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

/** Test hook. */
export function resetAutoContinue(): void {
  settled.clear();
  startedAt.clear();
}
