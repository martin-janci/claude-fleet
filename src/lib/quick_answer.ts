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
] as const;

const RISKY = new RegExp(`\\b(?:${RISKY_WORDS.join('|')})\\b|don'?t ask again`, 'i');

/** An option AI never proposes: a push, a permission, a step hard to undo. */
export function risky(label: string): boolean {
  return RISKY.test(label);
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
 * the floor, `unsure`), on a permission, or when the proposed option is
 * risky. `proposed` is the option that moved, for the "Proposed by Jev" line.
 */
export function quickOrder<T extends { n: number; label: string }>(
  options: readonly T[],
  proposal: ProposalLike | null | undefined,
  kind: 'permission' | 'input' | string = 'input',
): { shown: T[]; proposed: T | null } {
  const shown = [...options];
  // A permission is a person's (NEVER_DECIDES' approve_permission).
  if (kind === 'permission' && neverDecides('approve_permission')) return { shown, proposed: null };
  const n = proposedN(preselect(QUICK_ANSWER, proposal));
  const at = shown.findIndex((o) => o.n === n);
  if (at < 0 || risky(shown[at].label)) return { shown, proposed: null };
  const [o] = shown.splice(at, 1);
  return { shown: [o, ...shown], proposed: o };
}
