// The Review tab's day tally and Jev's unsure case (Orbit Fleet G7.7, the
// Fleet work review board). Pure: the tab and its tests read these.
import { preselect } from './ai_proposal';
import { reviewProposal, taskLabel, type ReviewItem } from './work_view';

/** What this person decided in the Review tab today, on this device. */
export interface ReviewTally {
  /** The local calendar day, `YYYY-MM-DD`. */
  day: string;
  confirmed: number;
  rejected: number;
}

/** `now`'s local calendar day, `YYYY-MM-DD`. */
export function localDay(now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}`;
}

export function isReviewTally(v: unknown): v is ReviewTally {
  const t = v as ReviewTally;
  return !!t && typeof t.day === 'string' && Number.isInteger(t.confirmed) && Number.isInteger(t.rejected);
}

/** `t` as of `now`: a tally from another day starts again at zero. */
export function tallyToday(t: ReviewTally | null | undefined, now: Date = new Date()): ReviewTally {
  const day = localDay(now);
  return t && t.day === day ? t : { day, confirmed: 0, rejected: 0 };
}

/** `t` plus (or, with negative counts, minus) a decision; never below zero. */
export function addTally(
  t: ReviewTally | null | undefined,
  d: { confirmed?: number; rejected?: number },
  now: Date = new Date(),
): ReviewTally {
  const base = tallyToday(t, now);
  return {
    day: base.day,
    confirmed: Math.max(0, base.confirmed + (d.confirmed ?? 0)),
    rejected: Math.max(0, base.rejected + (d.rejected ?? 0)),
  };
}

/** "Done today: 4 confirmed · 1 rejected", or null before the first. */
export function tallyLine(t: ReviewTally | null | undefined, now: Date = new Date()): string | null {
  const x = tallyToday(t, now);
  if (x.confirmed === 0 && x.rejected === 0) return null;
  return `Done today: ${x.confirmed} confirmed · ${x.rejected} rejected`;
}

/**
 * Jev's unsure case: a suggestion the decision model made without the
 * confidence to pre-select it (rule 7: unsure, or under the floor) while
 * other tasks fit too. The tab then asks "Which ticket?" with the
 * candidates, nothing pre-selected, and offers Pick a ticket… and No ticket
 * instead of Confirm. `null` for every other item.
 */
export function reviewUnsure(it: ReviewItem): { candidates: string[] } | null {
  if (it.kind !== 'suggestion' || it.proposed_by?.source !== 'jev') return null;
  const alts = it.alternatives ?? [];
  if (alts.length === 0) return null;
  if (preselect('work_link', reviewProposal(it)) !== null) return null;
  const name = (t: { key?: string | null; title?: string | null; task_id: string }) => t.key || taskLabel(t);
  return { candidates: [name(it.task), ...alts.map(name)] };
}

/** "PD-2970 and PD-2974", "A, B and C". */
export function andList(words: readonly string[]): string {
  if (words.length <= 1) return words[0] ?? '';
  return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}
