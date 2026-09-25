/**
 * Multi-repo start (work graph M9.6): one ticket, one sibling session per
 * repository, all on one branch name (decision D11). Types mirror
 * `service::trackers::tickets::MultiStart`. The dialog offers the projects
 * the key already ran in (past links and live sessions) — decision D11's
 * "projects the key ran in before".
 */

import { invokeCmd, type Result } from './result';
import { acceptCommandRow, type SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';
import type { StartWorkArgs } from './trackers';
import type { WorkLink } from './work';
import { push, pushError, type PushOptions } from './toasts';

export interface StartSkip {
  project_id: number;
  session_id?: number | null;
  reason: string;
}

export interface StartFailure {
  project_id: number;
  code: string;
  message: string;
  /** Refused by the cross-org rule: `force_cross_org` would start it. */
  cross_org?: boolean;
}

/** A session that started but whose link or brief failed (it runs). */
export interface StartWarning {
  project_id: number;
  session_id: number;
  code: string;
  message: string;
}

/** A skip's reason when the start ran out of time before that repository. */
export const SKIP_DEADLINE = 'deadline';

export interface MultiStart {
  key: string;
  started: SessionRow[];
  warnings?: StartWarning[];
  skipped?: StartSkip[];
  failed?: StartFailure[];
}

/** Start one ticket in several repositories, on the hub when paired. */
export async function startWorkMulti(
  args: StartWorkArgs & { project_ids: number[] },
): Promise<Result<MultiStart>> {
  const r = await invokeCmd<MultiStart>('start_work_multi', { args });
  if (r.ok) for (const row of r.value.started ?? []) acceptCommandRow(row);
  return r;
}

/** A project another sibling could start in. */
export interface SiblingCandidate {
  id: number;
  label: string;
}

/**
 * The projects `key` ran in before, other than `projectId`: ended links'
 * `snap_project_id` and live sessions carrying the key. Newest first,
 * limited to projects the fleet still has, system projects left out.
 */
export function siblingCandidates(
  key: string,
  projectId: number,
  ended: readonly WorkLink[],
  live: readonly Pick<SessionRow, 'project_id' | 'work' | 'last_activity_at'>[],
  projects: readonly ProjectTreeRow[],
): SiblingCandidate[] {
  const seen: { id: number; at: number }[] = [];
  const add = (id: number | null | undefined, at: number) => {
    if (id == null || id === projectId) return;
    const had = seen.find((x) => x.id === id);
    if (had) had.at = Math.max(had.at, at);
    else seen.push({ id, at });
  };
  for (const l of ended) add(l.snap_project_id, l.ended_at ?? l.created_at);
  for (const s of live) if (s.work?.key === key) add(s.project_id, s.last_activity_at);
  const byId = new Map(projects.map((p) => [p.project.id, p.project]));
  return seen
    .sort((a, b) => b.at - a.at)
    .flatMap((x) => {
      const p = byId.get(x.id);
      if (!p || p.system) return [];
      return [{ id: p.id, label: `${p.owner}/${p.repo}` }];
    });
}

/**
 * The ticked sibling projects still on offer, in ticking order: a tick left
 * over from another key (or a candidate that has since gone) is dropped, so
 * a start never reaches a repository the dialog does not show.
 */
export function shownSiblings(ticked: readonly number[], offered: readonly SiblingCandidate[]): number[] {
  return ticked.filter((id) => offered.some((c) => c.id === id));
}

/** One line on what a multi-repo start left out, or null when nothing. */
export function multiStartNote(r: MultiStart, labelOf: (projectId: number) => string): string | null {
  const parts: string[] = [];
  for (const s of r.skipped ?? []) {
    parts.push(`${labelOf(s.project_id)}: ${s.reason === SKIP_DEADLINE ? 'not started, out of time' : 'already running'}`);
  }
  for (const w of r.warnings ?? []) parts.push(`${labelOf(w.project_id)}: started, but ${w.message}`);
  for (const f of r.failed ?? []) parts.push(`${labelOf(f.project_id)}: ${f.message}`);
  return parts.length > 0 ? `Started ${r.started.length}; ${parts.join('; ')}` : null;
}

/**
 * The note's toast kind: a warning when anything failed, started half-way
 * or ran out of time; info when the rest only already ran.
 */
export function multiStartKind(r: MultiStart): 'info' | 'warning' {
  const late = (r.skipped ?? []).some((s) => s.reason === SKIP_DEADLINE);
  return (r.failed ?? []).length > 0 || (r.warnings ?? []).length > 0 || late ? 'warning' : 'info';
}

/** The repositories only the cross-org rule refused ("Start anyway"). */
export function crossOrgRetry(r: MultiStart): number[] {
  return (r.failed ?? []).filter((f) => f.cross_org).map((f) => f.project_id);
}

/**
 * The toast for a multi-repo start's result, or null when nothing was left
 * out. A cross-org refusal offers "Start anyway": the same start again, in
 * those repositories only, with `force_cross_org` — its own result toasted
 * the same way.
 */
export function multiStartToast(
  args: StartWorkArgs & { project_ids: number[] },
  r: MultiStart,
  labelOf: (projectId: number) => string,
): PushOptions | null {
  const message = multiStartNote(r, labelOf);
  if (!message) return null;
  const retry = crossOrgRetry(r);
  const opts: PushOptions = { kind: multiStartKind(r), message };
  if (retry.length > 0 && !args.force_cross_org) {
    opts.sticky = true;
    opts.action = {
      label: 'Start anyway',
      run: () => {
        const again = { ...args, project_ids: retry, force_cross_org: true };
        void startWorkMulti(again).then((res) => {
          if (!res.ok) {
            pushError(res.error, 'Start anyway failed');
            return;
          }
          push(multiStartToast(again, res.value, labelOf) ?? { kind: 'success', message: `Started ${res.value.started.length}` });
        });
      },
    };
  }
  return opts;
}
