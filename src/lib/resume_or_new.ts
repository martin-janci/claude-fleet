// Jev N2 "Resume or start fresh" (gap plan G7.10): the New session dialog's
// proposal for a work key that has only past work. Mirrors
// `service::decide::resume_or_new`. A rule answers when the key has exactly
// one recent resumable past session; otherwise Jev may, only with
// `decide.jev.resume_or_new` at assist (off by default). A hub without the
// tool answers nothing, and the dialog keeps its plain past-work notice.
import { invokeCmd, type Result } from './result';
import type { ProposalLike, ProposalSource } from './ai_proposal';

/** Mirrors `resume_or_new::ResumeOrNew`. Every field absent: no proposal. */
export interface ResumeOrNew {
  /** `l<link id>` (resume that past session) or `new`. */
  value?: string | null;
  link_id?: number | null;
  session_id?: number | null;
  name?: string | null;
  source?: string | null;
  reason?: string | null;
  confidence_pct?: number | null;
  run_id?: number | null;
  unsure?: boolean;
}

/** The option that starts fresh. */
export const START_FRESH = 'new';

/** The use case's floor, in whole percent (`resume_or_new::MIN_CONFIDENCE`). */
export const RESUME_OR_NEW_FLOOR = 50;

export function proposeResumeOrNew(key: string): Promise<Result<ResumeOrNew>> {
  return invokeCmd<ResumeOrNew>('resume_or_new_propose', { args: { key } });
}

/** The person's pick: `l<link id>` (Resume) or `new` (Start fresh instead).
 *  Marks the Jev proposal they were shown confirmed or corrected. */
export function followResumeOrNew(key: string, chosen: string): Promise<Result<boolean>> {
  return invokeCmd<boolean>('resume_or_new_follow', { args: { key, chosen } });
}

/** The pick a resume of `linkId` is. */
export function resumeChoice(linkId: number): string {
  return `l${linkId}`;
}

/** The proposal as the shared ProposedBy chip reads it; `null` when there
 *  is none (nothing proposed, unsure, or a source the UI does not know). */
export function resumeProposal(p: ResumeOrNew | null | undefined): ProposalLike | null {
  if (!p || p.unsure || !p.value) return null;
  if (p.source !== 'rule' && p.source !== 'jev') return null;
  if (p.value !== START_FRESH && p.link_id == null) return null;
  return {
    value: p.value,
    source: p.source as ProposalSource,
    reason: p.reason ?? null,
    confidence_pct: p.confidence_pct ?? null,
  };
}
