// Work links (roadmap M1b.2): which work a session is doing, decided by a
// person. Thin wrappers over the `*_session_work` commands; each mutation
// answers the session's updated row (its `work` / `work_rejected`), which is
// patched into the sessions store right away — the `session:updated` event
// the backend also emits then lands as a no-op.

import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { acceptCommandRow, type SessionRow } from './sessions';
import { timeAgo } from './session_status';

/** One session ↔ work link (`store::WorkLinkRow`). The snapshot fields are
 *  set once the session has ended. */
export interface WorkLink {
  id: number;
  item_id?: number | null;
  ref_key?: string | null;
  participant_id?: number | null;
  /** `confirmed` | `rejected` — tolerant of values a newer hub adds. */
  state: string;
  source: string;
  is_primary?: boolean;
  created_at: number;
  decided_at?: number | null;
  ended_at?: number | null;
  snap_host?: string | null;
  snap_tmux?: string | null;
  snap_name?: string | null;
  snap_project_id?: number | null;
  snap_worktree?: string | null;
  snap_branch?: string | null;
  snap_pr_url?: string | null;
  snap_claude_ids?: string | null;
  /** `work` | `review` | `worker` (inherited from a parent). Absent from an
   *  older hub. */
  role?: string;
  /** `false` once a purge removed the transcripts this link would resume
   *  from. Absent (= resumable) from an older hub. */
  resumable?: boolean;
}

/** What a decision is about: a key / free-form name, or a work item. */
export type WorkRef = { key: string } | { item_id: number };

/** A session's live links, primary first. */
export function sessionWorkLinks(sessionId: number): Promise<Result<WorkLink[]>> {
  return invokeCmd<WorkLink[]>('session_work_links', { args: { session_id: sessionId } });
}

async function decide(cmd: string, args: Record<string, unknown>): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>(cmd, { args });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Say the session works on `ref`; it becomes the session's primary work. */
export function linkSessionWork(sessionId: number, ref: WorkRef): Promise<Result<SessionRow>> {
  return decide('link_session_work', { session_id: sessionId, ...ref });
}

/** "Not this": the session does not work on `ref`. Sticky. */
export function rejectSessionWork(sessionId: number, ref: WorkRef): Promise<Result<SessionRow>> {
  return decide('reject_session_work', { session_id: sessionId, ...ref });
}

/** Remove a mistaken link (not a rejection: the key may come back). */
export function unlinkSessionWork(sessionId: number, linkId: number): Promise<Result<SessionRow>> {
  return decide('unlink_session_work', { session_id: sessionId, link_id: linkId });
}

// ── Past work and resume (roadmap M2) ──

/** The key an ended link is past work of: its bare key. (Links to a local
 *  item carry `item_id` instead; no UI creates those yet.) */
export function linkKey(l: WorkLink): string | null {
  return l.ref_key ?? null;
}

/** The ended links to one key, newest first. */
export function endedWorkLinks(key: string): Promise<Result<WorkLink[]>> {
  return invokeCmd<WorkLink[]>('session_work_links', { args: { key } });
}

/** Links that ended within the hub's `work.recent_days`, newest first. An
 *  older hub refuses the empty read (`E_INVALID`): no past-only groups then. */
export function recentEndedWorkLinks(): Promise<Result<WorkLink[]>> {
  return invokeCmd<WorkLink[]>('session_work_links', { args: {} });
}

/** key → its ended links, newest first: the sidebar's past work. */
export const pastWork = writable<Map<string, WorkLink[]>>(new Map());

let loadSeq = 0;

/** Load past work for the sidebar: every recently ended link, plus the older
 *  past of each key in `liveKeys` (a live group's Done section). Failures
 *  leave that part empty; a stale answer never overwrites a newer one. */
export async function loadPastWork(liveKeys: readonly string[]): Promise<Map<string, WorkLink[]>> {
  const seq = ++loadSeq;
  const byKey = new Map<string, Map<number, WorkLink>>();
  const add = (l: WorkLink) => {
    const k = linkKey(l);
    if (!k || l.ended_at == null || l.state !== 'confirmed') return;
    let m = byKey.get(k);
    if (!m) byKey.set(k, (m = new Map()));
    m.set(l.id, l);
  };
  const [recent, ...perKey] = await Promise.all([
    recentEndedWorkLinks(),
    ...liveKeys.map((k) => endedWorkLinks(k)),
  ]);
  if (recent.ok && Array.isArray(recent.value)) recent.value.forEach(add);
  for (const r of perKey) if (r.ok && Array.isArray(r.value)) r.value.forEach(add);
  const out = new Map<string, WorkLink[]>();
  for (const [k, m] of byKey) {
    out.set(
      k,
      [...m.values()].sort((a, b) => (b.ended_at ?? 0) - (a.ended_at ?? 0) || b.id - a.id),
    );
  }
  if (seq === loadSeq) pastWork.set(out);
  return out;
}

