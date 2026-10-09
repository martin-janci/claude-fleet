// Work links (roadmap M1b.2): which work a session is doing, decided by a
// person. Thin wrappers over the `*_session_work` commands; each mutation
// answers the session's updated row (its `work` / `work_rejected`), which is
// patched into the sessions store right away — the `session:updated` event
// the backend also emits then lands as a no-op.

import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { acceptCommandRow, sessions, type SessionRow } from './sessions';
import type { WorkItemRow } from './trackers';
import { timeAgo } from './session_status';
import type { ProposalLike } from './ai_proposal';

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

/** Bumped after every work write this window makes, and by `work:changed`
 *  and session events that touch work; the Work view, the task detail and
 *  the session's Tasks re-read what they show (debounced) when it moves.
 *  (Re-exported by `work_view.ts`, where its readers live.) */
export const workChanged = writable(0);

/** What moved, per bump: a session event (`session`), a `work:changed`
 *  kind from the hub (`placement` / `org` / `rule` / `view`), the stream's
 *  own `resync` after a gap, or a write this window made (`local`, which
 *  may have moved anything). Readers that show only some of it (the saved
 *  views, the rules) skip the rest. */
export type WorkChangeKind = 'session' | 'placement' | 'org' | 'rule' | 'view' | 'resync' | 'local';

const CHANGE_KINDS: ReadonlySet<string> = new Set<WorkChangeKind>([
  'session',
  'placement',
  'org',
  'rule',
  'view',
  'resync',
  'local',
]);

// The kinds of the bump being delivered: set just before the store moves,
// read by `onWorkChangedDebounced`'s subscribers in the same tick.
let bumpKinds: readonly WorkChangeKind[] = ['local'];

export function bumpWorkChanged(...kinds: WorkChangeKind[]): void {
  const known = kinds.filter((k) => CHANGE_KINDS.has(k));
  bumpKinds = known.length > 0 ? known : ['local'];
  workChanged.update((n) => n + 1);
}

/** Whether a debounced run's kinds include any of `want`. */
export function changedAny(kinds: ReadonlySet<WorkChangeKind>, ...want: WorkChangeKind[]): boolean {
  return want.some((k) => kinds.has(k));
}

/** A link decision's answer: the session's row and, for a confirm / reject
 *  of one link by id, that link's version after the write
 *  (`link_version`, read under the write's own lock; absent from a hub
 *  built before it). The row the store keeps never carries it. */
export type DecidedRow = SessionRow & { link_version?: number };

/** Run `fn` once `workChanged` has been quiet for `ms()` after a bump — one
 *  re-read for a burst (a write's own bump, then its `session:updated`),
 *  never for the subscription's initial call. `fn` is given every kind the
 *  burst carried. `ms` is read per bump, so a component can pass its prop.
 *  With `maxWaitMs`, the run comes at most that long after the first bump
 *  it waits for, so a steady stream of bumps (a busy fleet) cannot hold it
 *  back forever. The returned unsubscriber also cancels a pending run. */
export function onWorkChangedDebounced(
  fn: (kinds: ReadonlySet<WorkChangeKind>) => void,
  ms: () => number,
  maxWaitMs?: () => number,
): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pendingSince: number | null = null;
  let pending = new Set<WorkChangeKind>();
  let first = true;
  const off = workChanged.subscribe(() => {
    if (first) {
      first = false;
      return;
    }
    for (const k of bumpKinds) pending.add(k);
    clearTimeout(timer);
    let wait = ms();
    if (maxWaitMs) {
      const now = Date.now();
      pendingSince ??= now;
      wait = Math.max(0, Math.min(wait, pendingSince + maxWaitMs() - now));
    }
    timer = setTimeout(() => {
      pendingSince = null;
      const kinds = pending;
      pending = new Set();
      fn(kinds);
    }, wait);
  });
  return () => {
    off();
    clearTimeout(timer);
  };
}

async function decide(cmd: string, args: Record<string, unknown>): Promise<Result<DecidedRow>> {
  const r = await invokeCmd<DecidedRow>(cmd, { args });
  if (r.ok) {
    const row: DecidedRow = { ...r.value };
    delete row.link_version;
    acceptCommandRow(row);
    bumpWorkChanged();
  }
  return r;
}

