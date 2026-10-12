// Send later from the composer (Orbit Fleet gap plan step G2.7, the
// FormsSession board's "composer › Send later"): the time choices turned into
// the `queue_prompt` timing G1.8 added (`not_before`, `until_limit_reset`,
// `skip_if_archived`). Pure and clock-taking, so the tests hold the time;
// SendLaterSheet.svelte renders it.
import type { SendLaterTiming } from './sessions';

export type SendLaterChoice = 'idle' | 'hour' | 'tomorrow' | 'limit' | 'at';

export const SEND_LATER_CHOICES: readonly { value: SendLaterChoice; label: string }[] = [
  { value: 'idle', label: 'When it is idle' },
  { value: 'hour', label: 'In 1 hour' },
  { value: 'tomorrow', label: 'Tomorrow 09:00' },
  { value: 'limit', label: 'When the usage limit resets' },
  { value: 'at', label: 'At…' },
];

/** 09:00 local time on the day after `nowMs`. */
export function tomorrowAtNine(nowMs: number): Date {
  const d = new Date(nowMs);
  d.setDate(d.getDate() + 1);
  d.setHours(9, 0, 0, 0);
  return d;
}

/** A `datetime-local` value ("2026-10-11T09:00") as local time; null when
 *  it does not read as one. */
export function parseLocalDateTime(value: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value.trim());
  if (!m) return null;
  const [y, mo, d, h, mi] = m.slice(1).map(Number);
  const out = new Date(y, mo - 1, d, h, mi, 0, 0);
  return out.getMonth() === mo - 1 && out.getDate() === d ? out : null;
}

/** `Date` → the `datetime-local` value for it, in local time. */
export function toLocalDateTime(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

export interface SendLaterPlan {
  timing: SendLaterTiming;
  /** When it goes, for the toast: "at Sat 09:00", "when it is idle". */
  when: string;
  /** Why it cannot be scheduled; null when it can. */
  error: string | null;
}

function whenAt(d: Date, locale?: string): string {
  const day = new Intl.DateTimeFormat(locale, { weekday: 'short' }).format(d);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${day} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The `queue_prompt` timing for a choice at `nowMs`. `at` is the
 *  `datetime-local` value of "At…". */
export function sendLaterPlan(
  choice: SendLaterChoice,
  nowMs: number,
  at: string,
  skipIfArchived: boolean,
  locale?: string,
): SendLaterPlan {
  const skip = skipIfArchived ? { skipIfArchived: true } : {};
  switch (choice) {
    case 'idle':
      return { timing: { ...skip }, when: 'when it is idle', error: null };
    case 'hour': {
      const d = new Date(nowMs + 3600_000);
      return { timing: { notBefore: Math.floor(d.getTime() / 1000), ...skip }, when: `at ${whenAt(d, locale)}`, error: null };
    }
    case 'tomorrow': {
      const d = tomorrowAtNine(nowMs);
      return { timing: { notBefore: Math.floor(d.getTime() / 1000), ...skip }, when: `at ${whenAt(d, locale)}`, error: null };
    }
    case 'limit':
      return { timing: { untilLimitReset: true, ...skip }, when: 'when the usage limit resets', error: null };
    case 'at': {
      const d = parseLocalDateTime(at);
      if (!d) return { timing: {}, when: '', error: 'Pick a day and a time.' };
      if (d.getTime() <= nowMs) return { timing: {}, when: '', error: 'That time has passed. Pick a later one.' };
      return { timing: { notBefore: Math.floor(d.getTime() / 1000), ...skip }, when: `at ${whenAt(d, locale)}`, error: null };
    }
  }
}
