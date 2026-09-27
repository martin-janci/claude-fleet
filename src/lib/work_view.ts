// The Work view's commands (work graph M14.1d): typed wrappers over the
// eighteen Tauri commands of `commands/work_view.rs`, each routed to the
// hub's `work { … }` / `work_link { … }` on a paired desktop, and the
// `work:changed` frame. No UI here — that is M14.2 / M14.3. Shapes follow
// `docs/superpowers/specs/2026-09-27-work-view-design.md` → *Contracts*.

import { invokeCmd, type IpcError, type Result } from './result';
import { acceptCommandRow, type SessionRow } from './sessions';

// ── shared types ────────────────────────────────────────────────────────────

/** The same object for tree pages, saved views, desktop and phone. */
export interface WorkTreeFilters {
  /** An org id, or `"none"` (unassigned); absent: all visible. */
  org?: number | 'none';
  /** A tracker id, `"local"` (local items) or `"ref"` (bare keys). */
  tracker?: number | 'local' | 'ref';
  status?: 'any' | 'open' | 'todo' | 'in_progress' | 'done';
  mine?: boolean;
  has?: 'any' | 'active' | 'past_only' | 'none' | 'suggested';
  review?: boolean;
  query?: string;
  /** One group only (a section being expanded). */
  group?: string;
}

export interface GroupRef {
  id: string;
  label: string;
  source: string;
  rule_id?: number;
  tracker_value?: string;
  editable: boolean;
}

/** One session under a task. */
export interface WorkTaskLink {
  link_id: number;
  link_version: number;
  state: 'active' | 'ended' | 'suggested' | 'rejected' | string;
  primary: boolean;
  session_id?: number;
  name: string;
  host?: string;
  source: string;
  strength?: string;
  rule?: string;
  why: string;
  /** `task` / `session_tasks` only, never in a tree page. */
  evidence?: unknown[];
  created_at: number;
  decided_at?: number;
  ended_at?: number;
  end_reason?: string;
  claude_status?: string;
  needs_you: boolean;
  archived: boolean;
  resumable: boolean;
  branch?: string;
  pr_url?: string;
  cross_org: boolean;
  other_tasks: number;
  review_ack_at?: number;
}

export interface WorkTask {
  task_id: string;
  item_id?: number;
  key?: string;
  title: string;
  url?: string;
  kind: 'tracker' | 'local' | 'ref' | string;
  tracker_id?: number;
  tracker_name?: string;
  provider?: string;
  tracker_state?: string;
  status_category?: string;
  status_name?: string;
  resolution?: string;
  unavailable: boolean;
  unavailable_reason?: string;
  assignees?: string[];
  mine: boolean;
  org_id?: number;
  org_source: 'tracker' | 'item' | 'sessions' | 'none' | string;
  org_fenced: boolean;
  org_mixed: boolean;
  group: GroupRef;
  counts: { active: number; ended: number; suggested: number };
  needs_you: boolean;
  review: boolean;
  last_activity_at?: number;
  repos?: string[];
  /** 0: no placement. */
  placement_version: number;
  sessions: WorkTaskLink[];
  sessions_more: number;
}

export interface TreeGroup {
  org_id?: number;
  org_name?: string;
  group: GroupRef;
  count: number;
}

export interface TreePage {
  tasks: WorkTask[];
  groups: TreeGroup[];
  orgs: { id: number; name: string; color?: string }[];
  trackers: { id: number; name: string; provider: string; state: string; org_id?: number }[];
  total: number;
  next_cursor?: string;
  generated_at: number;
}

export interface Placement {
  task_id: string;
  group?: string;
  note?: string;
  version: number;
  updated_at: number;
  updated_by?: string;
}

export interface TaskDetail {
  task: WorkTask;
  aliases?: string[];
  description?: string;
  last_outcome?: {
    at: number;
    name: string;
    host?: string;
    branch?: string;
    pr_url?: string;
    end_reason?: string;
    summary?: string;
    summary_kind?: string;
  };
  placement?: Placement;
  rules?: number[];
}