/** The Work view's guards on a decision (work graph M14): `primary: false`
 *  links or confirms a secondary and leaves the primary where it is;
 *  `expectedVersion` is the link version the person saw (`E_CONFLICT` when
 *  it moved). Absent, a decision behaves as it always has. */
export interface WorkDecisionGuards {
  primary?: boolean;
  expectedVersion?: number;
}

function guards(opts: WorkDecisionGuards): Record<string, unknown> {
  return {
    ...(opts.primary !== undefined ? { primary: opts.primary } : {}),
    ...(opts.expectedVersion !== undefined ? { expected_version: opts.expectedVersion } : {}),
  };
}

/** Say the session works on `ref`; it becomes the session's primary work
 *  (unless `primary: false`).
 *  `forceCrossOrg`: the person saw the cross-org refusal and meant it. */
export function linkSessionWork(
  sessionId: number,
  ref: WorkRef,
  opts: { forceCrossOrg?: boolean; ackLive?: boolean } & WorkDecisionGuards = {},
): Promise<Result<SessionRow>> {
  return decide('link_session_work', {
    session_id: sessionId,
    ...ref,
    ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
    ...(opts.ackLive !== undefined ? { ack_live: opts.ackLive } : {}),
    ...guards(opts),
  });
}

/** Task → session P-2: move the session from the work of its live link
 *  `fromLinkId` to `ref` in one step. The old link ends (its conversation
 *  stays Continue-able on the old task) and `ref` becomes the primary; a
 *  compare-and-set on the primary the person saw (`expectedPrimary`). */
export function switchSessionWork(
  sessionId: number,
  fromLinkId: number,
  ref: WorkRef,
  opts: { expectedPrimary?: number; ackLive?: boolean; forceCrossOrg?: boolean } = {},
): Promise<Result<SessionRow>> {
  return decide('switch_session_work', {
    session_id: sessionId,
    link_id: fromLinkId,
    ...ref,
    ...(opts.expectedPrimary !== undefined ? { expected_primary: opts.expectedPrimary } : {}),
    ...(opts.ackLive !== undefined ? { ack_live: opts.ackLive } : {}),
    ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
  });
}

/** One other live session on the task a link or switch named (P-3). Its
 *  id only when this client may see it. */
export interface LiveElsewhere {
  session_id?: number;
  message: string;
}

/** The P-3 refusal (`ack_live: false`): the task's other live sessions, or
 *  null for any other error. */
export function liveElsewhereOf(e: { code: string; details?: unknown }): LiveElsewhere[] | null {
  const d = e.details as { live_elsewhere?: unknown } | undefined;
  if (e.code !== 'E_EXISTS' || !Array.isArray(d?.live_elsewhere)) return null;
  return (d.live_elsewhere as LiveElsewhere[]).filter((l) => typeof l?.message === 'string');
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
  opts: { forceCrossOrg?: boolean } & WorkDecisionGuards = {},
): Promise<Result<DecidedRow>> {
  return decide('confirm_session_work', {
    session_id: sessionId,
    link_id: linkId,
    ...(opts.forceCrossOrg ? { force_cross_org: true } : {}),
    ...guards(opts),
  });
}

/** "Not this" for one detected link (a suggestion or an auto link, e.g. the
 *  Undo of an automatic link). Sticky: never proposed again. */
export function rejectWorkLink(
  sessionId: number,
  linkId: number,
  opts: Pick<WorkDecisionGuards, 'expectedVersion'> = {},
): Promise<Result<DecidedRow>> {
  return decide('reject_session_work', { session_id: sessionId, link_id: linkId, ...guards(opts) });
}

/** Trust (or stop trusting) branch keys in a project: a sole branch key there
 *  then links by itself, with Undo. Answers the trusted project ids. */
export async function setWorkProjectTrust(projectId: number, on: boolean): Promise<Result<number[]>> {
  const r = await invokeCmd<{ trusted?: number[] }>('set_work_project_trust', {
    args: { project_id: projectId, on },
  });
  return r.ok ? { ok: true, value: r.value?.trusted ?? [] } : r;
}

/** The rule a decision-model suggestion carries (J1 `work_link`, redesign
 *  6.8): it keeps it after a person decides, when its source becomes theirs. */