/** One resume mode and why it is not possible (`service::work::resume`). */
export interface ResumeMode {
  mode: 'last' | 'brief' | 'fresh' | string;
  ok: boolean;
  reason?: string | null;
}

export interface ResumeCandidate {
  link_id: number;
  ended_at?: number | null;
  name?: string | null;
  host_alias?: string | null;
  branch?: string | null;
  worktree?: string | null;
  pr_url?: string | null;
  conversations?: number;
  last_claude_session_id?: string | null;
  resumable?: boolean;
}

export interface LiveWork {
  session_id: number;
  host_alias: string;
  tmux_name: string;
  friendly_name?: string | null;
}

/** What a resume of a key would do (`work { action: resume_plan }`). */
export interface ResumePlan {
  key: string;
  title?: string | null;
  live?: LiveWork[];
  candidates?: ResumeCandidate[];
  link_id?: number | null;
  host_alias?: string | null;
  project_id?: number | null;
  branch?: string | null;
  worktree?: string | null;
  worktree_present?: boolean;
  modes: ResumeMode[];
  hosts?: string[];
  brief?: string | null;
}

export interface ResumePlanOpts {
  linkId?: number | null;
  hostAlias?: string | null;
  withBrief?: boolean;
}

/** A hub older than resume answers the plan read with its link list (or an
 *  error): say so instead of offering buttons that cannot work. */
export const RESUME_UNSUPPORTED = 'Resume needs a newer hub: update fleet-hub to resume past work.';

export async function workResumePlan(key: string, opts: ResumePlanOpts = {}): Promise<Result<ResumePlan>> {
  const r = await invokeCmd<ResumePlan>('work_resume_plan', {
    args: {
      key,
      link_id: opts.linkId ?? null,
      host_alias: opts.hostAlias ?? null,
      with_brief: opts.withBrief ?? false,
    },
  });
  if (r.ok && (!r.value || !Array.isArray(r.value.modes))) {
    return { ok: false, error: { code: 'E_UNSUPPORTED', message: RESUME_UNSUPPORTED } };
  }
  if (!r.ok && (r.error.code === 'E_PARSE' || r.error.code === 'E_UNKNOWN_COMMAND')) {
    return { ok: false, error: { code: 'E_UNSUPPORTED', message: RESUME_UNSUPPORTED } };
  }
  return r;
}

export interface ResumeWorkArgs {
  key: string;
  mode: 'last' | 'brief' | 'fresh';
  linkId?: number | null;
  hostAlias?: string | null;
  /** The brief as edited in the preview (`brief` mode). */
  brief?: string | null;
}

/** Start a session on past work; the new row is patched in at once. */
export async function resumeWork(a: ResumeWorkArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('resume_work', {
    args: {
      key: a.key,
      mode: a.mode,
      link_id: a.linkId ?? null,
      host_alias: a.hostAlias ?? null,
      brief: a.brief ?? null,
    },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** The work keys a purge of `projectId` on `hosts` would leave without
 *  resumable conversations. */
export async function workPurgeImpact(projectId: number, hosts: string[]): Promise<Result<string[]>> {
  const r = await invokeCmd<{ keys?: string[] }>('work_purge_impact', {
    args: { project_id: projectId, host_aliases: hosts },
  });
  return r.ok ? { ok: true, value: r.value?.keys ?? [] } : r;
}

/** "2 sessions, last 3d ago" for a key's past work. */
export function pastWorkSummary(links: readonly WorkLink[], nowMs: number = Date.now()): string {
  const n = links.length;
  const last = links.reduce((m, l) => Math.max(m, l.ended_at ?? 0), 0);
  const ago = last > 0 ? `, last ${timeAgo(last, nowMs)}` : '';
  return `${n} session${n === 1 ? '' : 's'}${ago}`;
}

