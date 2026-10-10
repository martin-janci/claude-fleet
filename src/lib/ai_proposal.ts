/**
 * Redesign step 3.11: the two AI patterns' logic, shared by ProposedBy and
 * DraftField, and the one table of what AI never decides.
 *
 * Every machine suggestion goes rule first, then Jev, then an LLM
 * (docs/ux/2026-10-08-orbit-fleet-redesign/design-system/ai.md). A
 * proposal pre-selects; a person confirms. Below the confidence floor, on
 * `unsure`, or for anything in {@link NEVER_DECIDES}, nothing is
 * pre-selected and the UI asks as it does without AI.
 *
 * The proposal shape mirrors the wire (redesign step 2.8). The components
 * take only the fields below, so a row, a task or a start preview can hand
 * its proposal straight in.
 */

/** Who answered: a fleet rule, the decision model (Jev), or an LLM. */
export type ProposalSource = 'rule' | 'jev' | 'llm';

/** One proposal as the UI reads it. */
export interface ProposalLike {
  /** The proposed option, in the use case words. */
  value: string;
  source: ProposalSource;
  /** Why, in words a person reads. */
  reason?: string | null;
  /** Whole percent; absent for a rule. */
  confidence_pct?: number | null;
}

/** The option that proposes nothing. */
export const UNSURE = 'unsure';

/**
 * Below this a model answer is not pre-selected (the floor every Jev use
 * case shares unless it passes its own, as `start_project` does with 50).
 */
export const DEFAULT_CONFIDENCE_FLOOR = 50;

/**
 * What AI never decides (the plan's "Where AI never decides" and the design
 * manual's ai.md). These stay rules or a person's choice: a proposal aimed
 * at one is dropped, whatever its confidence. One row per target, so a test
 * and a review read the same list.
 */
export const NEVER_DECIDES = [
  { target: 'approve_push', why: 'Approving a push is a person' },
  { target: 'approve_permission', why: 'Approving a permission is a person' },
  { target: 'approve_risky_step', why: 'A risky step is approved by a person' },
  { target: 'device_trust', why: 'Device trust is a person' },
  { target: 'role', why: 'Roles are a person' },
  { target: 'share', why: 'Sharing is a person' },
  { target: 'access_level', why: 'Access level is a person' },
  { target: 'assign_org', why: 'A task organisation is a person or a rule' },
  { target: 'mission_autonomy', why: 'Mission autonomy is a person' },
  { target: 'orchestrator_max_level', why: 'The autonomy ceiling is a setting a person changes' },
  { target: 'settings_change', why: 'Settings change only when a person applies them' },
  { target: 'mission_complete', why: 'Completion is CI, tests and commits' },
  { target: 'verified', why: 'Verified is proof only' },
  { target: 'force_kill', why: 'Force kill readiness is git only' },
  { target: 'cleanup_ready', why: 'Clean up readiness is git only' },
  { target: 'host_by_numbers', why: 'Limits, disk and latency choose by numbers' },
  { target: 'account_by_numbers', why: 'Account limits choose by numbers' },
  { target: 'priority', why: 'Priority is a person' },
  { target: 'task_size', why: 'Task size is a person' },
  { target: 'needs_you_order', why: 'The order of Needs you is a rule' },
  { target: 'add_host_checks', why: 'Add-host checks are checks' },
  { target: 'error_report', why: 'Error reports are facts' },
] as const;

export type NeverTarget = (typeof NEVER_DECIDES)[number]['target'];

const NEVER = new Set<string>(NEVER_DECIDES.map((r) => r.target));

/** Whether `target` is one AI never decides. */
export function neverDecides(target: string): boolean {
  return NEVER.has(target);
}

/** The words that put a form field on {@link NEVER_DECIDES}, whatever its
 *  options are called: an agent's "Priority" select with the options
 *  low/high is still a priority (review r15 F14). */
const NEVER_FIELD_WORDS =
  /\b(priority|priorit\w*|size|estimate|story points?|roles?|share|sharing|access|trust\w*|org|orgs|organi[sz]ations?|autonomy|approv\w*|permissions?|verif\w*|complet\w*|kill|clean ?up|settings?)\b/i;

/** Whether a form field asks something AI never decides, read from its
 *  name, label and help (`max_priority` reads as `max priority`). */
export function neverDecidesField(f: { name: string; label?: string; help?: string }): boolean {
  const words = [f.name.replace(/[_-]+/g, ' '), f.label ?? '', f.help ?? ''].join(' ');
  return NEVER_FIELD_WORDS.test(words);
}

/**
 * The value to pre-select for `target`, or `null` when nothing may be:
 * no proposal, a target AI never decides, `unsure`, or a model answer
 * below `floor` (a rule has no confidence and always passes).
 */
export function preselect(
  target: string,
  proposal: ProposalLike | null | undefined,
  floor = DEFAULT_CONFIDENCE_FLOOR,
): string | null {
  if (!proposal || neverDecides(target)) return null;
  if (!proposal.value || proposal.value === UNSURE) return null;
  if (proposal.source !== 'rule') {
    const pct = proposal.confidence_pct;
    if (pct == null || pct < floor) return null;
  }
  return proposal.value;
}

/** The pill text for a source. */
export function proposedByLabel(source: ProposalSource): string {
  switch (source) {
    case 'jev':
      return 'Proposed by Jev';
    case 'llm':
      return 'Proposed by the LLM';
    case 'rule':
      return 'Proposed by a rule';
  }
}

/**
 * A confidence in words, never a percentage (AI patterns board: "A
 * confidence word, never a percentage"). Takes whole percent. Null when
 * there is no number.
 */
export function confidenceWord(pct: number | null | undefined): string | null {
  if (typeof pct !== 'number' || !Number.isFinite(pct)) return null;
  if (pct >= 90) return 'almost sure';
  if (pct >= 70) return 'likely';
  if (pct >= 50) return 'maybe';
  return 'unsure';
}

/** A why line from the backend with its Jev confidence note ("Jev
 *  proposed ABC-12 (82%) · R12") worded: "(likely)". */
export function wordConfidenceIn(text: string): string {
  return text.replace(/\((\d{1,3})%\)/g, (_, n: string) => `(${confidenceWord(Number(n))})`);
}

/** The corrected state (AI patterns board): the person picked something
 *  else, and the pick is kept as the label Jev is measured against.
 *  `recorded` says when: "now" or a moment still ahead ("on start"). */
export function correctionLine(from: string, to: string, recorded = ''): string {
  return `You changed ${from} → ${to} · recorded as a correction${recorded ? ` ${recorded}` : ''}`;
}

/** The parts of the "When AI changed something" line, in board order:
 *  "Linked to PD-2592 · Proposed by Jev · you confirmed". The Undo is the
 *  consumer's button or toast action. */
export function aiChangeLine(what: string, source: ProposalSource, confirmed = false): string {
  return [what, proposedByLabel(source), confirmed ? 'you confirmed' : ''].filter(Boolean).join(' · ');
}

/** Where a draft came from: "by haiku on mercury · from 3 changed files". */
export function draftedBy(model?: string | null, host?: string | null, from?: string | null): string {
  const who = [model ? `by ${model}` : '', host ? `on ${host}` : ''].filter(Boolean).join(' ');
  return [who, from ?? ''].filter(Boolean).join(' · ');
}
