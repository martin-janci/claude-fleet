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
  // ── detection (work graph M4); all absent from an older hub ──
  /** The conversation the link was decided (a suggestion: last seen) in. */
  claude_session_id?: string | null;
  /** `explicit` | `strong` | `weak`. */
  strength?: string | null;
  /** The rule that made it (`R3`, `R5` …). */
  rule?: string | null;
  /** What was seen, oldest first. */
  evidence?: WorkEvidence[];
  preselected?: boolean;
  /** Why a live session's link ended (`branch_changed`, `pr_changed`). */
  end_reason?: string | null;
  /** The link's org (work graph M5): its item's, else its session's. */
  org_id?: number | null;
}

/** One evidence line of a link (`service::work::resolve::Evidence`). */
export interface WorkEvidence {
  /** `branch` | `pr_head` | `pr_closing` | `pr_text` | `trailer` |
   *  `prompt_url` | `prompt_key` | `prompt_issue` | `agent_inferred` —
   *  tolerant of more. */
  signal: string;
  rule: string;
  text: string;
  snippet?: string | null;
  at: number;
  conversation?: string | null;
  /** `reference` for a key from a reference list (the dump guard). */
  note?: string | null;
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

/** Say the session works on `ref`; it becomes the session's primary work.
 *  `forceCrossOrg`: the person saw the cross-org refusal and meant it. */
export function linkSessionWork(
  sessionId: number,
  ref: WorkRef,
  opts: { forceCrossOrg?: boolean } = {},
): Promise<Result<SessionRow>> {
  return decide('link_session_work', {
    session_id: sessionId,
    ...ref,
    ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
  });
}

/** Work graph M5: the refusal to link work of one org to a session of
 *  another (data integrity, not access control), read from an error. */
export interface CrossOrgRefusal {
  workOrgId: number;
  sessionOrgId: number;
}

export function crossOrgOf(e: { code: string; details?: unknown }): CrossOrgRefusal | null {
  const d = e.details as { cross_org?: boolean; work_org_id?: number; session_org_id?: number } | undefined;
  if (e.code !== 'E_FORBIDDEN' || !d?.cross_org) return null;
  if (typeof d.work_org_id !== 'number' || typeof d.session_org_id !== 'number') return null;
  return { workOrgId: d.work_org_id, sessionOrgId: d.session_org_id };
}

/** The sentence the UI shows before offering "Link anyway". */
export function crossOrgSentence(
  what: string,
  c: CrossOrgRefusal,
  orgName: (id: number) => string | undefined,
): string {
  const w = orgName(c.workOrgId) ?? `organisation ${c.workOrgId}`;
  const s = orgName(c.sessionOrgId) ?? `organisation ${c.sessionOrgId}`;
  return `${what} belongs to ${w}, and this session to ${s}. Fleet does not link one company's work to another's session by mistake.`;
}

/** "Not this": the session does not work on `ref`. Sticky. */
export function rejectSessionWork(sessionId: number, ref: WorkRef): Promise<Result<SessionRow>> {
  return decide('reject_session_work', { session_id: sessionId, ...ref });
}

/** Confirm a detected suggestion (work graph M4.4): it becomes the
 *  session's primary work. */
export function confirmSessionWork(
  sessionId: number,
  linkId: number,
  opts: { forceCrossOrg?: boolean } = {},
): Promise<Result<SessionRow>> {
  return decide('confirm_session_work', {
    session_id: sessionId,
    link_id: linkId,
    ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
  });
}

/** "Not this" for one detected link (a suggestion or an auto link, e.g. the
 *  Undo of an automatic link). Sticky: never proposed again. */
export function rejectWorkLink(sessionId: number, linkId: number): Promise<Result<SessionRow>> {
  return decide('reject_session_work', { session_id: sessionId, link_id: linkId });
}

/** Trust (or stop trusting) branch keys in a project: a sole branch key there
 *  then links by itself, with Undo. Answers the trusted project ids. */
export async function setWorkProjectTrust(projectId: number, on: boolean): Promise<Result<number[]>> {
  const r = await invokeCmd<{ trusted?: number[] }>('set_work_project_trust', {
    args: { project_id: projectId, on },
  });
  return r.ok ? { ok: true, value: r.value?.trusted ?? [] } : r;
}

/** Link sources detection writes; a confirmed link with one is "auto". */
export const AUTO_SOURCES: readonly string[] = ['branch', 'pr', 'trailer', 'url', 'prompt'];

/** A confirmed link detection made without a person (shown with a dot and
 *  offered for Undo). */
export function isAutoLink(w: { source: string; state?: string } | null | undefined): boolean {
  return !!w && AUTO_SOURCES.includes(w.source) && (w.state ?? 'confirmed') === 'confirmed';
}

const SOURCE_LABEL: Record<string, string> = {
  branch: 'branch',
  pr: 'pull request',
  trailer: 'commit trailer',
  url: 'ticket URL',
  prompt: 'prompt',
  manual: 'linked by you',
  started: 'started for it',
  agent: 'declared by Claude',
  agent_inferred: "Claude's guess",
  resumed: 'resumed',
  forked: 'forked',
  inherited: 'inherited',
};

/** "branch", "pull request" … for a link source. */
export function sourceLabel(source: string): string {
  return SOURCE_LABEL[source] ?? source;
}

function clock(at: number): string {
  const d = new Date(at * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** One evidence line, e.g. "branch `abc-123-login` since 09:05 · R3" or
 *  "mentioned ABC-99 in a prompt at 10:12 (reference) · R6". */
export function describeEvidence(e: WorkEvidence): string {
  const rule = e.rule ? ` · ${e.rule}` : '';
  const note = e.note ? ` (${e.note})` : '';
  switch (e.signal) {
    case 'branch':
      return `branch \`${e.text}\` since ${clock(e.at)}${rule}`;
    case 'pr_head':
      return `pull request from \`${e.text}\`${rule}`;
    case 'pr_closing':
      return `pull request closes ${e.text}${rule}`;
    case 'pr_text':
      return `named in the pull request (${e.text})${rule}`;
    case 'trailer':
      return `commit trailer ${e.text}${rule}`;
    case 'prompt_url':
      return `ticket URL in a prompt at ${clock(e.at)}${note}${rule}`;
    case 'prompt_key':
    case 'prompt_issue':
      return `mentioned ${e.text} in a prompt at ${clock(e.at)}${note}${rule}`;
    case 'agent_inferred':
      return `Claude guessed ${e.text} when asked at ${clock(e.at)}${rule}`;
    default:
      return `${e.signal}: ${e.text}${rule}`;
  }
}

/** The one-line "why" of a row's link or suggestion, for a chip tooltip. */
export function workWhy(w: { source: string; state?: string; rule?: string | null }): string {
  const what = w.state === 'suggested' ? 'suggested from the' : 'from the';
  const rule = w.rule ? ` · rule ${w.rule}` : '';
  return AUTO_SOURCES.includes(w.source)
    ? `${what} ${sourceLabel(w.source)}${rule}`
    : `${sourceLabel(w.source)}${rule}`;
}

/** Rows with a suggestion to decide, for the batch review. */
export function rowsWithSuggestions<T extends Pick<SessionRow, 'work_suggested'>>(rows: readonly T[]): T[] {
  return rows.filter((r) => !!r.work_suggested && r.work_suggested.link_id != null);
}

/** session id → primary link id of every auto link, to spot new ones. */
export function autoLinkSnapshot(rows: readonly SessionRow[]): Map<number, number> {
  const m = new Map<number, number>();
  for (const r of rows) if (r.work && isAutoLink(r.work)) m.set(r.id, r.work.link_id);
  return m;
}

/** Rows whose primary is an auto link that was not in `prev` (for the
 *  "Linked … · Undo" toast). */
export function newAutoLinks(prev: ReadonlyMap<number, number>, rows: readonly SessionRow[]): SessionRow[] {
  return rows.filter((r) => r.work && isAutoLink(r.work) && prev.get(r.id) !== r.work.link_id);
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

/**
 * Ask a live session to write its hand-off for the next session on its work
 * (work graph M9.3, on demand only). Fleet types one prompt into the idle
 * REPL; the reply is read by the next Stop hook and kept in the work journal,
 * where the resume brief shows it first. Answers the session's row.
 */
export async function requestWorkHandover(sessionId: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('request_work_handover', { args: { session_id: sessionId } });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

