/**
 * The Work view (work graph M14): organisation → group → task → every
 * session of the task, the second projection of the same work graph the
 * Sessions tree shows the other way round.
 *
 * Types mirror the wire contract in
 * `docs/superpowers/specs/2026-09-27-work-view-design.md` ("Contracts"):
 * the reads are `work { tree | task | session_tasks | review | rules |
 * rule_preview | views | org_impact }`, the writes `work_link { … }`, and
 * every desktop command takes `{ args: { …fields of the action… } }`. Every
 * field a newer hub may add is optional or tolerated; an unknown value reads
 * as its most cautious meaning (an unknown link state is never "active").
 *
 * Nothing here decides what anyone may see: the hub filters every answer by
 * the caller's scope. The org filter below is a view, the org of a task is
 * whatever the hub answered.
 */

import { errorText } from './error_copy';
import type { ProposalLike, ProposalSource } from './ai_proposal';
import { derived, get, writable, type Readable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';
import { readPref, writePref } from './prefs';
import type { DecisionProposal } from './proposals';
import { acceptCommandRow, formatCostMicros, sessions, type SessionEvent, type SessionRow } from './sessions';
import { bumpWorkChanged, workChanged, type WorkChangeKind, type WorkEvidence } from './work';
import { pickTask, selectedTaskId, taskFocused, type TaskSessionLink } from './selection';
import { trackerStateBadge } from './trackers';
import { knownProviderShort } from './tracker_health';
import { classify, type TriageBucket } from './attention';

// ---------------------------------------------------------------------------
// Wire types

/** `WorkTreeFilters`: one object for tree pages, saved views, desktop and
 *  phone. Absent = no filter on that field. */
export interface WorkTreeFilters {
  /** An org id, or `none` (unassigned). */
  org?: number | 'none';
  /** A tracker id, `local` (local items) or `ref` (bare keys). */
  tracker?: number | 'local' | 'ref';
  status?: WorkStatusFilter;
  /** Assigned to me in its tracker. */
  mine?: boolean;
  has?: WorkHasFilter;
  /** Only tasks with something to review. */
  review?: boolean;
  /** Case-insensitive substring of key or title. */
  query?: string;
  /** One group only (a section being expanded). */
  group?: string;
  /** Include archived tasks (done, or every session link archived, with no
   *  active session). `false` hides them, and the page says how many
   *  (`archived_hidden`); absent shows them (a client from before the
   *  archive). `workTree` always sends it. An older hub ignores it. */
  archived?: boolean;
  /** One person it is assigned to in its tracker, by name (redesign 6.2). */
  assignee?: string;
  /** One tracker column, the tracker's own status name ("QA Review"). */
  status_name?: string;
  /** What a section under each org is; absent is `group`. */
  group_by?: WorkGroupBy;
  /** Any of these orgs (ids, or `none`): the panel's organisation chips,
   *  several at once. An older hub ignores it. */
  orgs?: (number | 'none')[];
  /** Any of these stages (`WorkTask.stage`): the panel's status chips,
   *  several at once. An older hub ignores it. */
  stages?: WorkStage[];
}

/** `WorkTask.stage`, in board order (the Work board's status chips). */
export const WORK_STAGES = ['backlog', 'in_progress', 'in_review', 'blocked', 'done'] as const;
export type WorkStage = (typeof WORK_STAGES)[number];
// A blocked stage reads Needs you, one of the six status words; the
// task line says what it is blocked on (step 6.3).
export const WORK_STAGE_LABELS: Record<WorkStage, string> = {
  backlog: 'Backlog',
  in_progress: 'In progress',
  in_review: 'In review',
  blocked: 'Needs you',
  done: 'Done',
};

/** `filters.group_by` (redesign step 6.2): the task's own group (a person,
 *  rule, tracker container, repo or key), one section per org, or its
 *  assignee, mission, account or repo. An older hub ignores it. */
export const WORK_GROUP_BY = ['group', 'org', 'person', 'mission', 'account', 'repo'] as const;
export type WorkGroupBy = (typeof WORK_GROUP_BY)[number];

export const STATUS_FILTERS = ['any', 'open', 'todo', 'in_progress', 'done'] as const;
export type WorkStatusFilter = (typeof STATUS_FILTERS)[number];
export const STATUS_FILTER_LABELS: Record<WorkStatusFilter, string> = {
  any: 'Any',
  open: 'Open',
  todo: 'To do',
  in_progress: 'In progress',
  done: 'Done',
};

export const HAS_FILTERS = ['any', 'active', 'past_only', 'none', 'suggested'] as const;
export type WorkHasFilter = (typeof HAS_FILTERS)[number];
export const HAS_FILTER_LABELS: Record<WorkHasFilter, string> = {
  any: 'Any',
  active: 'Active session',
  past_only: 'Past only',
  none: 'No session',
  suggested: 'Suggested',
};

/** `manual` | `rule` | `tracker` | `repo` | `key` | `none`. */
export type GroupSource = 'manual' | 'rule' | 'tracker' | 'repo' | 'key' | 'none';

export interface GroupRef {
  /** `label:<label>` (a manual or rule placement: `source` says which),
   *  `tracker:<id>:<container>`, `repo:<owner/repo>`, `key:<PREFIX>`,
   *  `none`. */
  id: string;
  label: string;
  source: GroupSource | string;
  rule_id?: number | null;
  tracker_value?: string | null;
}

/** `active` | `ended` | `suggested` | `rejected`. */
export type WorkLinkState = 'active' | 'ended' | 'suggested' | 'rejected';

/** One session under a task. */
export interface WorkTaskLink {
  link_id: number;
  link_version: number;
  state: WorkLinkState | string;
  primary?: boolean;
  /** The live session; absent when ended. */
  session_id?: number | null;
  name?: string | null;
  host?: string | null;
  source?: string | null;
  strength?: string | null;
  rule?: string | null;
  /** One line, from the evidence. */
  why?: string | null;
  /** `task` / `session_tasks` only, never in `tree`. */
  evidence?: WorkEvidence[];
  created_at?: number | null;
  decided_at?: number | null;
  ended_at?: number | null;
  end_reason?: string | null;
  claude_status?: string | null;
  needs_you?: boolean;
  archived?: boolean;
  resumable?: boolean;
  branch?: string | null;
  pr_url?: string | null;
  cross_org?: boolean;
  /** Other active tasks of this session the caller sees. */
  other_tasks?: number;
}

/** `tracker` | `local` | `ref`. */
export type WorkTaskKind = 'tracker' | 'local' | 'ref';
/** `tracker` | `item` | `sessions` | `none`. */
export type OrgSource = 'tracker' | 'item' | 'sessions' | 'none';

export interface WorkTask {
  /** `item:<id>` or `ref:<KEY>`. */
  task_id: string;
  item_id?: number | null;
  key?: string | null;
  title?: string | null;
  url?: string | null;
  kind: WorkTaskKind | string;
  tracker_id?: number | null;
  tracker_name?: string | null;
  provider?: string | null;
  /** The tracker's sync state: an outage is not "no sessions". */
  tracker_state?: string | null;
  status_category?: string | null;
  status_name?: string | null;
  resolution?: string | null;
  unavailable?: boolean;
  unavailable_reason?: string | null;
  assignees?: string[];
  /** The date the work is due, `YYYY-MM-DD` (absent from an older hub). */
  due_at?: string | null;
  mine?: boolean;
  org_id?: number | null;
  org_source?: OrgSource | string;
  /** The org is a boundary (tracker / item), not inferred. */
  org_fenced?: boolean;
  /** An unfenced task whose sessions span orgs. */
  org_mixed?: boolean;
  group: GroupRef;
  counts?: { active?: number; ended?: number; suggested?: number };
  needs_you?: boolean;
  review?: boolean;
  /** Done, or every session link archived, with nothing running: hidden
   *  from the tree unless `filters.archived` (or `status: done`). */
  archived?: boolean;
  /** Where the item came from: `manual` (a person wrote it in Fleet),
   *  `agent` (a delegated job's mirror), `proposed` (an agent's accepted
   *  proposal) or `detected` (a tracker ticket, a bare key). Absent from an
   *  older hub. */
  origin?: string | null;
  /** The project a native task runs in. */
  project_id?: number | null;
  /** That project as `owner/repo` (or `repo` for a local owner). */
  project_label?: string | null;
  /** A native subtask's parent (`item:<id>`); nested under it when listed. */
  parent_task_id?: string | null;
  /** A job mirror's state (the delegated job's `state`). */
  job_state?: string | null;
  /** The title is borrowed from the first session's name (the item has none). */
  title_derived?: boolean;
  /** Agent proposals under this task waiting for a person's decision. */
  open_proposals?: number;
  /** It waits for work that is not done (a dependency edge), and is not done
   *  itself. Absent when false. */
  blocked?: boolean;
  /** What it waits for (`item:<id>`), only the items this reader may see. */
  blocked_by?: string[];
  /** Spend of its sessions in micro-USD, each session once. Absent when 0. */
  cost_micros?: number;
  /** Where it stands (`WORK_STAGES`); absent from an older hub. */
  stage?: WorkStage | string;
  /** What a rule, Jev or an LLM proposes about the task (redesign 2.8).
   *  Absent when nothing proposes anything. */
  proposals?: DecisionProposal[];
  last_activity_at?: number | null;
  repos?: string[];
  /** 0 = no placement. */
  placement_version?: number;
  /** Active (primary first), suggested, ended newest first. */
  sessions?: WorkTaskLink[];
  sessions_more?: number;
}

/** A section header: every group of the whole filtered result. */
export interface WorkTreeGroup {
  org_id: number | null;
  org_name?: string | null;
  group: GroupRef;
  count: number;
  /** Spend of the tasks it counts, in micro-USD. Absent when 0. */
  cost_micros?: number;
}

export interface WorkTreeOrg {
  id: number;
  name: string;
  color?: string | null;
}

export interface WorkTreeTracker {
  id: number;
  name: string;
  provider: string;
  state: string;
  org_id?: number | null;
}

export interface WorkTreePage {
  tasks: WorkTask[];
  groups: WorkTreeGroup[];
  orgs: WorkTreeOrg[];
  trackers: WorkTreeTracker[];
  total: number;
  /** Tasks that passed every other filter but were hidden as archived
   *  (absent from an older hub, which hides none). */
  archived_hidden?: number;
  /** Tasks the filters hide that would show with none set (the archived
   *  switch kept): the "Hidden by filters" row (absent from an older hub). */
  hidden_by_filters?: number;
  next_cursor?: string | null;
  generated_at?: number;
  /** The sections `WorkTreeQuery.sections` asked for, paged from the same
   *  read (absent from an older hub: read each by itself). */
  sections?: WorkTreeSection[];
  /** The review inbox's total, when asked (absent from an older hub). */
  review_total?: number;
}

/** A section to page in the same read: exactly what a read of that
 *  section by itself (`sectionFilters`) answers first. */
export interface WorkTreeSectionAsk {
  org_id: number | null;
  group_id: string;
  /** 1–200, default 50. */
  limit?: number;
}

export interface WorkTreeSection {
  org_id: number | null;
  group_id: string;
  tasks: WorkTask[];
  next_cursor?: string | null;
}

export interface LastOutcome {
  at: number;
  name?: string | null;
  host?: string | null;
  branch?: string | null;
  pr_url?: string | null;
  /** Claude's reading of a transcript: text, never markup. */
  summary?: string | null;
}

export interface Placement {
  group: string;
  note?: string | null;
  version: number;
  updated_at?: number | null;
  updated_by?: string | null;
}

/** `work { task }`. */
export interface TaskDetail {
  task: WorkTask;
  /** Older ids of this task (a bare key a sync bound to an item). */
  aliases?: string[];
  /** Tracker text (plain, fenced for an agent, ≤ 600 chars). */
  description?: string | null;
  /** The description's full length the hub knows (the tracker's count, else
   *  the cached excerpt's), in characters. Absent from an older hub. */
  description_chars?: number | null;
  /** `description` shows less than `description_chars`. Absent means whole
   *  (or an older hub, which never said). */
  description_truncated?: boolean;
  last_outcome?: LastOutcome | null;
  placement?: Placement | null;
  /** Ids of the rules that match. */
  rules?: number[];
  /** A native task's notes (plain text, fenced for an agent). */
  notes?: string | null;
  /** The delegated job's result, when this task is a job mirror (plain text). */
  job_result?: string | null;
  /** Native subtasks: written by a person, accepted proposals, job mirrors. */
  subtasks?: SubtaskView[];
  /** Agent proposals waiting for a person's decision. */
  proposals?: ProposalView[];
  rejected_proposals?: ProposalView[];
  /** Jobs delegated under this task. */
  jobs?: JobView[];
  /** The steps agents took, per conversation. */
  steps?: StepGroup[];
}

/** A native subtask on a task page. */
export interface SubtaskView {
  task_id: string;
  item_id: number;
  key?: string | null;
  title: string;
  origin: string;
  status?: string | null;
  project_id?: number | null;
  live_sessions: number;
  job_state?: string | null;
}

/** An agent's proposed subtask. `why` and `notes` are agent text: render as text. */
export interface ProposalView {
  item_id: number;
  key?: string | null;
  title: string;
  why?: string | null;
  notes?: string | null;
  proposed_by?: string | null;
  at: number;
  /** Jev's "may duplicate" (redesign 6.9, K4): an open task this proposal
   *  may repeat. Only a live assist answer; absent from an older hub. */
  duplicate?: DuplicateHint | null;
}

/** The existing task a proposal may duplicate, as Jev proposed it. */
export interface DuplicateHint {
  item_id: number;
  /** `item:<id>`, the task page to open. */
  task_id: string;
  key?: string | null;
  title: string;
  source: ProposalSource;
  confidence_pct?: number | null;
  run_id?: number | null;
}

/** A delegated job under a task. `result` is agent text: render as text. */
export interface JobView {
  item_id: number;
  key?: string | null;
  title: string;
  state: string;
  result?: string | null;
  /** The worker session's name, while it exists. */
  worker?: string | null;
  at: number;
}

/** One conversation's steps, labelled by the session that ran it. */
export interface StepGroup {
  label: string;
  claude_session_id: string;
  steps?: StepLine[];
}

/** One step an agent took (its own task tools). `text` is agent text. */
export interface StepLine {
  text: string;
  /** `pending` | `in_progress` | `completed`. */
  state: string;
  at: number;
}

/** The task a `session_tasks` link is to. */
export interface SessionTaskRef {
  task_id: string;
  key?: string | null;
  title?: string | null;
  kind?: string;
  status_category?: string | null;
  status_name?: string | null;
  url?: string | null;
  unavailable?: boolean;
  org_id?: number | null;
  tracker_name?: string | null;
}

export interface SessionTaskLink extends WorkTaskLink {
  task: SessionTaskRef;
}

/** `work { session_tasks }`: every link of the session's participant. */
export interface SessionTasks {
  session_id: number;
  org_id?: number | null;
  primary_link_id?: number | null;
  links: SessionTaskLink[];
}

/** `suggestion` | `cross_org` | `unavailable` | `no_primary`. */
export type ReviewKind = 'suggestion' | 'cross_org' | 'unavailable' | 'no_primary';

/**
 * A suggestion at or above this confidence is "high confidence": Review's
 * "Confirm all high-confidence" ticks exactly these. Mirrors
 * `work::confidence::HIGH_CONFIDENCE` in fleet-core.
 */
export const HIGH_CONFIDENCE_PCT = 85;

/** A review item's suggestion clears the high-confidence bar. */
export function isHighConfidence(it: Pick<ReviewItem, 'kind' | 'confidence'>): boolean {
  return it.kind === 'suggestion' && (it.confidence ?? 0) >= HIGH_CONFIDENCE_PCT;
}

export interface ReviewItem {
  review_id: string;
  kind: ReviewKind | string;
  session_id: number;
  session_name?: string | null;
  host?: string | null;
  link_id: number;
  link_version?: number;
  task: { task_id: string; key?: string | null; title?: string | null; org_id?: number | null };
  why?: string[];
  strength?: string | null;
  rule?: string | null;
  /** 0–100, from detection (`work::confidence`); absent from an older hub. */
  confidence?: number | null;
  preselected?: boolean;
  alternatives?: { link_id?: number | null; task_id: string; key?: string | null; title?: string | null }[];
  created_at?: number;
  /** Who proposed it when no rule read it off a signal: the decision
   *  model's suggestion (J1, rule R12, redesign 6.8). Absent otherwise, and
   *  from an older hub. */
  proposed_by?: ReviewProposer | null;
  /** The tracker ticket this suggestion's LOCAL task may duplicate (J7
   *  `tracker_duplicate`, redesign 6.8): a live Jev proposal only. Absent
   *  otherwise, and from an older hub. */
  duplicate_of?: ReviewDuplicate | null;
}

/** `ReviewItem.duplicate_of` (`work::view::ReviewDuplicate`). */
export interface ReviewDuplicate {
  task_id: string;
  item_id: number;
  key?: string | null;
  title: string;
  source: 'jev' | 'rule' | 'llm';
  confidence_pct?: number | null;
}

/** The proposal `ProposedBy` shows beside a Review item's "May duplicate"
 *  (J7), or null when nothing is flagged. */
export function reviewDuplicateProposal(it: Pick<ReviewItem, 'duplicate_of'>): ProposalLike | null {
  const d = it.duplicate_of;
  if (!d) return null;
  return {
    value: d.key ?? d.task_id,
    source: d.source,
    reason: 'same work as a tracker ticket',
    confidence_pct: d.confidence_pct ?? null,
  };
}

/** `ReviewItem.proposed_by` (`work::view::ReviewProposer`). */
export interface ReviewProposer {
  source: 'jev' | 'rule' | 'llm';
  /** Why, in fleet's words ("from the first prompt"). */
  reason: string;
  confidence_pct?: number | null;
}

/** The proposal `ProposedBy` shows for a Review item, or null for a rule's
 *  own reading. */
export function reviewProposal(it: Pick<ReviewItem, 'proposed_by' | 'task'>): ProposalLike | null {
  const p = it.proposed_by;
  if (!p) return null;
  return {
    value: it.task.key ?? it.task.task_id,
    source: p.source,
    reason: p.reason,
    confidence_pct: p.confidence_pct ?? null,
  };
}

/** The proposal `ProposedBy` shows beside a proposal's "May duplicate", or
 *  null when Jev flagged nothing. */
export function duplicateProposal(p: Pick<ProposalView, 'duplicate'>): ProposalLike | null {
  const d = p.duplicate;
  if (!d) return null;
  return {
    value: d.key ?? d.task_id,
    source: d.source,
    reason: 'alike title',
    confidence_pct: d.confidence_pct ?? null,
  };
}

export interface ReviewPage {
  items: ReviewItem[];
  total: number;
  next_cursor?: string | null;
}

export interface WorkRuleConditions {
  tracker_id?: number | null;
  container?: string | null;
  key_prefix?: string | null;
  title_contains?: string | null;
  repo?: string | null;
}

export interface WorkRule {
  id: number;
  name: string;
  enabled: boolean;
  version: number;
  conditions: WorkRuleConditions;
  group: string;
  /** "Its sessions start here" (G7.1): the host a start of a matching task
   *  lands on when no start rule names one. */
  host_alias?: string | null;
  /** The account (credential profile) those sessions bill. */
  profile?: string | null;
  created_at?: number;
  updated_at?: number;
}

/** What `rule_save` and `rule_preview` take. */
export interface WorkRuleDraft {
  id?: number;
  name: string;
  enabled: boolean;
  conditions: WorkRuleConditions;
  group: string;
  host_alias?: string | null;
  profile?: string | null;
  expected_version?: number;
}

export interface RulePreview {
  affected: { task_id: string; key?: string | null; title?: string | null; from: GroupRef; to: GroupRef }[];
  total: number;
  /** Tasks placed by a person that the rule leaves where they are. */
  kept_manual: number;
  /** Open tasks the draft's conditions match now, moved or not (G2.2);
   *  absent from an older hub. */
  matched?: number;
  /** The first few of them, by key (else title). */
  matched_sample?: string[];
}

export interface WorkView {
  id: number;
  name: string;
  filters: WorkTreeFilters;
  version: number;
  updated_at?: number;
}

export interface OrgImpactLink {
  link_id: number;
  session_id?: number | null;
  name?: string | null;
  host?: string | null;
  state: string;
  session_org?: number | null;
  becomes_cross_org?: boolean;
}

export interface OrgImpact {
  task_id: string;
  from_org?: number | null;
  to_org?: number | null;
  allowed: boolean;
  /** Why not (`tracker_controlled` …). */
  reason?: string | null;
  links: OrgImpactLink[];
  hosts_losing: string[];
  hosts_gaining: string[];
  bound_clients_losing: number;
  bound_clients_gaining: number;
  /** People whose org-bound devices lose / gain the task (G2.2); absent
   *  when none, or from an older hub. */
  people_losing?: string[];
  people_gaining?: string[];
  journal_entries: number;
  summaries: number;
  impact_token?: string | null;
}

export type Decision = 'confirm' | 'reject' | 'reconsider' | 'ack';

export interface BatchDecision {
  session_id: number;
  link_id: number;
  decision: Decision;
  expected_version?: number;
  primary?: boolean;
}

export interface BatchItemResult {
  link_id: number;
  ok: boolean;
  code?: string | null;
  message?: string | null;
  version?: number | null;
}

export interface BatchResult {
  results: BatchItemResult[];
}

// ---------------------------------------------------------------------------
// Commands. Reads route to the hub's `work`, writes to its `work_link`.

/** Drop `undefined` fields so the arguments are exactly what was meant. */
function clean(o: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(o)) if (v !== undefined) out[k] = v;
  return out;
}

