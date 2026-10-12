// What a rule, Jev or an LLM proposes about a row (redesign 2.8): one shape
// on session rows, Work view tasks and start previews, read from
// `decision_runs` by the backend (`DecisionProposal` in fleet-core). A
// proposal pre-selects at most; a person confirms or changes it, and none
// ever acts. The backend sends only live `assist` answers at or above the
// confidence floor, never `unsure`, and drops one once a person decided it.

/** Where a proposal came from. */
export type ProposalSource = 'rule' | 'jev' | 'llm';

/** One proposal: a value for one question (`feature`) about a row. */
export interface DecisionProposal {
  /** The question: a decide feature (`start_project`, `turn_outcome`, …). */
  feature: string;
  /** The proposed answer, an id or a vocabulary word (`p12`, `finished`). */
  value: string;
  source: ProposalSource;
  /** Fleet's own words for why, when the use case composes them. */
  reason?: string | null;
  /** The model's confidence in whole percent, when it gave one. */
  confidence_pct?: number | null;
  /** The recorded run, which a confirm or a change marks. */
  run_id?: number | null;
  /** When it was decided, unix seconds. */
  at?: number | null;
  /** A `related_session` a person confirmed with Link (M15 G4.3): no
   *  longer a proposal but the row's linked partner. */
  linked?: boolean | null;
}

/** The proposal a row carries for `feature`, if any. */
export function proposalFor(
  row: { proposals?: DecisionProposal[] | null } | null | undefined,
  feature: string,
): DecisionProposal | null {
  return row?.proposals?.find((p) => p.feature === feature) ?? null;
}
