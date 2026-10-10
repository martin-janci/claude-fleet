// The task page's Delivery block and the start rule that places it (gap
// plan G3.4, the TaskDetail board): where the work stands as a delivery,
// its pull request and checks, the tracker's column, what its sessions
// spent and over how long, and who owns it by when. Pure: the page and its
// tests read these.
import { formatDuration } from './account_usage';
import type { SessionRow } from './sessions';
import { ownerDueChip } from './work';
import { prChip, prNumber, type PrChip } from './work_row';
import { taskSpend, type LastOutcome, type WorkTask } from './work_view';
import type { StartRule } from './start_rules';

/** The Task detail's tabs. Comments are not one: fleet keeps no comments on
 *  a task (cut, gap plan decisions). */
export const TASK_TABS = ['overview', 'sessions', 'activity'] as const;
export type TaskTab = (typeof TASK_TABS)[number];
export const TASK_TAB_LABELS: Record<TaskTab, string> = { overview: 'Overview', sessions: 'Sessions', activity: 'Activity' };

export interface Delivery {
  /** A live session's PR with its checks, else the last outcome's PR. */
  pr: PrChip | null;
  /** The tracker's own column ("QA Review"), when it reports one. */
  column: string | null;
  /** "$4.20" across its sessions, each counted once. */
  spend: string | null;
  /** "2d 4h": from its first session's link to the last end, or now
   *  while one is active. */
  duration: string | null;
  /** "You · Fri", "Ana +1 · overdue". */
  owner: { text: string; overdue: boolean } | null;
}

/** The time its sessions span, in seconds; `null` with no dated link. */
export function workSpan(t: Pick<WorkTask, 'sessions'>, nowSec: number): number | null {
  const links = t.sessions ?? [];
  const starts = links.map((l) => l.created_at).filter((x): x is number => typeof x === 'number');
  if (starts.length === 0) return null;
  const live = links.some((l) => l.state === 'active');
  const ends = links.map((l) => l.ended_at).filter((x): x is number => typeof x === 'number');
  const end = live || ends.length === 0 ? nowSec : Math.max(...ends);
  return Math.max(0, end - Math.min(...starts));
}

export function deliveryOf(
  t: WorkTask,
  lastOutcome: LastOutcome | null | undefined,
  rows: ReadonlyMap<number, Pick<SessionRow, 'pr_evidence'>>,
  nowSec: number,
  now: Date = new Date(nowSec * 1000),
): Delivery {
  let pr = prChip(t, rows);
  if (!pr && lastOutcome?.pr_url) {
    pr = { url: lastOutcome.pr_url, label: `PR ${prNumber(lastOutcome.pr_url) ?? ''}`.trim(), checks: null, failing: 0 };
  }
  const span = workSpan(t, nowSec);
  return {
    pr,
    column: t.status_name?.trim() || null,
    spend: taskSpend(t),
    duration: span == null ? null : formatDuration(span),
    owner: ownerDueChip(t, now),
  };
}

/** Is there anything to show in the block? */
export function hasDelivery(d: Delivery): boolean {
  return !!(d.pr || d.column || d.spend || d.duration || d.owner);
}

/** Mirrors `service::start_rules::glob_match`: `*` is any run of
 *  characters, the rest matches itself, ASCII case ignored. */
export function globMatch(pattern: string, key: string): boolean {
  const p = [...pattern.toUpperCase()];
  const k = [...key.toUpperCase()];
  let pi = 0;
  let ki = 0;
  let star: [number, number] | null = null;
  while (ki < k.length) {
    if (pi < p.length && p[pi] === '*') {
      star = [pi, ki];
      pi++;
    } else if (pi < p.length && p[pi] === k[ki]) {
      pi++;
      ki++;
    } else if (star) {
      pi = star[0] + 1;
      ki = star[1] + 1;
      star = [star[0], star[1] + 1];
    } else return false;
  }
  while (pi < p.length && p[pi] === '*') pi++;
  return pi === p.length;
}

/** The active start rule that would place a start of `key`: the most
 *  specific match (most characters that are not `*`), as the backend
 *  picks it. `null` for a task with no key or no rule. */
export function startRuleFor<R extends Pick<StartRule, 'pattern' | 'state'>>(key: string | null | undefined, rules: readonly R[]): R | null {
  if (!key) return null;
  const spec = (p: string) => [...p].filter((c) => c !== '*').length;
  return (
    rules
      .filter((r) => r.state === 'active' && globMatch(r.pattern, key))
      .sort((a, b) => spec(b.pattern) - spec(a.pattern))[0] ?? null
  );
}

/** The subtasks header's "2 / 4" (G7.6, TaskDetail board): how many are
 *  done of how many. `null` with none. */
export function subtaskProgress(subtasks: readonly { status?: string | null }[] | null | undefined): string | null {
  const all = subtasks ?? [];
  if (all.length === 0) return null;
  return `${all.filter((s) => s.status === 'done').length} / ${all.length}`;
}

/** A subtask's mark: ✓ done, ◐ in progress, ○ anything else. */
export function subtaskMark(status: string | null | undefined): { glyph: string; label: string } {
  if (status === 'done') return { glyph: '✓', label: 'done' };
  if (status === 'in_progress') return { glyph: '◐', label: 'in progress' };
  return { glyph: '○', label: 'to do' };
}