export interface TaskBrief {
  task_id: string;
  key?: string;
  title: string;
  kind: string;
  status_category?: string;
  status_name?: string;
  url?: string;
  unavailable: boolean;
  org_id?: number;
  tracker_name?: string;
}

export interface SessionTasks {
  session_id: number;
  org_id?: number;
  primary_link_id?: number;
  links: (WorkTaskLink & { task: TaskBrief })[];
}

export interface ReviewItem {
  review_id: string;
  kind: 'suggestion' | 'cross_org' | 'unavailable' | 'no_primary' | string;
  session_id: number;
  session_name: string;
  host: string;
  link_id: number;
  link_version: number;
  task: TaskBrief;
  why?: string[];
  strength?: string;
  rule?: string;
  preselected: boolean;
  alternatives?: { link_id: number; task_id: string; key?: string; title: string }[];
  created_at: number;
}

export interface ReviewPage {
  items: ReviewItem[];
  total: number;
  next_cursor?: string;
}

export interface RuleConditions {
  tracker_id?: number;
  container?: string;
  key_prefix?: string;
  title_contains?: string;
  repo?: string;
}

export interface WorkRule {
  id: number;
  name: string;
  enabled: boolean;
  version: number;
  conditions: RuleConditions;
  group: string;
  created_at: number;
  updated_at: number;
}

/** A rule to preview or save: no `id` creates one. */
export interface RuleInput {
  id?: number;
  name: string;
  enabled?: boolean;
  conditions: RuleConditions;
  group: string;
  expected_version?: number;
}

export interface RulePreview {
  affected: { task_id: string; key?: string; title: string; from: GroupRef; to: GroupRef }[];
  total: number;
  kept_manual: number;
}

export interface WorkView {
  id: number;
  name: string;
  filters: WorkTreeFilters;
  owner_org?: number;
  version: number;
  updated_at: number;
}

/** A view to save: no `id` creates one. */
export interface ViewInput {
  id?: number;
  name: string;
  filters: WorkTreeFilters;
  expected_version?: number;
}

export interface OrgImpact {
  task_id: string;
  from_org?: number;
  to_org?: number;
  allowed: boolean;
  reason?: string;
  links: {
    link_id: number;
    session_id?: number;
    name: string;
    host?: string;
    state: string;
    session_org?: number;
    becomes_cross_org: boolean;
  }[];
  hosts_losing: string[];
  hosts_gaining: string[];
  bound_clients_losing: number;
  bound_clients_gaining: number;
  journal_entries: number;
  summaries: number;
  impact_token: string;
}

export type LinkDecisionKind = 'confirm' | 'reject' | 'reconsider' | 'ack';

export interface LinkDecision {
  session_id: number;
  link_id: number;
  decision: LinkDecisionKind;
  expected_version?: number;
  primary?: boolean;
}

export interface DecisionResult {
  link_id: number;
  session_id: number;
  ok: boolean;
  code?: string;
  message?: string;
  version?: number;
}

export interface BatchResult {
  results: DecisionResult[];
}

// ── reads (`work { … }`) ────────────────────────────────────────────────────

export interface TreeOpts {
  filters?: WorkTreeFilters;
  cursor?: string;
  /** 1–200, default 50. */
  limit?: number;
  /** 0–50, default 8. */
  perTask?: number;
}

/** One page of the Work view. A cursor is bound to the filters it came
 *  with: another filter set answers `E_INVALID`. */
export function workTree(opts: TreeOpts = {}): Promise<Result<TreePage>> {
  return invokeCmd<TreePage>('work_tree', {
    args: {
      ...(opts.filters ? { filters: opts.filters } : {}),
      ...(opts.cursor !== undefined ? { cursor: opts.cursor } : {}),
      ...(opts.limit !== undefined ? { limit: opts.limit } : {}),
      ...(opts.perTask !== undefined ? { per_task: opts.perTask } : {}),
    },
  });
}

