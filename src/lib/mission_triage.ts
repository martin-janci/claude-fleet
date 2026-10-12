// K3 mission triage (redesign step 9.10): a stuck mission's card. Fleet's
// own facts say whether a mission is stuck; Jev proposes the outcome so far
// and the next step when its feature is on (`decide.jev.mission_triage`);
// the LLM writes the card's words only when asked. A person picks the step
// through the action they already have: triage never completes a mission
// and never sets Verified.
import type { ProposalLike } from './ai_proposal';
import type { Draft } from './drafts';
import type { OfState } from './kit/status';
import { invokeCmd, type Result } from './result';

/** Why fleet calls a mission stuck. */
export interface Stuck {
  /** `budget` | `no_progress` | `failed` | `blocked`. */
  reason: string;
  why: string;
  done: number;
  failed: number;
  blocked: number;
  total: number;
  last_failure?: string | null;
}

/** One proposal on the wire (step 2.8). */
export interface WireProposal {
  feature: string;
  value: string;
  source: 'rule' | 'jev' | 'llm';
  reason?: string | null;
  confidence_pct?: number | null;
  run_id?: number | null;
  at?: number | null;
}

export interface Triage {
  /** Absent when the mission is not stuck: no card. */
  stuck?: Stuck | null;
  outcome?: WireProposal | null;
  next?: WireProposal | null;
  /** The drafted words, when this call drafted them. */
  card?: Draft | null;
  may_change?: boolean;
}

/** The next steps, in the card's order. */
export const NEXT_STEPS = ['retry', 'split', 'give_up', 'ask'] as const;
export type NextStep = (typeof NEXT_STEPS)[number];

const STEP_LABELS: Record<NextStep, string> = {
  retry: 'Retry',
  split: 'Split',
  give_up: 'Give up',
  ask: 'Ask',
};

/** What each step does, in the button's title. */
export const STEP_HINTS: Record<NextStep, string> = {
  retry: 'Run the failed task again',
  split: 'Ask the planner to break the work down',
  give_up: 'Cancel the mission (asks first)',
  ask: 'Answer or add a question on the mission',
};

/** An outcome (a mission's, or a task report's) as a status word and the
 *  kit state its chip takes: the manual's six words, with what is partial or
 *  blocked as the reason. Both wait on a person: Needs you. */
const OUTCOMES: Record<string, [OfState, string]> = {
  done: ['done', 'Done'],
  partial: ['waiting', 'Needs you · partly done'],
  blocked: ['waiting', 'Needs you · blocked'],
  failed: ['failed', 'Failed'],
};

export function stepLabel(step: string): string {
  return STEP_LABELS[step as NextStep] ?? step;
}

export function outcomeLabel(outcome: string): string {
  return OUTCOMES[outcome]?.[1] ?? outcome;
}

export function outcomeState(outcome: string): OfState {
  return OUTCOMES[outcome]?.[0] ?? 'idle';
}

/** "2 done, 1 failed of 4": the card's counts line. */
export function countsLine(s: Stuck): string {
  const parts = [`${s.done} done`];
  if (s.failed) parts.push(`${s.failed} failed`);
  if (s.blocked) parts.push(`${s.blocked} blocked`);
  return `${parts.join(', ')} of ${s.total}`;
}

/** A wire proposal as ProposedBy reads it. */
export function asProposal(p: WireProposal | null | undefined): ProposalLike | null {
  if (!p) return null;
  return { value: p.value, source: p.source, reason: p.reason, confidence_pct: p.confidence_pct };
}

/**
 * A stuck mission's card. `refresh` also drafts its words (an LLM run,
 * booked on the mission as `triage`); without it nothing runs but Jev,
 * whose answer on the same facts is reused for a week.
 */
export function missionTriage(missionId: number, refresh = false): Promise<Result<Triage>> {
  return invokeCmd<Triage>('mission_triage', { args: { mission_id: missionId, refresh } });
}

/** What a mission's triage answer depends on that the list row shows: a
 *  change to any of these is a new question for Jev. */
export interface TriageFacts {
  id: number;
  state: string;
  version?: number;
  updated_at?: number;
  done?: number;
  total?: number;
}

function triageKey(m: TriageFacts): string {
  return [m.state, m.version, m.updated_at, m.done, m.total].join('\u0000');
}

/** The newest answer per mission, keyed by the facts it was asked on. */
const triageAsked = new Map<number, { key: string; answer: Promise<Result<Triage>> }>();

/**
 * `missionTriage(m.id)` at most once per mission per state for the app's
 * lifetime (review r15): each ask spends Jev's budget, and Today's Nudge
 * re-reads on every work change. A mission whose state, version, timestamp
 * or progress moved is asked again; a failed answer is not kept.
 */
export function missionTriageOnce(m: TriageFacts): Promise<Result<Triage>> {
  const key = triageKey(m);
  const hit = triageAsked.get(m.id);
  if (hit && hit.key === key) return hit.answer;
  const answer = missionTriage(m.id);
  triageAsked.set(m.id, { key, answer });
  void answer.then((r) => {
    if (!r.ok && triageAsked.get(m.id)?.answer === answer) triageAsked.delete(m.id);
  });
  return answer;
}

/** Test hook: forget every cached triage answer. Not for production code. */
export function resetTriageCacheForTests(): void {
  triageAsked.clear();
}

/**
 * A hub older than 9.10 answers one of these: no triage there, not a
 * failure. Matched by code, never by message text.
 */
export const HUB_HAS_NO_TRIAGE = ['E_INVALID', 'E_FORBIDDEN', 'E_HUB_PROTOCOL'];
