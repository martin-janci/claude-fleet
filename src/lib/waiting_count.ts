// Redesign step 3.14: how many sessions wait for a person, for the Halo on
// the status bar's mark. The Sidebar's "Needs you" rule (countNeedsYou), with
// the same exception it makes: a mass loss folded into one "12 stopped on
// trn" row (step 1.1) is one host to restore, not twelve people to answer.
import type { SessionRow } from './sessions';
import { countNeedsYou, type AttentionOptions } from './attention';
import { foldedIds, lostFolds } from './lost_fold';

export function waitingForYou(rows: readonly SessionRow[], opts: AttentionOptions): number {
  const folded = foldedIds(lostFolds(rows));
  return countNeedsYou(
    folded.size === 0 ? rows : rows.filter((s) => !folded.has(s.id)),
    opts,
  );
}
