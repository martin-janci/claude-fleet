// Jev N5 host placement (Orbit Fleet redesign step 4.11): the decision
// model's host for a project's new session, asked only when the dialog keeps
// no host for the project and two or more online hosts under their limit are
// left. Mirrors `service::decide::host_placement`. Off by default; a hub
// client asks the hub (gap plan G7.3).
import { invokeCmd, type Result } from './result';
import type { ProposalLike } from './ai_proposal';

/** Mirrors `host_placement::SuggestedHost`. */
export interface SuggestedHost {
  host_alias: string;
  confidence_pct?: number | null;
  run_id?: number | null;
}

/** The use case's floor, in whole percent (`host_placement::MIN_CONFIDENCE`). */
export const HOST_PLACEMENT_FLOOR = 50;

export function proposeHostPlacement(projectId: number): Promise<Result<SuggestedHost | null>> {
  return invokeCmd<SuggestedHost | null>('propose_host_placement', { args: { project_id: projectId } });
}

/** After a person's start: marks the proposal confirmed or corrected. */
export function recordHostPlacement(projectId: number, hostAlias: string): Promise<Result<boolean>> {
  return invokeCmd<boolean>('record_host_placement', {
    args: { project_id: projectId, host_alias: hostAlias },
  });
}

/** The suggestion as the shared ProposedBy chip reads it. */
export function hostProposal(s: SuggestedHost): ProposalLike {
  return { value: s.host_alias, source: 'jev', confidence_pct: s.confidence_pct ?? null };
}
