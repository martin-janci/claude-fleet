// The AI check on done (docs/work-graph.md#ai-check-on-done): when a ticket's
// work is finished, a headless tester runs the project's tests and reads the
// change against the ticket. Mirrors `service::work::ticket_check`.
//
// Test now and the project switch are a person's asks; Accept anyway is the
// existing `verify_work_item` with the "ai check" line.

import { invokeCmd, type Result } from './result';
import { verifyWorkItem, type VerifyOutcome } from './missions';
import type { TaskRow } from './tasks';
import type { OfState } from './kit/status';
import { bumpWorkChanged } from './work';

export type CheckMode = 'off' | 'ask' | 'auto';
/** Every mode, in the order the switch offers them. */
export const CHECK_MODES: readonly CheckMode[] = ['off', 'ask', 'auto'];
export const CHECK_MODE_LABEL: Record<CheckMode, string> = {
  off: 'Off',
  ask: 'On request',
  auto: 'When done',
};
/** The line Accept anyway verifies (`ticket_check::CHECK_LINE`). */
export const CHECK_LINE = 'ai check';

/** `ticket_check::CheckSetup`: a project's switch. */
export interface CheckSetup {
  project_id: number;
  /** `owner/repo`. */
  project: string;
  mode: CheckMode;
  command?: string | null;
}

export type CheckState = 'ready' | 'running' | 'passed' | 'failed' | 'error' | 'accepted';

/** `ticket_check::TicketCheckView`: what a task's card says about its check. */
export interface TicketCheckView {
  state: CheckState;
  setup?: CheckSetup | null;
  task_id?: number | null;
  at?: number | null;
  summary?: string | null;
  blockers?: string[];
  tests_run?: string[];
  error?: string | null;
  note?: string | null;
  can_run?: boolean;
  why_not?: string | null;
}

/** The chip's words: a status word, then why. */
export function checkHeadline(c: TicketCheckView): { state: OfState; label: string; tone: 'success' | 'warning' | 'danger' | 'muted' } {
  switch (c.state) {
    case 'running':
      return { state: 'working', label: 'Working · testing', tone: 'muted' };
    case 'passed':
      return { state: 'done', label: 'Done · tests passed', tone: 'success' };
    case 'accepted':
      return { state: 'done', label: 'Done · accepted anyway', tone: 'success' };
    case 'failed':
      return { state: 'failed', label: 'Failed · not ready', tone: 'danger' };
    case 'error':
      return { state: 'failed', label: 'Failed · could not run', tone: 'warning' };
    default:
      return { state: 'idle', label: 'Idle · not tested yet', tone: 'muted' };
  }
}

/** Test now / Rerun: what the run button says. */
export function runLabel(c: TicketCheckView): string {
  return c.state === 'ready' ? 'Test now' : 'Rerun';
}

/** Whether Accept anyway is offered: a check that judged it not ready, or
 *  could not run. */
export function canAccept(c: TicketCheckView): boolean {
  return c.state === 'failed' || c.state === 'error';
}

export async function runTicketCheck(itemId: number): Promise<Result<TaskRow>> {
  const r = await invokeCmd<TaskRow>('run_ticket_check', { args: { item_id: itemId } });
  if (r.ok) bumpWorkChanged();
  return r;
}

export async function setTicketCheck(projectId: number, mode: CheckMode, command?: string | null): Promise<Result<CheckSetup>> {
  const c = command?.trim() ?? '';
  const r = await invokeCmd<CheckSetup>('set_ticket_check', {
    args: { project_id: projectId, mode, command: c },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}

export async function acceptCheck(itemId: number, note?: string): Promise<Result<VerifyOutcome>> {
  const r = await verifyWorkItem(itemId, CHECK_LINE, true, note);
  if (r.ok) bumpWorkChanged();
  return r;
}