export interface WorkTreeQuery {
  filters?: WorkTreeFilters;
  cursor?: string | null;
  /** 1–200, default 50. */
  limit?: number;
  /** 0–50, default 8. */
  per_task?: number;
  /** Open sections to page from the same read (at most 100). */
  sections?: WorkTreeSectionAsk[];
  /** Add `review_total` from the same read. */
  with_review_total?: boolean;
}

/** `archived` always goes on the wire: the hub hides archived tasks only
 *  when asked (`archived: false`), so a phone from before the archive, which
 *  never sends it, keeps seeing them. */
export function workTree(q: WorkTreeQuery = {}): Promise<Result<WorkTreePage>> {
  const n = normalizeFilters(q.filters);
  const filters: WorkTreeFilters = { ...n, archived: n.archived === true };
  return invokeCmd<WorkTreePage>('work_tree', {
    args: clean({
      filters,
      cursor: q.cursor ?? undefined,
      limit: q.limit,
      per_task: q.per_task,
      sections: q.sections && q.sections.length > 0 ? q.sections : undefined,
      with_review_total: q.with_review_total || undefined,
    }),
  });
}

export function workTask(taskId: string): Promise<Result<TaskDetail>> {
  return invokeCmd<TaskDetail>('work_task', { args: { task_id: taskId } });
}