export function workTask(taskId: string): Promise<Result<TaskDetail>> {
  return invokeCmd<TaskDetail>('work_task', { args: { task_id: taskId } });
}

export function workSessionTasks(sessionId: number): Promise<Result<SessionTasks>> {
  return invokeCmd<SessionTasks>('work_session_tasks', { args: { session_id: sessionId } });
}

export function workReview(opts: { cursor?: string; limit?: number } = {}): Promise<Result<ReviewPage>> {
  return invokeCmd<ReviewPage>('work_review', {
    args: {
      ...(opts.cursor !== undefined ? { cursor: opts.cursor } : {}),
      ...(opts.limit !== undefined ? { limit: opts.limit } : {}),
    },
  });
}

export function workRules(): Promise<Result<WorkRule[]>> {
  return invokeCmd<WorkRule[]>('work_rules', { args: {} });
}

export function workRulePreview(rule: RuleInput): Promise<Result<RulePreview>> {
  return invokeCmd<RulePreview>('work_rule_preview', { args: { rule } });
}

export function workViews(): Promise<Result<WorkView[]>> {
  return invokeCmd<WorkView[]>('work_views', { args: {} });
}

/** What moving a local task to `orgId` (`0`: no org) would change. Its
 *  `impact_token` is what `assignWorkOrg` must send back. */
export function workOrgImpact(taskId: string, orgId: number): Promise<Result<OrgImpact>> {
  return invokeCmd<OrgImpact>('work_org_impact', { args: { task_id: taskId, org_id: orgId } });
}

// ── writes (`work_link { … }`) ──────────────────────────────────────────────

/** A decision answers the session's row: patch it in place, as every other
 *  work decision does (`work.ts`). */
async function decide(cmd: string, args: Record<string, unknown>): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>(cmd, { args });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Make `linkId` the session's primary. `expectedPrimary`: the primary the
 *  person saw (`0`: none); a stale one answers `E_CONFLICT`. */
export function setPrimaryWork(
  sessionId: number,
  linkId: number,
  expectedPrimary?: number,
): Promise<Result<SessionRow>> {
  return decide('set_primary_work', {
    session_id: sessionId,
    link_id: linkId,
    ...(expectedPrimary !== undefined ? { expected_primary: expectedPrimary } : {}),
  });
}

/** Undo a person's confirm / reject: back to a suggestion. */
export function reconsiderWorkLink(
  sessionId: number,
  linkId: number,
  expectedVersion?: number,
): Promise<Result<SessionRow>> {
  return decide('reconsider_work_link', {
    session_id: sessionId,
    link_id: linkId,
    ...(expectedVersion !== undefined ? { expected_version: expectedVersion } : {}),
  });
}

/** Keep a conflict (cross-org, unavailable) on purpose. */
export function ackWorkLink(
  sessionId: number,
  linkId: number,
  expectedVersion?: number,
): Promise<Result<SessionRow>> {
  return decide('ack_work_link', {
    session_id: sessionId,
    link_id: linkId,
    ...(expectedVersion !== undefined ? { expected_version: expectedVersion } : {}),
  });
}

/** At most 100 decisions, each judged on its own; the answer says, per item
 *  and in order, which did what. */
export function decideWorkBatch(decisions: LinkDecision[]): Promise<Result<BatchResult>> {
  return invokeCmd<BatchResult>('decide_work_batch', { args: { decisions } });
}

/** Put a task in a group; `group` empty and no `note` clears the placement.
 *  `expectedVersion`: the task's `placement_version` as shown (0: none). */
export function placeWork(
  taskId: string,
  group: string,
  expectedVersion: number,
  note?: string,
): Promise<Result<WorkTask>> {
  return invokeCmd<WorkTask>('place_work', {
    args: {
      task_id: taskId,
      group,
      expected_version: expectedVersion,
      ...(note !== undefined ? { note } : {}),
    },
  });
}

/** Move a local task to `orgId` (`0`: none), with the token of a fresh
 *  `workOrgImpact`; a changed impact answers `E_CONFLICT` with the new one. */
