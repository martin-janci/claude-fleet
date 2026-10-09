// Orbit Fleet 11.11: "Since 13:20", a short summary of what a session did
// since a time, for whoever watches it (and at the top of Details). Drafted
// on the session's own host under its account, only with its org's consent,
// and hidden when Jev cannot confirm it against the transcript
// (`decide.jev.summary_check`).
import { invokeCmd, type Result } from './result';

/** What J9 said: `off` and `shadow` show unchecked; `failed` and
 *  `unchecked` hide the text. Mirrors `summary_check::Check`. */
export type SummaryCheck = 'off' | 'shadow' | 'passed' | 'failed' | 'unchecked';

/** Mirrors `watch_summary::WatchSummary`. */
export interface WatchSummary {
  /** `null`/absent: nothing happened since, or the check hid it. */
  text?: string | null;
  check: SummaryCheck;
  since: number;
  turns: number;
  model: string;
  host_alias: string;
  at: number;
}

export function sessionSummarySince(sessionId: number, since: number): Promise<Result<WatchSummary>> {
  return invokeCmd<WatchSummary>('session_summary_since', { args: { session_id: sessionId, since } });
}

/** The window's start when the person names none: when they last looked at
 *  the session, if that was within a day, else an hour ago. Unix seconds. */
export function defaultSince(lastViewedAt: number | null | undefined, now: number): number {
  if (lastViewedAt && lastViewedAt < now && now - lastViewedAt <= 86_400) return lastViewedAt;
  return now - 3_600;
}

/** "13:20": a unix time as local HH:MM. */
export function clock(at: number): string {
  const d = new Date(at * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** The line under the summary: what checked it. */
export function checkLabel(c: SummaryCheck): string {
  switch (c) {
    case 'passed':
      return 'checked against the transcript by Jev';
    case 'failed':
      return 'hidden: Jev found it does not match the transcript';
    case 'unchecked':
      return 'hidden: it could not be checked against the transcript';
    default:
      return 'not checked against the transcript';
  }
}