export function workSessionTasks(sessionId: number): Promise<Result<SessionTasks>> {
  return invokeCmd<SessionTasks>('work_session_tasks', { args: { session_id: sessionId } });
}

export function workReview(q: { cursor?: string | null; limit?: number } = {}): Promise<Result<ReviewPage>> {
  return invokeCmd<ReviewPage>('work_review', { args: clean({ cursor: q.cursor ?? undefined, limit: q.limit }) });
}

export function workRules(): Promise<Result<WorkRule[]>> {
  return invokeCmd<WorkRule[]>('work_rules', { args: {} });
}

export function workRulePreview(rule: WorkRuleDraft): Promise<Result<RulePreview>> {
  return invokeCmd<RulePreview>('work_rule_preview', { args: { rule: ruleWire(rule) } });
}

export function workViews(): Promise<Result<WorkView[]>> {
  return invokeCmd<WorkView[]>('work_views', { args: {} });
}

/** What moving a local task to `orgId` (0 = no org) would change. */
export function workOrgImpact(taskId: string, orgId: number): Promise<Result<OrgImpact>> {
  return invokeCmd<OrgImpact>('work_org_impact', { args: { task_id: taskId, org_id: orgId } });
}

/** Every write below goes through here or `write`: on success the Work
 *  view, the task detail and the session's Tasks re-read at once (a
 *  secondary link's change moves no field they could diff). */