export const JEV_RULE = 'R12';

/** Who proposed a link, when the decision model did (J1, rule R12): the
 *  desktop's mirror of `service::work::view::proposer_of`, with the
 *  confidence its last `jev` evidence note holds (`82%`). Null for a
 *  rule's own reading. A J1 answer only becomes a link in assist mode;
 *  shadow records it and writes nothing, so nothing here shows it. */
export function linkProposal(
  l: Pick<WorkLink, 'rule' | 'evidence' | 'ref_key' | 'item_id'>,
): ProposalLike | null {
  if (l.rule !== JEV_RULE) return null;
  let confidence: number | null = null;
  for (const e of l.evidence ?? []) {
    if (e.signal !== 'jev' || !e.note) continue;
    const n = Number.parseInt(e.note.replace(/%$/, ''), 10);
    if (Number.isFinite(n)) confidence = n;
  }
  return {
    value: l.ref_key ?? (l.item_id != null ? `item ${l.item_id}` : 'work'),
    source: 'jev',
    reason: 'from the first prompt',
    confidence_pct: confidence,
  };
}

/** Link sources detection writes; a confirmed link with one is "auto". */
export const AUTO_SOURCES: readonly string[] = ['branch', 'pr', 'trailer', 'url', 'prompt', 'agent_inferred', 'jev'];

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
  agent_inferred: "Claude's guess when asked",
  jev: 'proposed by Jev',
  manual: 'linked by you',
  started: 'started for it',
  agent: 'declared by Claude',
  agent_started: 'started for it by Claude',
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
      return `Claude named ${e.text} when asked at ${clock(e.at)}${rule}`;
    case 'jev':
      return `Jev proposed ${e.text} from the first prompt at ${clock(e.at)}${note}${rule}`;
    default:
      return `${e.signal}: ${e.text}${rule}`;
  }
}

/** The one-line "why" of a row's link or suggestion, for a chip tooltip. */
export function workWhy(w: { source: string; state?: string; rule?: string | null }): string {
  const what = w.state === 'suggested' ? 'suggested from the' : 'from the';
  const rule = w.rule ? ` · rule ${w.rule}` : '';
  // The classification nudge's answer (M4.6) is Claude's, not a signal's.
  if (w.source === 'agent_inferred') return `${w.state === 'suggested' ? 'suggested' : 'named'} by Claude when asked${rule}`;
  // The decision model's answer (J1, redesign 6.8) is Jev's, not a signal's.
  if (w.source === 'jev' || w.rule === JEV_RULE) return `proposed by Jev from the first prompt${rule}`;
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

/** Remove a mistaken link (not a rejection: the key may come back — but not
 *  from the unchanged branch or pull request that named it, rule R9u). */
export function unlinkSessionWork(
  sessionId: number,
  linkId: number,
  opts: Pick<WorkDecisionGuards, 'expectedVersion'> = {},
): Promise<Result<SessionRow>> {
  return decide('unlink_session_work', { session_id: sessionId, link_id: linkId, ...guards(opts) });
}

// ── Local work: "Name this work…" (work graph M11.1) ──

/** Longest title a person may give local work (`LOCAL_WORK_TITLE_MAX_CHARS`). */
export const LOCAL_WORK_TITLE_MAX = 120;

/** Why `raw` is not a usable work title, or null when it is: the backend's
 *  rule (trimmed, 1–120 characters, no control characters). */
export function workTitleError(raw: string): string | null {
  const t = raw.trim();
  if (t === '') return 'A title is required.';
  if ([...t].length > LOCAL_WORK_TITLE_MAX) return `At most ${LOCAL_WORK_TITLE_MAX} characters.`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(t)) return 'No control characters.';
  return null;
}

/** One row of `work { action: local_items }`. */
export interface LocalWorkItem {
  id: number;
  key?: string | null;
  title: string;
  created_at: number;
  updated_at?: number;
  /** Live sessions linked to it (a per-host view counts its host's only). */
  live_sessions?: number;
}

/** Name new local work (a title, an optional key) and link the session to
 *  it: manual and confirmed, primary when the session has no primary work.
 *  A key a visible ticket or a local item already carries is refused with
 *  `E_EXISTS` — link that instead. */