export function assignWorkOrg(taskId: string, orgId: number, impactToken: string): Promise<Result<WorkTask>> {
  return invokeCmd<WorkTask>('assign_work_org', {
    args: { task_id: taskId, org_id: orgId, impact_token: impactToken },
  });
}

export function saveWorkRule(rule: RuleInput): Promise<Result<WorkRule>> {
  return invokeCmd<WorkRule>('save_work_rule', { args: { rule } });
}

export function deleteWorkRule(ruleId: number, expectedVersion?: number): Promise<Result<{ deleted: boolean }>> {
  return invokeCmd<{ deleted: boolean }>('delete_work_rule', {
    args: {
      rule_id: ruleId,
      ...(expectedVersion !== undefined ? { expected_version: expectedVersion } : {}),
    },
  });
}

export function saveWorkView(view: ViewInput): Promise<Result<WorkView>> {
  return invokeCmd<WorkView>('save_work_view', { args: { view } });
}

export function deleteWorkView(viewId: number, expectedVersion?: number): Promise<Result<{ deleted: boolean }>> {
  return invokeCmd<{ deleted: boolean }>('delete_work_view', {
    args: {
      view_id: viewId,
      ...(expectedVersion !== undefined ? { expected_version: expectedVersion } : {}),
    },
  });
}

// ── errors ──────────────────────────────────────────────────────────────────

/** An `E_CONFLICT` answer's current value (a version, a primary, a fresh
 *  impact), or `null` for any other error. The UI shows it with *Reload* and
 *  never overwrites silently. */
export function conflictOf(e: IpcError): Record<string, unknown> | null {
  if (e.code !== 'E_CONFLICT') return null;
  const d = e.details;
  return d && typeof d === 'object' && !Array.isArray(d) ? (d as Record<string, unknown>) : {};
}

/** An older hub, which does not list the Work view's actions, answers
 *  `E_INVALID unknown work(_link) action …`: the UI says "Needs a newer hub". */
export function needsNewerHub(e: IpcError): boolean {
  return e.code === 'E_INVALID' && /unknown work(_link)? action/.test(e.message);
}

// ── the `work:changed` frame ────────────────────────────────────────────────

/** `work:changed`: ids only. `resync` is the desktop's own: its hub stream
 *  was re-established after a gap (`lagged`, or a connection that did not
 *  resume), so nothing short of a whole reload is current. */
export type WorkChanged =
  | { what: 'placement'; task_id?: string }
  | { what: 'org'; task_id?: string }
  | { what: 'rule'; rule_id?: number }
  | { what: 'view'; view_id?: number }
  | { what: 'resync' };

const WHATS = new Set(['placement', 'org', 'rule', 'view', 'resync']);

/** Read one `work:changed` payload; `null` for anything malformed or a
 *  `what` this build does not know (a newer hub's), which the caller drops. */
export function parseWorkChanged(payload: unknown): WorkChanged | null {
  if (!payload || typeof payload !== 'object') return null;
  const p = payload as Record<string, unknown>;
  if (typeof p.what !== 'string' || !WHATS.has(p.what)) return null;
  switch (p.what) {
    case 'placement':
    case 'org':
      if (p.task_id !== undefined && typeof p.task_id !== 'string') return null;
      return p.task_id === undefined ? { what: p.what } : { what: p.what, task_id: p.task_id };
    case 'rule':
      if (p.rule_id !== undefined && typeof p.rule_id !== 'number') return null;
      return p.rule_id === undefined ? { what: 'rule' } : { what: 'rule', rule_id: p.rule_id };
    case 'view':
      if (p.view_id !== undefined && typeof p.view_id !== 'number') return null;
      return p.view_id === undefined ? { what: 'view' } : { what: 'view', view_id: p.view_id };
    default:
      return { what: 'resync' };
  }
}

/** Whether a batch of changes calls for reloading the whole view rather
 *  than re-reading the visible page: a gap in the stream. */
export function needsFullReload(changes: readonly WorkChanged[]): boolean {
  return changes.some((c) => c.what === 'resync');
}