async function write<T>(cmd: string, args: Record<string, unknown>): Promise<Result<T>> {
  const r = await invokeCmd<T>(cmd, { args });
  if (r.ok) bumpWorkChanged();
  return r;
}

async function rowCmd(cmd: string, args: Record<string, unknown>): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>(cmd, { args: clean(args) });
  if (r.ok) {
    acceptCommandRow(r.value);
    bumpWorkChanged();
  }
  return r;
}

/** Move the session's primary to `linkId`: a compare-and-set on the current
 *  primary (`expectedPrimary`, 0 = none). Another link is never ended. */
export function setPrimaryWork(
  sessionId: number,
  linkId: number,
  expectedPrimary: number | null | undefined,
): Promise<Result<SessionRow>> {
  return rowCmd('set_primary_work', {
    session_id: sessionId,
    link_id: linkId,
    expected_primary: expectedPrimary ?? 0,
  });
}

/** A person's confirm / reject goes back to a suggestion (the Undo). */
export function reconsiderWorkLink(
  sessionId: number,
  linkId: number,
  expectedVersion?: number,
): Promise<Result<SessionRow>> {
  return rowCmd('reconsider_work_link', {
    session_id: sessionId,
    link_id: linkId,
    expected_version: expectedVersion,
  });
}

/** Keep a conflict (cross-org, unavailable) on purpose. */
export function ackWorkLink(sessionId: number, linkId: number, expectedVersion?: number): Promise<Result<SessionRow>> {
  return rowCmd('ack_work_link', { session_id: sessionId, link_id: linkId, expected_version: expectedVersion });
}

/** Up to 100 decisions, each checked on its own. */
export function decideWorkBatch(decisions: readonly BatchDecision[]): Promise<Result<BatchResult>> {
  return write<BatchResult>('decide_work_batch', { decisions: decisions.map((d) => clean({ ...d })) });
}

/** Place a task in a group (`''` clears the placement). `expectedVersion`
 *  is the task's `placement_version` (0: "I expect none"). */
export function placeWork(
  taskId: string,
  group: string,
  expectedVersion: number,
  note?: string | null,
): Promise<Result<WorkTask>> {
  const n = note?.trim();
  return write<WorkTask>(
    'place_work',
    clean({ task_id: taskId, group: group.trim(), note: n ? n : undefined, expected_version: expectedVersion }),
  );
}

/** Move a local task to `orgId` (0 = none), with the token of a fresh
 *  impact preview. */
export function assignWorkOrg(taskId: string, orgId: number, impactToken: string): Promise<Result<WorkTask>> {
  return write<WorkTask>('assign_work_org', { task_id: taskId, org_id: orgId, impact_token: impactToken });
}

/** A draft as the wire takes it: trimmed, empty conditions dropped. */
export function ruleWire(d: WorkRuleDraft): WorkRuleDraft {
  const c = d.conditions ?? {};
  const s = (v: string | null | undefined) => {
    const t = v?.trim();
    return t ? t : null;
  };
  return clean({
    id: d.id,
    name: d.name.trim(),
    enabled: d.enabled,
    conditions: {
      tracker_id: c.tracker_id ?? null,
      container: s(c.container),
      key_prefix: s(c.key_prefix),
      title_contains: s(c.title_contains),
      repo: s(c.repo),
    },
    group: d.group.trim(),
    host_alias: s(d.host_alias) ?? undefined,
    profile: s(d.profile) ?? undefined,
    expected_version: d.expected_version,
  }) as unknown as WorkRuleDraft;
}

export function saveWorkRule(rule: WorkRuleDraft): Promise<Result<WorkRule>> {
  return write<WorkRule>('save_work_rule', { rule: ruleWire(rule) });
}

export function deleteWorkRule(ruleId: number, expectedVersion?: number): Promise<Result<{ deleted: boolean }>> {
  return write<{ deleted: boolean }>('delete_work_rule', clean({ rule_id: ruleId, expected_version: expectedVersion }));
}

export function saveWorkView(view: {
  id?: number;
  name: string;
  filters: WorkTreeFilters;
  expected_version?: number;
}): Promise<Result<WorkView>> {
  const { group: _group, ...filters } = normalizeFilters(view.filters);
  return write<WorkView>('save_work_view', {
    view: clean({ id: view.id, name: view.name.trim(), filters, expected_version: view.expected_version }),
  });
}

/** Delete a saved view if it is still at `expectedVersion` (the version the
 *  person saw; absent: any). */
export function deleteWorkView(viewId: number, expectedVersion?: number): Promise<Result<{ deleted: boolean }>> {
  return write<{ deleted: boolean }>('delete_work_view', clean({ view_id: viewId, expected_version: expectedVersion }));
}

// ---------------------------------------------------------------------------
// Errors

/** What an `E_CONFLICT` carries: the current value. A link's (`link_id`,
 *  `version`, `state`, `primary`, `ended`), a session's primary
 *  (`session_id`, `primary_link_id`), a placement's (`task_id`, `version`,
 *  `group`), a rule's or a view's (`rule_id` / `view_id`, `version`). */
export interface Conflict {
  link_id?: number | null;
  version?: number;
  state?: string;
  primary?: number | boolean | null;
  ended?: boolean;
  primary_link_id?: number | null;
  group?: string | null;
  [k: string]: unknown;
}

/** The conflict of an error, or null when it is not one. */
export function conflictOf(e: IpcError | null | undefined): Conflict | null {
  if (!e || e.code !== 'E_CONFLICT') return null;
  const d = e.details;
  return d && typeof d === 'object' ? (d as Conflict) : {};
}

/** The sentence a conflict shows before the reload. */
export function conflictSentence(what: string): string {
  return `${what} changed elsewhere (another window or device) — reloaded, so check it and try again.`;
}

