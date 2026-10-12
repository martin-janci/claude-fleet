// One task as a Work list row (redesign board "Work · tasks with filters
// open"): a status dot, the key and title, one line saying where it stands
// and why, and chips for its live session, its pull request and its spend.
// Pure: the row component reads these, and so do its tests.
import type { OfState } from './kit/status';
import { formatCostMicros, type SessionRow } from './sessions';
import { timeAgo } from './session_status';
import { blockedOnLine, occurrenceKind, WORK_STAGE_LABELS, WORK_STAGES, type WorkStage, type WorkTask, type WorkTaskLink } from './work_view';

/** The links of sessions working on it now. */
export function liveLinks(t: Pick<WorkTask, 'sessions'>): WorkTaskLink[] {
  return (t.sessions ?? []).filter((l) => l.state === 'active' && l.session_id != null);
}

function failed(l: Pick<WorkTaskLink, 'claude_status'>): boolean {
  return l.claude_status === 'failed';
}

/** The tone of a session's own dot. */
export function linkTone(l: Pick<WorkTaskLink, 'claude_status' | 'needs_you'>): OfState {
  if (l.needs_you || l.claude_status === 'blocked') return 'waiting';
  if (failed(l)) return 'failed';
  if (l.claude_status === 'working') return 'working';
  if (l.claude_status === 'completed') return 'done';
  return 'idle';
}

/** The stage the hub said, or one read from the tracker's category for an
 *  older hub that sends none. */
export function stageOf(t: Pick<WorkTask, 'stage' | 'status_category' | 'blocked' | 'counts'>): WorkStage {
  if (t.stage && (WORK_STAGES as readonly string[]).includes(t.stage)) return t.stage as WorkStage;
  if (t.status_category === 'done') return 'done';
  if (t.blocked) return 'blocked';
  if (t.status_category === 'in_progress' || (t.counts?.active ?? 0) > 0) return 'in_progress';
  return 'backlog';
}

/** The row's dot: what most needs a person first. */
export function taskTone(t: WorkTask): OfState {
  const live = liveLinks(t);
  if (t.needs_you || live.some((l) => l.needs_you)) return 'waiting';
  if (live.some(failed)) return 'failed';
  const stage = stageOf(t);
  if (stage === 'done') return 'done';
  // The status-word decision: a blocked task reads Needs you, with its
  // reason on the line (step 6.3).
  if (stage === 'blocked') return 'waiting';
  if (live.some((l) => l.claude_status === 'working') || stage === 'in_progress' || stage === 'in_review') return 'working';
  return 'idle';
}

/** The row's second line: the stage, then why. A failed session leads in
 *  red (`failed: true`) instead of the stage. */
export function taskLine(
  t: WorkTask,
  lookup?: (id: string) => Pick<WorkTask, 'key' | 'title'> | null | undefined,
): { lead: string; failed: boolean; why: string } {
  const live = liveLinks(t);
  const broken = live.find(failed);
  const stage = WORK_STAGE_LABELS[stageOf(t)];
  if (broken) return { lead: 'Session failed', failed: true, why: broken.name ?? '' };
  if (t.needs_you || live.some((l) => l.needs_you)) return { lead: stage, failed: false, why: 'session needs you' };
  if (t.blocked) return { lead: stage, failed: false, why: blockedOnLine(t, lookup) ?? 'Blocked on another task' };
  const main = live.find((l) => l.primary) ?? live[0];
  if (main?.name) return { lead: stage, failed: false, why: main.host ? `${main.name} on ${main.host}` : main.name };
  const where = t.tracker_name ?? t.project_label ?? '';
  return { lead: stage, failed: false, why: where };
}

/** `#476` from a pull request URL, else null. */
export function prNumber(url: string | null | undefined): string | null {
  const m = /\/pull\/(\d+)/.exec(url ?? '');
  return m ? `#${m[1]}` : null;
}

export interface PrChip {
  url: string;
  label: string;
  /** passing ✓, failing ✕ N, running, or unknown. */
  checks: 'passing' | 'failing' | 'running' | null;
  failing: number;
}

/** The pull request of a live session, with its checks when the session
 *  row (`sessions`) carries the last probe. */
export function prChip(t: WorkTask, rows: ReadonlyMap<number, Pick<SessionRow, 'pr_evidence'>>): PrChip | null {
  const live = liveLinks(t);
  const l = live.find((x) => x.primary && x.pr_url) ?? live.find((x) => x.pr_url);
  const url = l?.pr_url;
  if (!l || !url) return null;
  const c = rows.get(l.session_id!)?.pr_evidence?.checks;
  const failing = c?.failing_total ?? 0;
  const checks = !c || c.total === 0 ? null : failing > 0 ? 'failing' : c.pending > 0 ? 'running' : 'passing';
  return { url, label: `PR ${prNumber(url) ?? ''}`.trim(), checks, failing };
}

/** The spend chip, or null when nothing was spent. */
export function costChip(t: Pick<WorkTask, 'cost_micros'>): string | null {
  return (t.cost_micros ?? 0) > 0 ? formatCostMicros(t.cost_micros) : null;
}

/** A session chip's tooltip: what the link is, where, and why. */
export function occurrenceTitle(l: WorkTaskLink): string {
  const kind = occurrenceKind(l);
  const what =
    kind === 'primary'
      ? 'primary work of this session'
      : kind === 'secondary'
        ? 'also worked on by this session (not its primary)'
        : kind === 'suggested'
          ? 'a suggestion nobody has decided — it never groups a session'
          : kind === 'rejected'
            ? 'rejected'
            : `ended${l.ended_at ? ` ${timeAgo(l.ended_at)}` : ''}`;
  const parts = [what, l.host ?? '', l.why ?? ''].filter(Boolean);
  if (l.cross_org) parts.push('links two organisations');
  return parts.join(' · ');
}
