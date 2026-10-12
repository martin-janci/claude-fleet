// J5 quick answer (redesign step 10.9): Jev's likely option goes first in an
// agent's question or a chat form, with "Proposed by Jev". A person still
// answers; nothing is sent or pre-answered. The backend never asks about a
// permission dialog or a risky option (`decide/quick_answer.rs`), and this
// file checks the same words again before it moves anything, so a stale or
// odd proposal can never put a push, a permission or a step that is hard to
// undo first.
import { neverDecides, preselect, type ProposalLike } from './ai_proposal';
import { proposalFor, type DecisionProposal } from './proposals';

/** The decide feature, as a row's proposals name it. */
export const QUICK_ANSWER = 'quick_answer';

/** The words that make an option one AI never proposes. The same list as
 *  `RISKY_WORDS` in crates/fleet-core/src/service/decide/quick_answer.rs
 *  (its test reads this file). */
export const RISKY_WORDS = [
  'push',
  'force',
  'delete',
  'remove',
  'drop',
  'destroy',
  'wipe',
  'reset',
  'overwrite',
  'rm',
  'kill',
  'deploy',
  'publish',
  'release',
  'merge',
  'rebase',
  'revert',
  'truncate',
  'purge',
  'production',
  'prod',
  'allow',
  'always',
  'bypass',
  'permission',
  'permissions',
  'sudo',
  'approve',
] as const;

const RISKY = new RegExp(`\\b(?:${RISKY_WORDS.join('|')})\\b|don['’]?t ask again`, 'i');

/** An option AI never proposes: a push, a permission, a step hard to undo. */
export function risky(label: string): boolean {
  return RISKY.test(label);
}

/** A question that names a risky action ("Push the 3 commits to
 *  origin/main now?"): its "Yes, go ahead" is the push, though its own words
 *  name none, so nothing of it is reordered. The same words as `risky`; the
 *  backend's `risky_question` asks nothing about such a question. */
export function riskyQuestion(question: string | null | undefined): boolean {
  return !!question && RISKY.test(question);
}

const AFFIRMATIVE = /\b(?:yes|approve|proceed|accept|confirm|go ahead|allow)\b/i;

/** An option that says yes to whatever was asked: Jev may move it first on
 *  a safe question, but never draws it as the primary answer. */
export function affirmative(label: string): boolean {
  return AFFIRMATIVE.test(label);
}

/** The option number a proposal names (`o2` → 2), or null. */
export function proposedN(value: string | null | undefined): number | null {
  const m = /^o([1-9])$/.exec(value ?? '');
  return m ? Number(m[1]) : null;
}

/** The row's quick-answer proposal, when it carries one (redesign 2.8). */
export function quickProposal(
  row: { proposals?: DecisionProposal[] | null } | null | undefined,
): DecisionProposal | null {
  return proposalFor(row, QUICK_ANSWER);
}

/**
 * The options in the order to show them: the proposed one first, the rest
 * as they came. Nothing moves when there is no usable proposal (none, below
 * the floor, `unsure`), on a permission, when the question names a risky
 * action (`riskyQuestion`), or when the proposed option is risky.
 * `proposed` is the option that moved, for the "Proposed by Jev" line;
 * `primary` is the one to draw as the primary answer: the proposed one
 * unless it is affirmative, never an approval from Jev.
 */
export function quickOrder<T extends { n: number; label: string }>(
  options: readonly T[],
  proposal: ProposalLike | null | undefined,
  kind: 'permission' | 'input' | string = 'input',
  question?: string | null,
): { shown: T[]; proposed: T | null; primary: T | null } {
  const shown = [...options];
  const none = { shown, proposed: null, primary: null };
  // A permission is a person's (NEVER_DECIDES' approve_permission).
  if (kind === 'permission' && neverDecides('approve_permission')) return none;
  // A push, a merge, a deploy… asked in the question: approve_risky_step.
  if (riskyQuestion(question) && neverDecides('approve_risky_step')) return none;
  const n = proposedN(preselect(QUICK_ANSWER, proposal));
  const at = shown.findIndex((o) => o.n === n);
  if (at < 0 || risky(shown[at].label)) return none;
  const [o] = shown.splice(at, 1);
  return { shown: [o, ...shown], proposed: o, primary: affirmative(o.label) ? null : o };
}