/** The current value a conflict names, as one line ("Now: rejected ·
 *  version 4"), or null when it names none. `linkName` names a link id (a
 *  session's primary) when the caller knows it. */
export function conflictCurrent(
  c: Conflict | null | undefined,
  linkName?: (linkId: number) => string | null | undefined,
): string | null {
  if (!c) return null;
  const parts: string[] = [];
  if (typeof c.state === 'string' && c.state) {
    parts.push(c.ended ? 'ended' : c.primary === true ? `${c.state} · primary` : c.state);
  }
  if ('link_id' in c && c.link_id == null) parts.push('no link');
  if ('primary_link_id' in c) {
    const id = c.primary_link_id;
    parts.push(typeof id === 'number' ? `primary is ${linkName?.(id) ?? `link ${id}`}` : 'no primary');
  }
  if ('group' in c) parts.push(typeof c.group === 'string' && c.group ? `placed in “${c.group}”` : 'not placed');
  if (typeof c.version === 'number' && c.version > 0) parts.push(`version ${c.version}`);
  return parts.length > 0 ? `Now: ${parts.join(' · ')}` : null;
}

/** A conflict as a notice shows it: the sentence, the current value, and
 *  (`WorkConflictNotice`) a Reload action. */
export interface ConflictNotice {
  conflict: true;
  text: string;
  current: string | null;
}

/** The notice for an error when it is a conflict (null otherwise). */
export function conflictNotice(
  e: IpcError | null | undefined,
  what: string,
  linkName?: (linkId: number) => string | null | undefined,
): ConflictNotice | null {
  const c = conflictOf(e);
  if (!c) return null;
  return { conflict: true, text: conflictSentence(what), current: conflictCurrent(c, linkName) };
}

/** "Needs a newer hub": the hub (or this build's backend) has no Work view. */
export const NEWER_HUB = 'Needs a newer hub: update fleet-hub to use the Work view.';

/** Whether an error means the backend does not know the action at all —
 *  and only that: any other refusal (a bad cursor, a hub that is down, a
 *  protocol hiccup) is the backend's own sentence, never "update the hub". */
export function isOlderHub(e: IpcError | null | undefined): boolean {
  if (!e) return false;
  // The hub's `work` / `work_link` action enum: `unknown work action "tree";
  // one of …`.
  if (e.code === 'E_INVALID') return /\bunknown (?:work(?:_link)? )?action\b/i.test(e.message);
  // A hub with no such tool at all (rmcp's dispatch refusal).
  if (e.code === 'E_HUB_PROTOCOL') return /\btool\b.*\bnot found\b|\bunknown tool\b|\bno such tool\b/i.test(e.message);
  // A desktop build older than the command: Tauri's own refusal.
  return e.code === 'E_UNKNOWN' && /command\s+\S+\s+not found/i.test(e.message);
}

/** The error line a read shows. */
export function readErrorText(e: IpcError): string {
  return isOlderHub(e) ? NEWER_HUB : errorText(e);
}

// ---------------------------------------------------------------------------
// Filters

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** A filters object with defaults and junk dropped: `status: any`, `has:
 *  any`, empty `query`, `false` flags are all "no filter". */
export function normalizeFilters(v: unknown): WorkTreeFilters {
  if (!isObj(v)) return {};
  const out: WorkTreeFilters = {};
  const org = v.org;
  if (org === 'none') out.org = 'none';
  else if (typeof org === 'number' && Number.isInteger(org) && org > 0) out.org = org;
  const tr = v.tracker;
  if (tr === 'local' || tr === 'ref') out.tracker = tr;
  else if (typeof tr === 'number' && Number.isInteger(tr) && tr > 0) out.tracker = tr;
  if (typeof v.status === 'string' && (STATUS_FILTERS as readonly string[]).includes(v.status) && v.status !== 'any') {
    out.status = v.status as WorkStatusFilter;
  }
  if (v.mine === true) out.mine = true;
  if (typeof v.has === 'string' && (HAS_FILTERS as readonly string[]).includes(v.has) && v.has !== 'any') {
    out.has = v.has as WorkHasFilter;
  }
  if (v.review === true) out.review = true;
  if (typeof v.query === 'string' && v.query.trim() !== '') out.query = v.query.trim();
  if (typeof v.group === 'string' && v.group !== '') out.group = v.group;
  if (v.archived === true) out.archived = true;
  if (typeof v.assignee === 'string' && v.assignee.trim() !== '') out.assignee = v.assignee.trim();
  if (typeof v.status_name === 'string' && v.status_name.trim() !== '') out.status_name = v.status_name.trim();
  if (typeof v.group_by === 'string' && (WORK_GROUP_BY as readonly string[]).includes(v.group_by) && v.group_by !== 'group') {
    out.group_by = v.group_by as WorkGroupBy;
  }
  // Sets, kept in a stable order so equal choices compare equal.
  if (Array.isArray(v.orgs)) {
    const ids = v.orgs.filter((o): o is number => typeof o === 'number' && Number.isInteger(o) && o > 0);
    const orgs: (number | 'none')[] = [...new Set(ids)].sort((a, b) => a - b);
    if (v.orgs.includes('none')) orgs.push('none');
    if (orgs.length > 0) out.orgs = orgs;
  }
  if (Array.isArray(v.stages)) {
    const stages = WORK_STAGES.filter((st) => (v.stages as unknown[]).includes(st));
    if (stages.length > 0) out.stages = stages;
  }
  return out;
}

const FILTER_ORDER: (keyof WorkTreeFilters)[] = [
  'org',
  'orgs',
  'tracker',
  'status',
  'stages',
  'status_name',
  'mine',
  'assignee',
  'has',
  'review',
  'query',
  'group',
  'archived',
  'group_by',
];

/** A stable string for a filters object (equal filters, equal keys). */
export function filtersKey(f: WorkTreeFilters): string {
  const n = normalizeFilters(f);
  return JSON.stringify(FILTER_ORDER.filter((k) => n[k] !== undefined).map((k) => [k, n[k]]));
}

export function sameFilters(a: WorkTreeFilters, b: WorkTreeFilters): boolean {
  return filtersKey(a) === filtersKey(b);
}

/** How many filters are on (the chip's count); `group` is navigation,
 *  `group_by` arranges rather than narrows, and showing archived tasks
 *  widens the view. */
export function activeFilterCount(f: WorkTreeFilters): number {
  const n = normalizeFilters(f);
  return FILTER_ORDER.filter((k) => k !== 'group' && k !== 'archived' && k !== 'group_by' && n[k] !== undefined).length;
}

// ---------------------------------------------------------------------------
// Sections: org → group → tasks, merged from pages

/** A section's key: its org and its group (a group id can recur per org). */
export function sectionKey(orgId: number | null | undefined, groupId: string): string {
  return `${orgId ?? 'none'}|${groupId}`;
}

/** The inverse of `sectionKey` (the org part never has a `|`); null for a
 *  key that is not a section's (an org's, `org:<id>`). */
export function parseSectionKey(k: string): { orgId: number | null; groupId: string } | null {
  const i = k.indexOf('|');
  if (i <= 0) return null;
  const org = k.slice(0, i);
  if (org !== 'none' && !/^\d+$/.test(org)) return null;
  return { orgId: org === 'none' ? null : Number(org), groupId: k.slice(i + 1) };
}

export function orgSectionKey(orgId: number | null | undefined): string {
  return `org:${orgId ?? 'none'}`;
}

