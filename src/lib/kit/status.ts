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

/** The six status words (manual, content rules): the five states' words,
 *  plus Paused for an idle row that says why. A status label is one of these,
 *  optionally followed by " · " and the reason; `copy_lint.test.ts` holds the
 *  app to it. */
export const STATUS_WORDS = ['Needs you', 'Working', 'Failed', 'Done', 'Paused', 'Idle'] as const;
export type StatusWord = (typeof STATUS_WORDS)[number];

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