export function nameSessionWork(
  sessionId: number,
  title: string,
  key?: string | null,
): Promise<Result<SessionRow>> {
  const k = key?.trim();
  return decide('name_session_work', {
    session_id: sessionId,
    title: title.trim(),
    ...(k ? { key: k } : {}),
  });
}

/** Name one piece of work for several sessions (a group header's "Name
 *  this work…"): the first session names it, the rest link to the new item.
 *  Stops at the first failure; answers the rows it updated. */
export async function nameWorkForSessions(
  sessionIds: readonly number[],
  title: string,
  key?: string | null,
): Promise<Result<SessionRow[]>> {
  const [first, ...rest] = sessionIds;
  if (first === undefined) return { ok: true, value: [] };
  const named = await nameSessionWork(first, title, key);
  if (!named.ok) return named;
  const out = [named.value];
  const itemId = named.value.work?.item_id;
  if (itemId == null) return { ok: true, value: out };
  for (const id of rest) {
    const r = await linkSessionWork(id, { item_id: itemId });
    if (!r.ok) return r;
    out.push(r.value);
  }
  return { ok: true, value: out };
}

/** Show a local item's new title on every row that shows it, before the
 *  backend's `session:updated` frames arrive (they carry a newer
 *  `row_version`, so they replace this patch). */
export function patchWorkItemTitle(itemId: number, title: string): void {
  sessions.update((arr) => {
    let changed = false;
    const next = arr.map((s) => {
      const work = s.work?.item_id === itemId ? { ...s.work, title } : s.work;
      const sugg =
        s.work_suggested?.item_id === itemId ? { ...s.work_suggested, title } : s.work_suggested;
      if (work === s.work && sugg === s.work_suggested) return s;
      changed = true;
      return { ...s, work, work_suggested: sugg };
    });
    return changed ? next : arr;
  });
}

/** Rename a local work item (a tracker's ticket is refused, `E_INVALID`). */
export async function renameWorkItem(itemId: number, title: string): Promise<Result<WorkItemRow>> {
  const r = await invokeCmd<WorkItemRow>('rename_work_item', {
    args: { item_id: itemId, title: title.trim() },
  });
  if (r.ok) {
    patchWorkItemTitle(itemId, r.value.title);
    bumpWorkChanged();
  }
  return r;
}