/** The tasks loaded for one section, and where its next page starts. */
export interface SectionState {
  tasks: WorkTask[];
  /** The section's own cursor; null: no more, or not loaded by section. */
  cursor: string | null;
  /** Loaded with `filters.group` (so `cursor` is the section's). */
  own: boolean;
}

export interface GroupSection {
  key: string;
  orgId: number | null;
  group: GroupRef;
  count: number;
  tasks: WorkTask[];
  /** More tasks exist than are loaded. */
  more: boolean;
  /** Spend of the tasks it counts, micro-USD (redesign 6.3); 0 when none. */
  cost: number;
}

export interface OrgSection {
  key: string;
  orgId: number | null;
  name: string;
  color: string | null;
  count: number;
  /** Spend of its groups' tasks, micro-USD (redesign 6.3 "spend on org rows"). */
  cost: number;
  groups: GroupSection[];
}

/** Append `add` to `base`, skipping task ids already there (a page is
 *  stable, but a task that moved may show up twice across reads). */
export function mergeTasks(base: readonly WorkTask[], add: readonly WorkTask[]): WorkTask[] {
  const seen = new Set(base.map((t) => t.task_id));
  const out = [...base];
  for (const t of add) {
    if (seen.has(t.task_id)) {
      const i = out.findIndex((x) => x.task_id === t.task_id);
      out[i] = t;
    } else {
      seen.add(t.task_id);
      out.push(t);
    }
  }
  return out;
}

/** Spread a page's tasks into their sections' states (the first page of the
 *  view fills the first sections). A section loaded by itself keeps its own
 *  tasks and cursor. */
export function distributeTasks(
  page: Pick<WorkTreePage, 'tasks'>,
  prev: ReadonlyMap<string, SectionState> = new Map(),
): Map<string, SectionState> {
  const out = new Map<string, SectionState>();
  for (const [k, s] of prev) if (s.own) out.set(k, s);
  for (const t of page.tasks ?? []) {
    const k = sectionKey(t.org_id, t.group?.id ?? 'none');
    const cur = out.get(k);
    if (cur?.own) {
      out.set(k, { ...cur, tasks: mergeTasks(cur.tasks, [t]) });
    } else {
      out.set(k, { tasks: mergeTasks(cur?.tasks ?? [], [t]), cursor: null, own: false });
    }
  }
  return out;
}

/** The sections the view draws: every header from `groups` (in the hub's
 *  order: named orgs by name, unassigned last), with the tasks loaded so
 *  far. */
export function buildSections(
  groups: readonly WorkTreeGroup[],
  orgs: readonly WorkTreeOrg[],
  states: ReadonlyMap<string, SectionState>,
): OrgSection[] {
  const orgById = new Map(orgs.map((o) => [o.id, o]));
  const out: OrgSection[] = [];
  const byOrg = new Map<string, OrgSection>();
  for (const g of groups) {
    const ok = orgSectionKey(g.org_id);
    let o = byOrg.get(ok);
    if (!o) {
      const meta = g.org_id != null ? orgById.get(g.org_id) : undefined;
      o = {
        key: ok,
        orgId: g.org_id ?? null,
        name: g.org_id == null ? 'Unassigned' : (meta?.name ?? g.org_name ?? `Organisation ${g.org_id}`),
        color: meta?.color ?? null,
        count: 0,
        cost: 0,
        groups: [],
      };
      byOrg.set(ok, o);
      out.push(o);
    }
    const k = sectionKey(g.org_id, g.group.id);
    const st = states.get(k);
    const tasks = st?.tasks ?? [];
    o.count += g.count;
    o.cost += g.cost_micros ?? 0;
    o.groups.push({
      key: k,
      orgId: g.org_id ?? null,
      group: g.group,
      count: g.count,
      cost: g.cost_micros ?? 0,
      tasks,
      more: st?.own ? st.cursor != null : tasks.length < g.count,
    });
  }
  return out;
}

/** The filters that load exactly one section. */
export function sectionFilters(base: WorkTreeFilters, orgId: number | null, groupId: string): WorkTreeFilters {
  return { ...normalizeFilters(base), org: orgId ?? 'none', group: groupId };
}

// ---------------------------------------------------------------------------
// Occurrences, states and provenance

/** How an occurrence draws: `primary` ★, `secondary`, `suggested` (dashed,
 *  "?"), `past` (dimmed, "ended"), `rejected`. An unknown state is past:
 *  never shown as active. */
export type OccurrenceKind = 'primary' | 'secondary' | 'suggested' | 'past' | 'rejected';

export function occurrenceKind(l: Pick<WorkTaskLink, 'state' | 'primary'>): OccurrenceKind {
  switch (l.state) {
    case 'active':
      return l.primary ? 'primary' : 'secondary';
    case 'suggested':
      return 'suggested';
    case 'rejected':
      return 'rejected';
    default:
      return 'past';
  }
}

/** Whether `l` is an occurrence of the selected session (every one is
 *  highlighted; an ended link names no live session). */
export function isOccurrenceOf(l: Pick<WorkTaskLink, 'session_id' | 'state'>, sessionId: number | null | undefined): boolean {
  return sessionId != null && l.session_id === sessionId && l.state !== 'ended' && l.state !== 'rejected';
}

/** A task's short status: the tracker's own name, else its category. */
export function taskStatus(t: Pick<WorkTask, 'status_name' | 'status_category'>): string {
  if (t.status_name) return t.status_name;
  switch (t.status_category) {
    case 'todo':
      return 'to do';
    case 'in_progress':
      return 'in progress';
    case 'done':
      return 'done';
    default:
      return '';
  }
}

/** A task's spend ("$3.20"), or null when its sessions cost nothing yet
 *  (redesign 6.3: summed from its sessions, each once). */
export function taskSpend(t: Pick<WorkTask, 'cost_micros'>): string | null {
  return (t.cost_micros ?? 0) > 0 ? formatCostMicros(t.cost_micros) : null;
}

/** How a task a blocked one waits for reads: its key, else its title, else
 *  "task 212" from its id (one this view has not loaded). */
export function dependencyName(id: string, known?: Pick<WorkTask, 'key' | 'title'> | null): string {
  if (known?.key) return known.key;
  const title = (known?.title ?? '').trim();
  if (title) return title;
  const m = /^item:(\d+)$/.exec(id);
  return m ? `task ${m[1]}` : id;
}

/** The reason line of a blocked task: "Blocked on TASK-212", two named,
 *  then "+N more". The plan's status-word decision shows a blocked task
 *  as Needs you with this line; null for a task that waits for nothing. */
export function blockedOnLine(
  t: Pick<WorkTask, 'blocked' | 'blocked_by'>,
  lookup: (id: string) => Pick<WorkTask, 'key' | 'title'> | null | undefined = () => null,
): string | null {
  if (!t.blocked) return null;
  const deps = t.blocked_by ?? [];
  if (deps.length === 0) return 'Blocked on another task';
  const names = deps.slice(0, 2).map((id) => dependencyName(id, lookup(id)));
  const more = deps.length > 2 ? ` +${deps.length - 2} more` : '';
  return `Blocked on ${names.join(', ')}${more}`;
}

/** `KEY title`, or the title alone, or the task id. */
export function taskLabel(t: { key?: string | null; title?: string | null; task_id: string }): string {
  const title = (t.title ?? '').trim();
  if (t.key && title) return `${t.key} ${title}`;
  return t.key || title || t.task_id;
}

/** Where a task's org comes from, as a sentence. */
export function orgSourceText(t: Pick<WorkTask, 'org_source' | 'tracker_name' | 'org_mixed'>): string {
  switch (t.org_source) {
    case 'tracker':
      return `from tracker ${t.tracker_name ?? 'its tracker'}`;
    case 'item':
      return 'set by a person';
    case 'sessions':
      return t.org_mixed
        ? 'inferred from its sessions, which span several organisations — not a boundary'
        : 'inferred from its sessions — not a boundary';
    default:
      return 'no organisation';
  }
}

