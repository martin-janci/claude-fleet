// Which leash a session is on, in one sentence, for the inspector (HITL
// audit H-01 step 4). The hub's guard (`service::hooks::pre_tool_use_decision`)
// answers a person's step — merging or readying a PR, pushing to the default
// branch, writing to a tracker — with `deny` in a mission's worker and `ask`
// in every other session fleet tracks while `sessions.ask_before_person_steps`
// is on (the default, owner decision 2026-11-11). Pure, so each case is tested
// without mounting the inspector.
import { sessionAgent, type SessionRow } from './sessions';

export interface Leash {
  /** "Asks you first", "Refused", "Unasked": the state, for tests and styling;
   *  the inspector's label is "Asks you" and `text` answers it. */
  label: string;
  /** The sentence. */
  text: string;
  /** What happens when the hub cannot answer: the guard fails open. */
  caveat: string;
}

const STEPS = 'merging or readying a PR, pushing to main, or writing to a tracker';
const STEPS_CAP = 'Before merging or readying a PR, pushing to main, or writing to a tracker';
const CAVEAT = "If the hub does not answer in time, the step runs; the repository's branch protection is the backstop.";

/** Null for a session the guard does not cover: anything but Claude Code,
 *  or a session fleet did not start. */
export function leashFor(
  session: Pick<SessionRow, 'agent' | 'kind' | 'origin'>,
  askBeforePersonSteps: boolean,
): Leash | null {
  if (sessionAgent(session) !== 'claude') return null;
  if (session.origin === 'mission') {
    return { label: 'Refused', text: `Never. A mission worker is refused ${STEPS}; everything else runs unasked.`, caveat: CAVEAT };
  }
  if (askBeforePersonSteps) {
    return { label: 'Asks you first', text: `${STEPS_CAP}. Everything else runs unasked.`, caveat: CAVEAT };
  }
  return {
    label: 'Unasked',
    text: 'Never. Every step runs unasked, a merge and a push to main included. “Ask before a person’s step” in Settings turns it on.',
    caveat: '',
  };
}
