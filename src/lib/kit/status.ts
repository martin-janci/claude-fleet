// The manual's five states and their words (StatusChip). Map domain states
// onto these: a question, permission, grant to sign or push to approve is
// waiting ("Needs you"); stuck, CI red and blocked are failed; completed,
// CI green and merged are done; paused, stopped and queued are idle.
export type OfState = 'waiting' | 'working' | 'failed' | 'done' | 'idle';

export const OF_STATES: readonly OfState[] = ['waiting', 'working', 'failed', 'done', 'idle'];

export const STATE_WORD: Record<OfState, string> = {
  waiting: 'Needs you',
  working: 'Working',
  failed: 'Failed',
  done: 'Done',
  idle: 'Idle',
};

/** One answer on a QuestionCard, in the agent's own order. */
export interface Answer {
  label: string;
  /** The consumer's primary, never the AI's pick on a push or permission. */
  primary?: boolean;
  onselect: () => void;
  /** The key hint when it is not the answer's position (an agent's own
   *  ordinal); the number keys follow it too. */
  kbd?: string;
  disabled?: boolean;
  title?: string;
  /** A multi-select answer: ticked or not. Absent on a single choice. */
  checked?: boolean;
  testid?: string;
}