/** "Jira", "GitHub", … for a provider id; "the tracker" for none or one
 *  this build does not know. */
export function providerName(p: string | null | undefined): string {
  return (p && knownProviderShort(p)) || 'the tracker';
}

/** Where a task's group comes from, as a sentence. */
export function groupSourceText(
  g: GroupRef,
  t: Pick<WorkTask, 'tracker_name' | 'provider'>,
  ruleName?: string | null,
): string {
  switch (g.source) {
    case 'manual':
      return 'placed here by a person';
    case 'rule':
      return ruleName ? `placed by the rule “${ruleName}”` : 'placed by a rule';
    case 'tracker':
      return `from the tracker: ${t.tracker_name ?? providerName(t.provider)} ${g.tracker_value ?? g.label}`;
    case 'repo':
      return `from the repository of its most recent session (${g.label})`;
    case 'key':
      return `from its key prefix ${g.label}`;
    default:
      return 'nothing known — no group';
  }
}

/** What an edit of the group changes, for the placement note. */
export function placementNote(g: GroupRef, t: Pick<WorkTask, 'provider'>): string | null {
  if (g.source === 'tracker') {
    return `Placing it elsewhere is local to fleet: it never changes ${providerName(t.provider)}.`;
  }
  if (g.source === 'rule') return 'Placing it by hand overrides the rule for this task only.';
  if (g.source === 'manual') return 'Clearing the placement falls back to where fleet would put it.';
  return 'Placing it is local to fleet navigation; it is never a boundary.';
}

/** A key's prefix as the hub reads it (`view.rs` `key_prefix`): the part
 *  before the first `-`, non-empty, followed by digits only, in ASCII
 *  upper case (`abc-12` → `ABC`, `ops2.x-12` → `OPS2.X`, `ABC-12a` → null). */
export function keyPrefix(key: string | null | undefined): string | null {
  if (!key) return null;
  const i = key.indexOf('-');
  if (i <= 0) return null;
  const tail = key.slice(i + 1);
  if (!/^[0-9]+$/.test(tail)) return null;
  return key.slice(0, i).replace(/[a-z]+/g, (c) => c.toUpperCase());
}

/** "Make a rule…" prefilled from where a task's group comes from: its
 *  tracker and project, its key prefix (the key group's own label, else the
 *  hub's reading of the key) or its repository. */
export function ruleDraftFor(
  t: Pick<WorkTask, 'kind' | 'tracker_id' | 'key' | 'group'>,
  group: string,
  name = '',
): WorkRuleDraft {
  const g = t.group;
  const prefix = g?.source === 'key' ? g.label || null : keyPrefix(t.key);
  return {
    name,
    enabled: true,
    group,
    expected_version: 0,
    conditions: {
      tracker_id: t.kind === 'tracker' ? (t.tracker_id ?? null) : null,
      container: g?.source === 'tracker' ? (g.tracker_value ?? g.label) : null,
      key_prefix: g?.source !== 'tracker' ? prefix : null,
      repo: g?.source === 'repo' ? g.label : null,
      title_contains: null,
    },
  };
}

/** A tracker state that is not `ok`: "tracker down". */
export function trackerDown(t: Pick<WorkTask, 'tracker_state' | 'kind'>): boolean {
  return t.kind === 'tracker' && !!t.tracker_state && t.tracker_state !== 'ok';
}

/** What is wrong with a task's tracker, in the tracker settings' own words
 *  ("tracker: not tested yet", "tracker: token expired or wrong", …): a
 *  tracker that was never tested is not "down". */
export function trackerDownLabel(t: Pick<WorkTask, 'tracker_state'>): string {
  return `tracker: ${trackerStateBadge(t.tracker_state ?? '').label}`;
}

/** A review kind as a badge. */
export function reviewKindLabel(kind: string): string {
  switch (kind) {
    case 'suggestion':
      return 'suggestion';
    case 'cross_org':
      return 'cross-org';
    case 'unavailable':
      return 'ticket unavailable';
    case 'no_primary':
      return 'no primary';
    default:
      return kind;
  }
}

/** The undo of a review decision: a person's confirm / reject goes back to
 *  a suggestion. Nothing else has an exact inverse. */
export function undoOf(d: BatchDecision, version?: number | null): BatchDecision | null {
  if (d.decision !== 'confirm' && d.decision !== 'reject') return null;
  return {
    session_id: d.session_id,
    link_id: d.link_id,
    decision: 'reconsider',
    ...(version != null ? { expected_version: version } : {}),
  };
}

/** Group a session's links: active (primary first), suggested, past,
 *  rejected. */
export function groupSessionLinks<T extends Pick<WorkTaskLink, 'state' | 'primary' | 'ended_at' | 'link_id'>>(
  links: readonly T[],
): { active: T[]; suggested: T[]; past: T[]; rejected: T[] } {
  const active = links.filter((l) => l.state === 'active');
  active.sort((a, b) => Number(!!b.primary) - Number(!!a.primary) || a.link_id - b.link_id);
  const suggested = links.filter((l) => l.state === 'suggested');
  const rejected = links.filter((l) => l.state === 'rejected');
  const past = links
    .filter((l) => !['active', 'suggested', 'rejected'].includes(l.state))
    .sort((a, b) => (b.ended_at ?? 0) - (a.ended_at ?? 0) || b.link_id - a.link_id);
  return { active, suggested, past, rejected };
}

// ---------------------------------------------------------------------------
// Stores

/** Which tree the sidebar shows. */
/** `inbox` is the Inbox (redesign step 3.3): the Sessions list narrowed to
 *  what needs you. */
export type SidebarView = 'sessions' | 'work' | 'inbox';
const isSidebarView = (v: unknown): v is SidebarView => v === 'sessions' || v === 'work' || v === 'inbox';
export const sidebarView = writable<SidebarView>(readPref('sidebar.view', 'sessions', isSidebarView));
sidebarView.subscribe((v) => writePref('sidebar.view', v));

export function toggleSidebarView(): void {
  sidebarView.update((v) => (v === 'work' ? 'sessions' : 'work'));
}

/** The Work tab's layout (design 2026-09-29): List (by status: To do, Doing,
 *  Done) or Grouped (org → group). Defaults to List. */
export type WorkLayout = 'list' | 'grouped';
const isWorkLayout = (v: unknown): v is WorkLayout => v === 'list' || v === 'grouped';
export const workLayout = writable<WorkLayout>(readPref('work.layout', 'list', isWorkLayout));
workLayout.subscribe((v) => writePref('work.layout', v));

const isFilters = (v: unknown): v is WorkTreeFilters => isObj(v);
/** The Work view's filters (without `group`, which is per section). */
export const workViewFilters = writable<WorkTreeFilters>(
  (() => {
    const { group: _g, ...f } = normalizeFilters(readPref('work.filters', {}, isFilters));
    return f;
  })(),
);
workViewFilters.subscribe((v) => writePref('work.filters', normalizeFilters(v)));

const isViewId = (v: unknown): v is number | null => v === null || (typeof v === 'number' && Number.isInteger(v));
/** The saved view the filters came from (null: none). */
export const activeWorkViewId = writable<number | null>(readPref('work.view_id', null, isViewId));
activeWorkViewId.subscribe((v) => writePref('work.view_id', v));

/** The key per-view state is kept under. */
export const workViewKey: Readable<string> = derived(activeWorkViewId, (id) => (id == null ? 'custom' : `view:${id}`));