/** A task, or a subtask under `parent` (`item:<id>`), written in Fleet. */
export async function createWorkTask(input: {
  title: string;
  parent?: string | null;
  projectId?: number | null;
  notes?: string | null;
}): Promise<Result<WorkItemRow>> {
  const notes = input.notes?.trim();
  const r = await invokeCmd<WorkItemRow>('create_work_task', {
    args: {
      title: input.title.trim(),
      ...(input.parent ? { parent: input.parent } : {}),
      ...(input.projectId != null ? { project_id: input.projectId } : {}),
      ...(notes ? { notes } : {}),
    },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}

/** The statuses a person may give a native item (`blocked` is a session's
 *  state, never an item's). */
export type WorkItemStatus = 'todo' | 'in_progress' | 'done';

/** A person's status for a native item: final over the derived one
 *  (sprints design 2026-09-28 §2). A tracker's ticket is refused,
 *  `E_INVALID`, naming it: its status is its tracker's. */
export async function setWorkStatus(itemId: number, status: WorkItemStatus): Promise<Result<WorkItemRow>> {
  const r = await invokeCmd<WorkItemRow>('set_work_status', { args: { item_id: itemId, status } });
  if (r.ok) bumpWorkChanged();
  return r;
}

/** A person's edit of a native item: each field left out stays as it is;
 *  `notes: ''` and `assignees: []` clear them. A tracker's ticket is
 *  refused, `E_INVALID`, naming it: its text is its tracker's. */
export async function editWorkItem(
  itemId: number,
  edit: { title?: string; notes?: string; assignees?: string[] },
): Promise<Result<WorkItemRow>> {
  const args: Record<string, unknown> = { item_id: itemId };
  if (edit.title !== undefined) args.title = edit.title.trim();
  if (edit.notes !== undefined) args.notes = edit.notes;
  if (edit.assignees !== undefined) args.assignees = edit.assignees;
  const r = await invokeCmd<WorkItemRow>('edit_work_item', { args });
  if (r.ok) {
    if (edit.title !== undefined) patchWorkItemTitle(itemId, r.value.title);
    bumpWorkChanged();
  }
  return r;
}

/** Assignees typed as one line: split on commas, trimmed, empty ones and
 *  repeats (case-insensitive) dropped, as the backend stores them. */
export function parseAssignees(raw: string): string[] {
  const out: string[] = [];
  for (const a of raw.split(',').map((x) => x.trim())) {
    if (a && !out.some((o) => o.toLowerCase() === a.toLowerCase())) out.push(a);
  }
  return out;
}

/** A person accepts or rejects an agent's proposed subtask. */
export async function decideWorkProposal(itemId: number, accept: boolean): Promise<Result<WorkItemRow>> {
  const r = await invokeCmd<WorkItemRow>(accept ? 'accept_work_proposal' : 'reject_work_proposal', {
    args: { item_id: itemId },
  });
  if (r.ok) bumpWorkChanged();
  return r;
}

/** Redesign 6.9: Merge a proposal into the task it duplicates. What hangs
 *  on the proposal (its session links, its subtasks) moves to `intoItemId`,
 *  then the proposal closes as rejected. Only ever a person's click. */
export async function mergeWorkProposal(itemId: number, intoItemId: number): Promise<Result<WorkItemRow>> {
  const r = await invokeCmd<WorkItemRow>('reject_work_proposal', {
    args: { item_id: itemId, merge_into: intoItemId },
  });
  if (r.ok) bumpWorkChanged();
  return r;
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
  /** What the plan could not check; never blocks a mode (M11.2). */
  warnings?: string[];
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

/**
 * The conversation ids an ended link's snapshot names.
 *
 * `snap_claude_ids` is written by SQLite's `json_group_array` over the session's
 * conversations (migration 046, and `store/work_detect.rs` on every re-snap), so
 * it is a JSON array of strings on the wire. Anything this build cannot parse
 * names NOTHING rather than something: the only caller uses the answer to
 * CONFIRM an identity, so an empty list fails closed.
 */
function snapClaudeIds(raw: string | null | undefined): string[] {
  if (!raw) return [];
  try {
    const v: unknown = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    return v.filter((x): x is string => typeof x === 'string' && x !== '');
  } catch {
    return [];
  }
}

/**
 * The session an ended link came from — or `null` when this client cannot
 * identify it, which is not the same as "there is none".
 *
 * Multi-user M1 (F2c). `resume_work` is `share.ts`'s `own` tier: it re-opens a
 * past session's conversation, and the question "whose?" is about that SOURCE
 * session. `WorkLink` names no `session_id` (the link rows predate the
 * question), so the row has to be found from the snapshot it does carry.
 *
 * ── Why a name match is not an identity (F2d) ──────────────────────────────
 *
 * F2c matched on `snap_host` + `snap_tmux` alone. A tmux name is NOT a session
 * identity over time: this repo's own reconcile logic treats a lost row's name
 * as reusable, so `(host, tmux)` can name a row today that merely INHERITED the
 * pane name from the session the link came from. Resolving to it answers the
 * access question about the wrong row — and in the direction that matters, since
 * the inheriting row is typically the one the current person just started, so an
 * `own` answer would be handed out for somebody else's transcript.
 *
 * Two rows can hold one `(host, tmux)` at once as well (a lost row plus the live
 * one that reused its name), and `find` would silently take whichever came
 * first in the list.
 *
 * So the match has to be CORROBORATED, and the link carries the one thing that
 * does it: `snap_claude_ids`, the conversation ids the session had when the link
 * ended. A Claude conversation id is a uuid, so a row whose current
 * `claude_session_id` is one of them is the same session and not a namesake.
 * Three refusals, all answering `null`:
 *
 *   1. more than one row holds the name — ambiguous, so unidentifiable;
 *   2. the snapshot names no conversation (an older hub, or a session that
 *      never ran one) — nothing to corroborate with;
 *   3. the row's conversation is not among them — either a namesake, or the
 *      same session long since moved on by a `/clear`.
 *
 * (3) is the price: a live session that started a new conversation after the
 * link ended stops being identifiable from the link, so a paired desktop loses
 * Resume on it. That is the fail-closed direction and the undo is the backend's
 * — the plan's T7 handoff asks for the link to carry its session (or its owner)
 * — not a wider match here.
 *
 * `null` is NOT "nothing to check" either: the hub fences rows this client may
 * not see off the stream, so an unresolvable link is one whose owner we cannot
 * vouch for. `share.ts::sessionIdActionBlocked` reads it that way and fails
 * closed on a paired desktop, while answering `null` on a standalone one, where
 * the master owns every row.
 */
export function linkSessionId(
  link: Pick<WorkLink, 'snap_host' | 'snap_tmux' | 'snap_claude_ids'> | null | undefined,
  rows: readonly Pick<SessionRow, 'id' | 'host_alias' | 'tmux_name' | 'claude_session_id'>[],
): number | null {
  const host = link?.snap_host;
  const tmux = link?.snap_tmux;
  if (!host || !tmux) return null;
  const named = rows.filter((r) => r.host_alias === host && r.tmux_name === tmux);
  if (named.length !== 1) return null;
  const row = named[0];
  const conv = row.claude_session_id;
  if (!conv) return null;
  return snapClaudeIds(link?.snap_claude_ids).includes(conv) ? row.id : null;
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
  if (r.ok) {
    acceptCommandRow(r.value);
    bumpWorkChanged();
  }
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


/** A past session's summary (work graph M13.4c): `work_link { summarize }`. */
export interface SummaryOutcome {
  key: string;
  link_id: number;
  host_alias: string;
  claude_session_id: string;
  model: string;
  journal_id: number;
  at: number;
  /** Fenced as untrusted: Claude's reading of a transcript. */
  summary: string;
  truncated?: boolean;
}

/**
 * Ask for a Claude-written summary of past work `linkId` of `key` (work
 * graph M13.4c, on demand only). One print-mode fork runs on the session's own
 * host, with no tools; the reply replaces that conversation's earlier
 * summary, and the next resume brief shows it.
 */
export async function summarizePastWork(key: string, linkId: number): Promise<Result<SummaryOutcome>> {
  return invokeCmd<SummaryOutcome>('summarize_past_work', { args: { key, link_id: linkId } });
}

/** Where the latest handover request stands (work graph M9.3). */
export type HandoverState = 'pending' | 'written' | 'missing' | 'failed';

export interface HandoverOutcome {
  state: HandoverState;
  /** unix seconds of the event that says so */
  at: number;
}

/** Mirrors `agent_handover::PENDING_TTL_SECS`: an older request is abandoned. */
export const HANDOVER_PENDING_TTL_SECS = 30 * 60;

const HANDOVER_STATES: Record<string, HandoverState> = {
  handover_requested: 'pending',
  handover_written: 'written',
  handover_missing: 'missing',
  handover_send_failed: 'failed',
};

/**
 * The latest handover outcome from a session's timeline: the newest
 * `handover_*` event. A request older than the hub's pending window is
 * abandoned (null), as the hub treats it.
 */
export function handoverOutcome(
  events: readonly { kind: string; at: number; id?: number }[] | null | undefined,
  nowSecs: number = Math.floor(Date.now() / 1000),
): HandoverOutcome | null {
  let newest: { kind: string; at: number; id?: number } | null = null;
  for (const e of events ?? []) {
    if (!(e.kind in HANDOVER_STATES)) continue;
    if (!newest || e.at > newest.at || (e.at === newest.at && (e.id ?? 0) > (newest.id ?? 0))) newest = e;
  }
  if (!newest) return null;
  const state = HANDOVER_STATES[newest.kind];
  if (state === 'pending' && newest.at <= nowSecs - HANDOVER_PENDING_TTL_SECS) return null;
  return { state, at: newest.at };
}

/** One line on a handover outcome, for the ticket card. */
export function handoverOutcomeLine(o: HandoverOutcome, nowMs: number = Date.now()): string {
  const ago = timeAgo(o.at, nowMs);
  switch (o.state) {
    case 'pending':
      return `Handover asked ${ago}; waiting for the reply`;
    case 'written':
      return `Handover written ${ago}; the next session's brief shows it`;
    case 'missing':
      return `The reply ${ago} had no handover; ask again`;
    case 'failed':
      return `Asking for a handover failed ${ago}`;
  }
}
