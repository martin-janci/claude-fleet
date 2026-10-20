// A task's result (design 2026-10-13, D1 / D6): the newest finished task of
// a work item that carries a report, joined on read with what fleet observed
// itself — git evidence, the completion gate, the PRs — and the follow-ups
// that already became tasks. Mirrors `service::work::result::TaskResultView`.
//
// The report is what the worker said; the gate and the evidence are what
// fleet read. `validationLines` keeps the two apart.

import { invokeCmd, type Result } from './result';
import { reportFromValue, type TaskReport } from './rich_blocks';
import type { TaskEvidence } from './missions';
import { outcomeLabel } from './mission_triage';
import type { TaskRow } from './tasks';
import type { WorkItemRow } from './trackers';
import { bumpWorkChanged } from './work';

export type { TaskEvidence };

/** One `done_when` line and what fleet found for it (`service::work::gate`). */
export interface GateCheck {
  line: string;
  kind: string;
  /** `pass` | `fail`; anything else is not decided yet. */
  state: string;
  detail: string;
}

/** The completion gate: `blocked` when a check failed or a subtask is open,
 *  `unverified` when lines exist and not all passed, `passed` otherwise. */
export interface Gate {
  state: 'passed' | 'unverified' | 'blocked';
  checks: GateCheck[];
  open_children: { id: number; title: string }[];
}

export interface ResultPr {
  url: string;
  number: number | null;
  state: string | null;
  ci_status: string | null;
  review_decision: string | null;
}

export interface TaskResultView {
  task_id: number;
  item_id: number;
  session_id: number | null;
  role: string;
  at: number;
  /** The done marker the report followed: the conversation matches on it. */
  marker: string;
  report: TaskReport;
  evidence: TaskEvidence | null;
  prs: ResultPr[];
  gate: Gate;
  /** The report's follow-ups, each with the task it became, if any. */
  followups: { text: string; item_id: number | null }[];
}

/** The result of a work item, or of the item a session works on; `null`
 *  when no finished task carries a report yet. */
export async function workResult(by: { itemId: number } | { sessionId: number }): Promise<Result<TaskResultView | null>> {
  const args = 'itemId' in by ? { item_id: by.itemId } : { session_id: by.sessionId };
  const r = await invokeCmd<TaskResultView | null>('work_result', { args });
  return r.ok && r.value ? { ok: true, value: normalizeResult(r.value) } : r;
}

/** The view as the panel reads it. The backend leaves out an empty list and
 *  an unset option (`skip_serializing_if`), so a report with no blockers
 *  arrives with no `blockers` at all: every list and nullable is filled in
 *  here, once, rather than at each read. */
export function normalizeResult(v: TaskResultView): TaskResultView {
  return {
    ...v,
    session_id: v.session_id ?? null,
    report: reportFromValue(v.report) ?? (reportFromValue({}) as TaskReport),
    evidence: v.evidence ?? null,
    prs: (v.prs ?? []).map((p) => ({
      url: p.url,
      number: p.number ?? null,
      state: p.state ?? null,
      ci_status: p.ci_status ?? null,
      review_decision: p.review_decision ?? null,
    })),
    gate: { state: v.gate.state, checks: v.gate.checks ?? [], open_children: v.gate.open_children ?? [] },
    followups: (v.followups ?? []).map((f) => ({ text: f.text, item_id: f.item_id ?? null })),
  };
}

/**
 * "Finish task": ask a live session for its result (`work_link { wrap_up }`).
 * Fleet types the run instruction into the idle REPL; the reply is read by
 * the next Stop hook. Answers the `wrapup` task that waits for it.
 */
export function requestWorkResult(sessionId: number): Promise<Result<TaskRow>> {
  return invokeCmd<TaskRow>('request_work_result', { args: { session_id: sessionId } });
}

/** Turn a follow-up of a result into a task: one of the report's lines by
 *  `index`, or a `title` of the person's own. Asking twice answers the same
 *  item. */
export async function createFollowupTask(
  taskId: number,
  pick: { index: number } | { title: string },
): Promise<Result<WorkItemRow>> {
  const args = 'index' in pick ? { task_id: taskId, index: pick.index } : { task_id: taskId, title: pick.title };
  const r = await invokeCmd<WorkItemRow>('create_followup_task', { args });
  if (r.ok) bumpWorkChanged();
  return r;
}

/** What "Ask about result" puts in the session's composer. */
export const ASK_ABOUT_RESULT = 'About the result of this task: ';

export type ValidationState = 'pass' | 'fail' | 'pending' | 'reported';

/** What speaks for the result: the gate's passed checks first (fleet read
 *  them), then the tests the worker says it ran, marked as reported. A check
 *  that failed or is still pending is listed once, with its detail, under
 *  the gate's own "Blocks completion" / "Not verified yet" group. */
export function validationLines(v: TaskResultView): { label: string; state: ValidationState }[] {
  return [
    ...v.gate.checks.filter((c) => c.state === 'pass').map((c) => ({ label: c.line, state: 'pass' as const })),
    ...v.report.tests_run.map((t) => ({ label: t, state: 'reported' as const })),
  ];
}

/** The panel's one-line verdict. "Task completed" only when the worker
 *  reported done AND the gate passed. */
export function headline(v: TaskResultView): { label: string; tone: 'success' | 'warning' | 'danger' } {
  // An outcome that is not done keeps the words ReportCard gives it: the
  // manual has no "Blocked" or "Partly done" status (copy_lint).
  switch (v.report.outcome) {
    case 'failed':
    case 'blocked':
      return { label: outcomeLabel(v.report.outcome), tone: 'danger' };
    case 'partial':
      return { label: outcomeLabel('partial'), tone: 'warning' };
  }
  if (v.gate.state === 'blocked') return { label: 'Completion blocked', tone: 'danger' };
  if (v.gate.state === 'unverified') return { label: 'Reported done, not verified', tone: 'warning' };
  return { label: 'Task completed', tone: 'success' };
}