type Nested = Record<string, Record<string, boolean>>;
const isNested = (v: unknown): v is Nested =>
  isObj(v) && Object.values(v).every((x) => isObj(x) && Object.values(x).every((b) => typeof b === 'boolean'));
/** view key → section key → expanded. */
export const workExpanded = writable<Nested>(readPref('work.expanded', {}, isNested));
workExpanded.subscribe((v) => writePref('work.expanded', v));

export function setExpanded(viewKey: string, key: string, open: boolean): void {
  workExpanded.update((m) => ({ ...m, [viewKey]: { ...(m[viewKey] ?? {}), [key]: open } }));
}

const isSelMap = (v: unknown): v is Record<string, string> =>
  isObj(v) && Object.values(v).every((x) => typeof x === 'string');
const selectedByView = writable<Record<string, string>>(readPref('work.selected', {}, isSelMap));
selectedByView.subscribe((v) => writePref('work.selected', v));

/** The task selected in the Work view (its id), kept per view. The selection
 *  itself lives in `selection.ts` (one store for sessions and tasks). */
export { selectedTaskId };
/** Whether Details shows the selected task (until a session is opened). */
export const taskDetailOpen = taskFocused;
// Start from the stored pick, before the write-back below can see an empty one.
selectedTaskId.set(get(selectedByView)[get(workViewKey)] ?? null);
let restoring = false;
selectedTaskId.subscribe((id) => {
  if (restoring) return;
  const k = get(workViewKey);
  selectedByView.update((m) => {
    const next = { ...m };
    if (id) next[k] = id;
    else delete next[k];
    return next;
  });
});
workViewKey.subscribe((k) => {
  restoring = true;
  const id = get(selectedByView)[k] ?? null;
  // A view switch while a task has the focus picks the view's task, so its
  // detail never sits beside the other view's session.
  if (id && get(taskDetailOpen)) pickTask(id);
  else selectedTaskId.set(id);
  restoring = false;
});

/** Select a task and show it in Details; the center pane follows it
 *  (`pickTask`). Pass the task's session links when the caller has them. */
export function openTask(taskId: string, links?: readonly TaskSessionLink[]): void {
  pickTask(taskId, links);
}

/** A request to scroll the Work tree to a task (and load its section). */
export const revealTaskRequest = writable<{ taskId: string; seq: number } | null>(null);
let revealSeqN = 0;

/** "Show in Work view": switch the sidebar, select the task, reveal it. */
export function showTaskInWorkView(taskId: string, links?: readonly TaskSessionLink[]): void {
  sidebarView.set('work');
  openTask(taskId, links);
  revealTaskRequest.set({ taskId, seq: ++revealSeqN });
}

/** The last tree page's orgs, trackers and group labels, for names in the
 *  detail and the placement picker. */
export const workTreeMeta = writable<{ orgs: WorkTreeOrg[]; trackers: WorkTreeTracker[]; groups: WorkTreeGroup[] }>({
  orgs: [],
  trackers: [],
  groups: [],
});

/** ⌘⇧O in the Work view: the next organisation in the Work view's own org
 *  filter (any → each org → unassigned → any), as the chord cycles the
 *  Sessions list's scope there. */
export function cycleWorkOrg(): void {
  const orgs = get(workTreeMeta).orgs;
  const ids: (number | 'none' | undefined)[] = [undefined, ...orgs.map((o) => o.id), 'none'];
  const cur = normalizeFilters(get(workViewFilters)).org;
  const i = ids.indexOf(cur);
  const next = ids[(i + 1) % ids.length];
  workViewFilters.update((f) => {
    const { group: _g, org: _o, ...rest } = normalizeFilters(f);
    return next === undefined ? rest : { ...rest, org: next };
  });
}

/** Bumped by every work write (`work.ts`), by `work:changed` and by session
 *  events that touch work; the Work view re-reads what it shows (debounced)
 *  when it moves. Lives in `work.ts` so its link wrappers bump it too. */
export { workChanged, bumpWorkChanged, type WorkChangeKind };

/** The session ids the loaded tree shows, so their status changes refresh
 *  it even when they have no primary work. */
export const workTreeSessionIds = writable<ReadonlySet<number>>(new Set());

/** A `work:changed` frame (ids only). */
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

/** The attention a row's "needs you" is drawn from: the triage bucket
 *  (`attention.ts`, the hub's `needs_attention_with` reasons), with the
 *  buckets the hub never counts folded away — `working` / `idle` (they
 *  flip on every turn and the tree draws nothing from them) and
 *  `done_unread` (a read / unread flip is not the tree's). With
 *  `idleSecs: 0`, `idle_long` never fires, so it is deterministic. */
function attentionSig(r: SessionRow): TriageBucket | null {
  const b = classify(r, { idleSecs: 0, now: 0 });
  return b === 'working' || b === 'idle' || b === 'done_unread' ? null : b;
}

/** What of a row the Work view shows: its primary and suggested work, its
 *  org, `work_rev` (moves when ANY of its live links changes, a secondary
 *  one too), whether it is alive, and why it needs you. Not the raw
 *  `claude_status`: working ↔ idle flips on every turn and the tree draws
 *  nothing from it but "needs you". */
function workSig(r: SessionRow | undefined): string {
  if (!r) return '';
  const w = r.work;
  const s = r.work_suggested;
  return JSON.stringify([
    w?.link_id ?? null,
    w?.state ?? null,
    w?.item_id ?? null,
    w?.key ?? null,
    w?.archived_at ?? null,
    s?.link_id ?? null,
    s?.suggestions ?? null,
    r.org_id ?? null,
    r.work_rev ?? 0,
    r.status,
    attentionSig(r),
  ]);
}

/** Whether a batch of session events changes anything the Work view shows:
 *  a row with work (before or after) or a row the tree shows whose work,
 *  org or attention moved, or that was created / killed. Call BEFORE the
 *  events are applied to the store (it compares against it). */
export function sessionEventsTouchWork(
  events: readonly SessionEvent[],
  current: readonly SessionRow[] = get(sessions),
  shown: ReadonlySet<number> = get(workTreeSessionIds),
): boolean {
  const byId = new Map(current.map((r) => [r.id, r]));
  for (const e of events) {
    if (e.type === 'killed') {
      const prev = byId.get(e.id);
      if (shown.has(e.id) || prev?.work || prev?.work_suggested) return true;
      continue;
    }
    if (e.type !== 'created' && e.type !== 'updated') continue;
    const prev = byId.get(e.row.id);
    const involved = shown.has(e.row.id) || !!(prev?.work || prev?.work_suggested || e.row.work || e.row.work_suggested);
    if (involved && workSig(prev) !== workSig(e.row)) return true;
  }
  return false;
}

/** Route the batched `work:*` frames: an `item` frame (a tracker item the
 *  sync changed: its title, status or group) bumps the tick as a
 *  `placement` — the tasks move, never a saved view or a rule. */
export function noteWorkEvents(events: readonly { type: string }[]): void {
  if (events.some((e) => e.type === 'item')) bumpWorkChanged('placement');
}

/** `work:changed` frames (placements, rules, views, a task's org, or the
 *  desktop's own `resync` after a stream gap): one bump carrying every
 *  kind, so each reader re-reads only what it shows — the saved views on
 *  `view`, the rules on `rule`, the tree on anything (and on `resync`, the
 *  whole view, dropping the sections it kept). */
export function noteWorkChanged(changes: readonly WorkChanged[]): void {
  if (changes.length > 0) bumpWorkChanged(...new Set(changes.map((c) => c.what)));
}
