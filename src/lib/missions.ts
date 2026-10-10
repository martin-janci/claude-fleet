// Missions (orchestration O1, design 2026-10-07 §9): a goal with a root
// task, member tasks, the repos it may run in and its decision log. Thin
// wrappers over the `*_mission` commands; the rules (who may read or change
// one, the lifecycle, the item cap) live in fleet-core's
// `service::work::missions`, served by the hub on a paired desktop.

import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { WorkItemRow } from './trackers';
import { bumpWorkChanged } from './work';
import type { StatusWord } from './kit/status';

/** One mission (`store::MissionRow`). */
export interface Mission {
  id: number;
  org_id?: number | null;
  owner_person_id?: number | null;
  root_item_id?: number | null;
  name: string;
  goal: string;
  non_goals?: string | null;
  done_when?: string[];
  /** `finite` | `continuous` — tolerant of more. */
  mode: string;
  /** `draft` | `active` | `paused` | `completed` | `failed` | `cancelled`. */
  state: string;
  /** The autonomy asked for, 0–3. */
  level: number;
  plan_version: number;
  created_at: number;
  updated_at: number;
  started_at?: number | null;
  finished_at?: number | null;
  version: number;
  total?: number;
  done?: number;
  repos?: MissionRepo[];
  /** What it asks of its loop (O4); absent from a hub older than O4. */
  policy?: MissionPolicy;
}

/** `store::MissionPolicy`. The backend reads a missing field as its
 *  default, so a save sends the whole policy back with its edits on top. */
export interface MissionPolicy {
  max_parallel?: number;
  max_retries?: number;
  require_review?: boolean;
  task_creation?: string;
  max_tasks?: number;
  max_planner_runs_per_hour?: number;
  no_progress_secs?: number;
  planner_model?: string | null;
  planner_host?: string | null;
  /** A continuous mission's timer: wake at least this often (≥ 300 s). */
  wake_every_secs?: number | null;
}

/** `store::POLICY_MAX_PARALLEL` and `POLICY_MIN_WAKE_SECS`. */
export const POLICY_MAX_PARALLEL = 8;
export const POLICY_MIN_WAKE_SECS = 300;
/** The default runs at once (`MissionPolicy::default`). */
export const POLICY_DEFAULT_PARALLEL = 2;

/** The policy to save: the mission's own with `edits` on top, so a field
 *  the form does not show keeps its value. `wake_every_secs: null` clears
 *  the timer. */
export function policyWith(current: MissionPolicy | undefined, edits: MissionPolicy): MissionPolicy {
  const out: MissionPolicy = { ...(current ?? {}), ...edits };
  if (out.wake_every_secs == null) delete out.wake_every_secs;
  return out;
}

/** "every 10 min", "every 2 h", or `null` without a timer. */
export function wakeLabel(secs: number | null | undefined): string | null {
  if (!secs) return null;
  if (secs % 3600 === 0) return `every ${secs / 3600} h`;
  return `every ${Math.round(secs / 60)} min`;
}

export interface MissionRepo {
  project_id: number;
  name: string;
  role?: string | null;
  created_at: number;
}

export interface MissionEvent {
  id: number;
  at: number;
  kind: string;
  actor: string;
  work_item_id?: number | null;
  task_id?: number | null;
  decision_id?: string | null;
  payload?: unknown;
}

/** One node of a mission's graph (`service::work::graph::GraphNode`). */
export interface GraphNode {
  item_id: number;
  /** done | proposed | rejected | running | failed | held | doing |
   *  blocked | waiting | ready — tolerant of more. */
  state: string;
  wave: number;
  depends_on?: number[];
  waiting_for?: number[];
  /** Its done_when answer (O3); absent without lines. */
  verification?: Verification | null;
  /** Its latest attempt, when this reader may see the task. */
  attempt?: AttemptBrief | null;
}

/** One done_when line's answer (`service::work::verify::CondCheck`). */
export interface CondCheck {
  line: string;
  /** ci | review | test | person — tolerant of more. */
  kind: string;
  /** pass | fail | pending. */
  state: string;
  detail: string;
  by?: string | null;
  at?: number | null;
}

export interface Verification {
  /** verified | failed | unverified. */
  state: string;
  checks: CondCheck[];
}

/** What git said about an attempt's checkout (`store::TaskEvidence`). */
export interface TaskEvidence {
  at: number;
  head?: string | null;
  base?: string | null;
  commits?: { sha: string; subject: string }[];
  commits_total?: number;
  files?: { path: string; added?: number | null; removed?: number | null }[];
  files_total?: number;
  uncommitted?: number | null;
  error?: string | null;
}

/** A node's latest attempt (`service::work::graph::AttemptBrief`). */
export interface AttemptBrief {
  task_id: number;
  role?: string | null;
  attempt?: number | null;
  state: string;
  /** The worker's own reported outcome. */
  outcome?: string | null;
  summary?: string | null;
  error?: string | null;
  evidence?: TaskEvidence | null;
}

export interface OutsideItem {
  id: number;
  key?: string | null;
  title: string;
  status_category: string;
}

export interface MissionGraph {
  nodes?: GraphNode[];
  waves?: number;
  outside?: OutsideItem[];
}

/** `work_mission`'s answer. */
export interface MissionDetail {
  mission: Mission;
  items?: WorkItemRow[];
  events?: MissionEvent[];
  /** `running` | `blocked` | `waiting`, for an active mission only. */
  phase?: string | null;
  /** Absent from an older hub. */
  graph?: MissionGraph;
  may_change?: boolean;
  /** The loop (orchestration O4–O6); absent for a draft or finished one. */
  plan?: MissionPlan | null;
}

/** One next step of the loop (`orchestrate::steps::Step`). */
export interface MissionStep {
  /** `run` | `retry` | `review` | `test` | `integrate` | `close` | `complete` | `ask`. */
  kind: string;
  item_id?: number | null;
  role?: string | null;
  reason: string;
  context?: string | null;
  /** The loop may take it under a grant; an ask is a person's only. */
  auto: boolean;
}

/** One card of the confirm queue (`store::CardRow`). */
export interface MissionCard {
  id: number;
  mission_id: number;
  decision_id: string;
  /** `planner` | `loop`. */
  source: string;
  kind: string;
  work_item_id?: number | null;
  payload?: Record<string, unknown> | null;
  /** `open` | `applied` | `dismissed` | `refused` | `stale`. */
  state: string;
  note?: string | null;
  created_at: number;
  decided_at?: number | null;
  decided_by?: string | null;
}

/** A person's signature on what the loop may do (`store::GrantRow`). */
export interface MissionGrant {
  id: number;
  mission_id: number;
  plan_version: number;
  level: number;
  granted_by: string;
  hosts?: string[] | null;
  budget_micros?: number | null;
  max_parallel?: number | null;
  /** The login its runs bill (redesign 8.7); absent = the host's own. */
  profile?: string | null;
  created_at: number;
  expires_at: number;
  revoked_at?: number | null;
}

export interface MissionAutonomy {
  asked: number;
  ceiling: number;
  effective: number;
  grant?: MissionGrant | null;
  why: string;
  enabled: boolean;
}

export interface MissionPlan {
  steps?: MissionStep[];
  cards?: MissionCard[];
  autonomy: MissionAutonomy;
  cost_micros: number;
  counts: { total: number; open: number; last_activity_at?: number | null };
}

export interface StepResult {
  step: MissionStep;
  ok: boolean;
  detail: string;
  task_id?: number | null;
}

/** The key `start_mission_wave` takes for one step (`run:12`). */
export function stepKey(s: MissionStep): string {
  return `${s.kind}:${s.item_id ?? 0}`;
}

/** A step as a short line. */
export function stepLine(s: MissionStep): string {
  const verb: Record<string, string> = {
    run: 'Run',
    retry: 'Retry',
    review: 'Review',
    test: 'Test',
    integrate: 'Integrate',
    close: 'Close',
    complete: 'Complete the mission',
    ask: 'Ask',
  };
  return `${verb[s.kind] ?? s.kind}: ${s.reason}`;
}

/** What a card asks, in a sentence. */
export function cardLine(c: MissionCard): string {
  const p = (c.payload ?? {}) as Record<string, unknown>;
  const item = c.work_item_id != null ? ` task ${c.work_item_id}` : '';
  switch (c.kind) {
    case 'create': {
      const tree = Array.isArray(p.tree) ? (p.tree as { title?: string }[]) : [];
      const titles = tree.map((t) => t.title ?? '?');
      return `Create ${titles.length} task${titles.length === 1 ? '' : 's'}: ${titles.join(', ')}`;
    }
    case 'ask':
      return String(p.question ?? 'A question');
    case 'add_dep':
      return `Make${item} wait for ${String(p.depends_on ?? '?')}`;
    case 'remove_dep':
      return `Stop${item} waiting for ${String(p.depends_on ?? '?')}`;
    case 'run':
      return `Run${item}${p.role ? ` (${String(p.role)})` : ''}`;
    case 'retry':
      return `Retry${item}${p.note ? `: ${String(p.note)}` : ''}`;
    case 'complete':
      return 'Complete the mission';
    default:
      return `${c.kind.replace(/_/g, ' ')}${item}`;
  }
}

/** Dollars from micro-USD, two places. */
export function dollars(micros: number): string {
  return `$${(micros / 1e6).toFixed(2)}`;
}

/** What `save_mission` writes; on a change every field is optional. */
export interface MissionInput {
  name?: string;
  goal?: string;
  non_goals?: string;
  done_when?: string[];
  mode?: string;
  level?: number;
  org_id?: number;
  policy?: MissionPolicy;
}

/** The lifecycle moves a person may make from each state, in button order. */
export const MISSION_MOVES: Record<string, readonly string[]> = {
  draft: ['active', 'cancelled'],
  active: ['paused', 'completed', 'failed', 'cancelled'],
  paused: ['active', 'completed', 'failed', 'cancelled'],
};

const MOVE_LABEL: Record<string, string> = {
  active: 'Start',
  paused: 'Pause',
  completed: 'Complete',
  failed: 'Mark failed',
  cancelled: 'Cancel',
};

/** The button label for a move to `state`. */
export function moveLabel(from: string, to: string): string {
  if (from === 'paused' && to === 'active') return 'Resume';
  return MOVE_LABEL[to] ?? to;
}

/** The moves that end a mission. They sit in the ⋯ menu
 *  beside Edit and Pause, each behind a confirm (redesign parity row P19). */
export const FINAL_MOVES: readonly string[] = ['completed', 'failed', 'cancelled'];

/** A state's moves split for the mission header: Start, Pause and Resume stay
 *  buttons; Complete, Mark failed and Cancel go to the ⋯ menu. */
export function splitMoves(state: string): { inline: string[]; menu: string[] } {
  const all = MISSION_MOVES[state] ?? [];
  return {
    inline: all.filter((to) => !FINAL_MOVES.includes(to)),
    menu: all.filter((to) => FINAL_MOVES.includes(to)),
  };
}

/** The confirm line before a move from the ⋯ menu ends a mission. */
export function finalMoveQuestion(name: string, to: string): string {
  switch (to) {
    case 'completed':
      return `Complete ${name}? It stops changing; its tasks stay.`;
    case 'failed':
      return `Mark ${name} failed? It stops changing; its tasks stay.`;
    default:
      return `Cancel ${name}? It stops changing; its tasks stay.`;
  }
}

/** A mission that no longer changes. */
export function isFinal(state: string): boolean {
  return state === 'completed' || state === 'failed' || state === 'cancelled';
}

/** The state as a person reads it, with the loop's phase when it runs. */
export function stateLabel(state: string, phase?: string | null): string {
  const base = state.charAt(0).toUpperCase() + state.slice(1);
  return phase ? `${base} · ${phase}` : base;
}

/** `3/7 done`, or nothing for an empty mission. */
export function progressLabel(m: Pick<Mission, 'total' | 'done'>): string {
  const total = m.total ?? 0;
  return total > 0 ? `${m.done ?? 0}/${total} done` : '';
}

/** The non-empty lines of a textarea, as `done_when` rows. */
export function doneWhenRows(text: string): string[] {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.length > 0);
}

/** A node's state as one of the manual's six status words, and the reason
 *  after " · " when the word alone would hide what the node waits on. The
 *  dot's colour is `toneOf` (mission_graph.ts); no glyph prefixes. */
const NODE_WORDS: Record<string, [StatusWord, string | null]> = {
  done: ['Done', null],
  running: ['Working', null],
  doing: ['Working', null],
  verifying: ['Working', 'verifying'],
  failed: ['Failed', null],
  blocked: ['Failed', 'blocked'],
  proposed: ['Needs you', 'proposed'],
  held: ['Paused', null],
  waiting: ['Idle', 'waiting'],
  ready: ['Idle', 'ready'],
  rejected: ['Idle', 'rejected'],
};

/** A node's state in words: "Working", "Needs you · proposed". */
export function nodeLabel(state: string): string {
  const [word, why] = NODE_WORDS[state] ?? ['Idle', state];
  return why ? `${word} · ${why}` : word;
}

/** A node's state as a count's noun ("2 proposed", "1 working"): the reason
 *  when there is one, else the word. */
export function nodeCountWord(state: string): string {
  const [word, why] = NODE_WORDS[state] ?? ['Idle', state];
  return why ?? word.toLowerCase();
}

/** Node states a mission is working on now (a run or a person's task in
 *  progress): its current steps. */
const WORKING = new Set(['running', 'doing']);

/** Whether `detail`'s mission waits on a person: an open card in the
 *  confirm queue, or a next step only a person takes (`ask`). */
export function waitsOnPerson(detail: Pick<MissionDetail, 'plan'>): boolean {
  const plan = detail.plan;
  return (plan?.cards ?? []).some((c) => c.state === 'open') || (plan?.steps ?? []).some((s) => s.kind === 'ask');
}

/** The nodes that carry Comet trails (redesign step 9.12): the current
 *  steps of an active mission, and none while it waits on a person (the
 *  manual: no loader while waiting on a person). */
export function trailNodes(detail: MissionDetail): Set<number> {
  if (detail.mission.state !== 'active' || waitsOnPerson(detail)) return new Set();
  return new Set((detail.graph?.nodes ?? []).filter((n) => WORKING.has(n.state)).map((n) => n.item_id));
}

/** A running mission at a glance, for Control (redesign step 9.12): the
 *  title of its current step (the first task it is working on), whether it
 *  waits on a person, and whether that step carries Comet trails. */
export interface MissionNow {
  id: number;
  name: string;
  step: string | null;
  waiting: boolean;
  trails: boolean;
}

export function missionNow(detail: MissionDetail): MissionNow {
  const nodes = trailNodes(detail);
  const working = (detail.graph?.nodes ?? []).find((n) => WORKING.has(n.state));
  const item = working ? (detail.items ?? []).find((i) => i.id === working.item_id) : undefined;
  return {
    id: detail.mission.id,
    name: detail.mission.name,
    step: item ? item.title : null,
    waiting: waitsOnPerson(detail),
    trails: working !== undefined && nodes.has(working.item_id),
  };
}

/** The graph's nodes by wave, W1 first; items the graph leaves out (an
 *  older hub) land in one wave of their own. */
export function wavesOf(detail: MissionDetail): { wave: number; nodes: GraphNode[] }[] {
  const nodes = detail.graph?.nodes ?? [];
  const known = new Set(nodes.map((n) => n.item_id));
  const rest: GraphNode[] = (detail.items ?? [])
    .filter((i) => !known.has(i.id))
    .map((i) => ({ item_id: i.id, state: i.status_category === 'done' ? 'done' : 'ready', wave: 1 }));
  const by = new Map<number, GraphNode[]>();
  for (const n of [...nodes, ...rest]) {
    const w = by.get(n.wave) ?? [];
    w.push(n);
    by.set(n.wave, w);
  }
  return [...by.entries()].sort((a, b) => a[0] - b[0]).map(([wave, ns]) => ({ wave, nodes: ns }));
}

/** The proposals waiting for a decision. */
export function openProposals(detail: MissionDetail): number[] {
  return (detail.items ?? []).filter((i) => i.proposal_state === 'proposed').map((i) => i.id);
}

const VERIFIED_LABEL: Record<string, string> = {
  verified: 'Verified',
  failed: 'Not met',
  unverified: 'Unverified',
};

/** A verification's state in words. */
export function verificationLabel(state: string): string {
  return VERIFIED_LABEL[state] ?? state;
}

/** A check's state as one glyph. */
export function checkGlyph(state: string): string {
  return state === 'pass' ? '✓' : state === 'fail' ? '✕' : '○';
}

/** May a person record a check of this line from the card? A line fleet
 *  derives (CI, a run) is checkable too, when it is still open. */
export function checkable(c: CondCheck): boolean {
  return c.state !== 'pass';
}

/** An attempt in one line: `implement #2 · done · reported partial ·
 *  3 commits, 5 files`. The report is the worker's word; the counts are git's. */
export function attemptLine(a: AttemptBrief): string {
  const parts = [`${a.role ?? 'run'}${a.attempt ? ` #${a.attempt}` : ''}`, a.state];
  if (a.outcome) parts.push(`reported ${a.outcome}`);
  const ev = a.evidence;
  if (ev) {
    if (ev.error) parts.push(`git: ${ev.error}`);
    else {
      const c = ev.commits_total ?? 0;
      const f = ev.files_total ?? 0;
      parts.push(`${c} commit${c === 1 ? '' : 's'}, ${f} file${f === 1 ? '' : 's'}`);
      if (ev.uncommitted) parts.push(`${ev.uncommitted} uncommitted`);
    }
  }
  return parts.join(' · ');
}

/** One log row as a sentence. */
export function eventSentence(e: MissionEvent): string {
  const p = (e.payload ?? {}) as Record<string, unknown>;
  switch (e.kind) {
    case 'created':
      return 'Created';
    case 'state':
      return `${String(p.from ?? '?')} → ${String(p.to ?? '?')}`;
    case 'updated': {
      const fields = Array.isArray(p.fields) ? (p.fields as unknown[]).map(String) : [];
      return fields.length ? `Changed ${fields.join(', ')}` : 'Saved, nothing changed';
    }
    case 'repo_added':
      return `Repo ${String(p.project_id ?? '')} allowed${p.role ? ` (${String(p.role)})` : ''}`;
    case 'repo_removed':
      return `Repo ${String(p.project_id ?? '')} removed`;
    case 'item_added':
      return `Task ${e.work_item_id ?? ''} added`;
    case 'item_removed':
      return `Task ${e.work_item_id ?? ''} removed`;
    case 'dep_added':
      return `Task ${e.work_item_id ?? ''} waits for ${String(p.depends_on ?? '')}`;
    case 'dep_removed':
      return `Task ${e.work_item_id ?? ''} no longer waits for ${String(p.depends_on ?? '')}`;
    case 'held':
      return `Task ${e.work_item_id ?? ''} held`;
    case 'released':
      return `Task ${e.work_item_id ?? ''} released`;
    case 'done_when': {
      const lines = Array.isArray(p.lines) ? (p.lines as unknown[]).length : 0;
      return `Task ${e.work_item_id ?? ''} has ${lines} condition${lines === 1 ? '' : 's'}`;
    }
    case 'verify':
      return `${String(p.line ?? '')} ${p.ok ? 'checked' : 'found not met'} on task ${e.work_item_id ?? ''}`;
    case 'step':
      return `${String(p.step ?? 'step')}: ${String(p.detail ?? '')}`;
    case 'refused':
      return `Refused: ${String(p.why ?? p.detail ?? '')}`;
    case 'planned':
      return `Asked the planner (${String(p.why ?? '')})`;
    case 'card':
      return `Card ${String(p.card_id ?? '')} ${String(p.state ?? '')}`;
    case 'grant':
      return `Granted L${String(p.level ?? '?')} for ${String(p.hours ?? '?')} h`;
    case 'revoked':
      return 'Grant revoked';
    case 'budget':
    case 'no_progress':
      return `Paused: ${String(p.why ?? '')}`;
    case 'integration':
      return `Tasks ${String(p.a ?? '')} and ${String(p.b ?? '')} conflict`;
    case 'note':
      return String(p.text ?? 'Note');
    case 'digest':
      return 'Older events, summarised';
    default:
      return e.kind.replace(/_/g, ' ');
  }
}

/** A request to show one mission in the Work view's Missions tab (a
 *  "Sent to a mission" chip in Control, redesign 9.3). WorkTree switches to
 *  the tab; WorkMissions opens the mission and clears the request. */
export const missionOpenRequest = writable<{ id: number } | null>(null);

export function openMission(id: number): void {
  missionOpenRequest.set({ id });
}

export function listMissions(): Promise<Result<Mission[]>> {
  return invokeCmd<Mission[]>('work_missions', { args: {} });
}

export function getMission(missionId: number, beforeEvent?: number): Promise<Result<MissionDetail>> {
  return invokeCmd<MissionDetail>('work_mission', {
    args: { mission_id: missionId, ...(beforeEvent != null ? { before_event: beforeEvent } : {}) },
  });
}

async function changed<T>(p: Promise<Result<T>>): Promise<Result<T>> {
  const r = await p;
  if (r.ok) bumpWorkChanged();
  return r;
}

/** A new draft (rooted at `itemId`, else at a new task of its name). */
export function createMission(input: MissionInput, itemId?: number): Promise<Result<Mission>> {
  return changed(
    invokeCmd<Mission>('save_mission', {
      args: { mission: input, ...(itemId != null ? { item_id: itemId } : {}) },
    }),
  );
}

export function updateMission(
  missionId: number,
  input: MissionInput,
  expectedVersion?: number,
): Promise<Result<Mission>> {
  return invokeCmd<Mission>('save_mission', {
    args: {
      mission_id: missionId,
      mission: input,
      ...(expectedVersion != null ? { expected_version: expectedVersion } : {}),
    },
  });
}

export function setMissionState(
  missionId: number,
  state: string,
  expectedVersion?: number,
): Promise<Result<Mission>> {
  return invokeCmd<Mission>('set_mission_state', {
    args: {
      mission_id: missionId,
      state,
      ...(expectedVersion != null ? { expected_version: expectedVersion } : {}),
    },
  });
}

export function setMissionRepo(
  missionId: number,
  projectId: number,
  on: boolean,
  role?: string,
): Promise<Result<Mission>> {
  const r = role?.trim();
  return invokeCmd<Mission>('set_mission_repo', {
    args: { mission_id: missionId, project_id: projectId, on, ...(r ? { role: r } : {}) },
  });
}

export function setMissionItem(missionId: number, itemId: number, on: boolean): Promise<Result<Mission>> {
  return changed(
    invokeCmd<Mission>('set_mission_item', { args: { mission_id: missionId, item_id: itemId, on } }),
  );
}

export function deleteMission(missionId: number): Promise<Result<{ removed: number }>> {
  return changed(invokeCmd<{ removed: number }>('delete_mission', { args: { mission_id: missionId } }));
}

/** What a graph write answers. */
export interface GraphChange {
  item_id: number;
  changed: boolean;
}

export function setWorkDep(itemId: number, dependsOn: number, on: boolean): Promise<Result<GraphChange>> {
  return invokeCmd<GraphChange>('set_work_dep', { args: { item_id: itemId, depends_on: dependsOn, on } });
}

export function setWorkHold(itemId: number, on: boolean): Promise<Result<GraphChange>> {
  return invokeCmd<GraphChange>('set_work_hold', { args: { item_id: itemId, on } });
}

export function acceptWorkProposals(itemIds: number[]): Promise<Result<WorkItemRow[]>> {
  return changed(invokeCmd<WorkItemRow[]>('accept_work_proposals', { args: { item_ids: itemIds } }));
}

export function undoWorkAccept(itemIds: number[]): Promise<Result<WorkItemRow[]>> {
  return changed(invokeCmd<WorkItemRow[]>('undo_work_accept', { args: { item_ids: itemIds } }));
}

/** What `set_work_done_when` and `verify_work_item` answer. */
export interface VerifyOutcome {
  item_id: number;
  changed: boolean;
  verification?: Verification | null;
}

export function setWorkDoneWhen(itemId: number, lines: string[]): Promise<Result<VerifyOutcome>> {
  return invokeCmd<VerifyOutcome>('set_work_done_when', { args: { item_id: itemId, done_when: lines } });
}

export function verifyWorkItem(itemId: number, line: string, ok: boolean, note?: string): Promise<Result<VerifyOutcome>> {
  const n = note?.trim();
  return invokeCmd<VerifyOutcome>('verify_work_item', {
    args: { item_id: itemId, line, ok, ...(n ? { note: n } : {}) },
  });
}

/** What `start_mission_wave` answers. */
export interface StartOutcome {
  mission_id: number;
  results?: StepResult[];
}

/** Take the mission's next steps, or the one `step` names (`run:12`). */
export function startMissionWave(missionId: number, step?: string): Promise<Result<StartOutcome>> {
  return changed(
    invokeCmd<StartOutcome>('start_mission_wave', {
      args: { mission_id: missionId, ...(step ? { step } : {}) },
    }),
  );
}

export function retryWorkItem(itemId: number, note?: string): Promise<Result<StepResult>> {
  const n = note?.trim();
  return changed(invokeCmd<StepResult>('retry_work_item', { args: { item_id: itemId, ...(n ? { note: n } : {}) } }));
}

export interface PlanOutcome {
  mission_id: number;
  cards?: MissionCard[];
  refused?: string | null;
}

export function planMission(missionId: number): Promise<Result<PlanOutcome>> {
  return invokeCmd<PlanOutcome>('plan_mission', { args: { mission_id: missionId } });
}

/** Apply (`ok`) or dismiss a card; a question is answered with `note`. */
export function decideMissionCard(cardId: number, ok: boolean, note?: string): Promise<Result<MissionCard>> {
  const n = note?.trim();
  return changed(
    invokeCmd<MissionCard>('decide_mission_card', { args: { card_id: cardId, ok, ...(n ? { note: n } : {}) } }),
  );
}

export interface GrantInput {
  level: number;
  hours?: number;
  budget_cents?: number;
  hosts?: string[];
  max_parallel?: number;
  /** A credential profile on the run's host; omitted = the host's own. */
  profile?: string;
}

export function grantMission(missionId: number, g: GrantInput): Promise<Result<MissionGrant>> {
  return invokeCmd<MissionGrant>('grant_mission', { args: { mission_id: missionId, ...g } });
}

export function revokeMissionGrant(missionId: number): Promise<Result<number>> {
  return invokeCmd<number>('revoke_mission_grant', { args: { mission_id: missionId } });
}

/** Pause every active mission this person may change, and end their grants. */
export function pauseAllMissions(): Promise<Result<number[]>> {
  return changed(invokeCmd<number[]>('pause_all_missions', { args: {} }));
}

// ── Errors in words (redesign step 1.3) ──
//
// A planner that could not run, or whose answer was refused, says what
// happened and what to do in plain words, with Retry; the raw code and
// message stay one click away under Details. No settings key reaches the
// user's text: the backend's "(orchestrator.max_level)"-style hints are for
// the log, and Details still carries them.

/** A failure as the Missions view shows it. */
export interface HumanError {
  /** One bold line: what happened. */
  title: string;
  /** What it means and what to do next. */
  text: string;
  /** The raw code and message, for Details. */
  details: string;
}

/** Text with any parenthesised settings key, like "(orchestrator.enabled)"
 *  or "(policy.max_planner_runs_per_hour)", taken out. */
export function withoutConfigKeys(text: string): string {
  return text
    .replace(/\s*\((?:[a-z][a-z0-9_]*\.)+[a-z][a-z0-9_]*\)/g, '')
    .replace(/\s{2,}/g, ' ')
    .trim();
}

const capital = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);
const sentence = (s: string) => {
  const t = capital(withoutConfigKeys(s));
  return t && !/[.!?]$/.test(t) ? `${t}.` : t;
};

/** `plan_mission` failed: why the planner could not run, in words. */
export function plannerError(e: { code: string; message: string }): HumanError {
  const details = `${e.code} · ${e.message}`;
  const title = "The planner couldn't run";
  const m = e.message;
  let hit: RegExpMatchArray | null;
  if (e.code === 'E_LIMIT' && (hit = m.match(/ran (\d+) times? in the last hour/))) {
    return {
      title,
      text: `It already ran ${hit[1]} times in the last hour, the most this mission allows. Try again later.`,
      details,
    };
  }
  if ((hit = m.match(/^Claude login expired on (.+?): run `(.+?)` there/))) {
    return {
      title: "The planner's Claude login has expired",
      text: `Claude Code on ${hit[1]} is signed out, so the planner can't run. Run ${hit[2]} there, then retry.`,
      details,
    };
  }
  if ((hit = m.match(/^claude is not on (.+?)'s PATH/))) {
    return { title, text: `Claude Code isn't installed on ${hit[1]}, so the planner has nowhere to run.`, details };
  }
  if ((hit = m.match(/^the planner on (.+?) gave no answer/))) {
    return { title, text: `The planner on ${hit[1]} finished without an answer. Retry, or look at Details.`, details };
  }
  if (e.code === 'E_SSH' || e.code === 'E_SSH_TIMEOUT' || e.code === 'E_HOST_OFFLINE') {
    return { title, text: "Fleet couldn't reach the planner's host. Check that it is online, then retry.", details };
  }
  if (e.code === 'E_HUB_UNREACHABLE' || e.code === 'E_HUB_TIMEOUT') {
    return { title, text: "The hub didn't answer. Your missions are unchanged; retry when it is back.", details };
  }
  if (e.code === 'E_INVALID_STATE' && /\bis (completed|failed|cancelled)$/.test(m)) {
    return { title, text: 'This mission has ended, so there is nothing left to plan.', details };
  }
  return { title, text: sentence(m) || 'Something went wrong. Retry, or look at Details.', details };
}

/** Claude Code's own words for a run with no usable login. */
const SIGNED_OUT = /login expired|run \/login|invalid api key|not logged in|oauth token has (?:expired|been revoked)/i;

/** The planner ran but its answer could not be used (`PlanOutcome.refused`). */
export function plannerRefusal(why: string): HumanError {
  // A hub before 0.6.1 hands Claude Code's own "Login expired · Run /login"
  // back as a refused answer; it is a signed-out host, not a bad answer.
  if (SIGNED_OUT.test(why)) {
    return {
      title: "The planner's Claude login has expired",
      text: "Claude Code on the planner's host is signed out. Run claude /login there, then retry.",
      details: why,
    };
  }
  return {
    title: "The planner's answer couldn't be used",
    text: 'Nothing was changed. Retry to ask again; Details shows what was wrong with the answer.',
    details: why,
  };
}

// ── Autonomy in words (redesign step 1.8) ──

/** The header's autonomy: what applies, what bounds it, and the one hint
 *  that says how to change it, all without a settings key. */
export interface AutonomyWords {
  /** "Runs at L1". */
  runs: string;
  /** "L3 asked · L1 ceiling · no grant". */
  limits: string;
  /** What holds it back and where to change that, or `null`. */
  hint: string | null;
}

export function autonomyWords(a: MissionAutonomy, now = Date.now() / 1000): AutonomyWords {
  const g = a.grant && (a.grant.revoked_at == null && a.grant.expires_at > now) ? a.grant : null;
  const until = g ? new Date(g.expires_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '';
  const parts = [`L${a.asked} asked`, `L${a.ceiling} ceiling`, g ? `L${g.level} grant until ${until}` : 'no grant'];
  let hint: string | null = null;
  if (!a.enabled) hint = 'The mission loop is off for the whole fleet. Turn it on in Settings.';
  else if (a.effective >= a.asked) hint = null;
  else if (a.ceiling < a.asked && a.effective === a.ceiling) hint = `The fleet's ceiling holds it at L${a.ceiling}. Raise it in Settings.`;
  else if (!g) hint = 'Without a grant a person presses every step. Grant… lets it run by itself.';
  else hint = `The grant signs L${g.level}. End it and grant again to change that.`;
  return { runs: a.enabled ? `Runs at L${a.effective}` : 'Loop off', limits: parts.join(' · '), hint };
}

// ── The Missions board (redesign gap G3.6) ──
//
// The list grouped by state with a reason line per row, the detail's owner,
// planner and brakes lines, and its Runs tab. All of it is read from what
// `work_missions` and `work_mission` already answer; nothing here asks the
// hub for more.

/** One group of the list. */
export interface MissionGroup {
  key: 'active' | 'paused' | 'draft' | 'finished';
  label: string;
  missions: Mission[];
}

const GROUPS: readonly { key: MissionGroup['key']; label: string }[] = [
  { key: 'active', label: 'Active' },
  { key: 'paused', label: 'Paused' },
  { key: 'draft', label: 'Drafts' },
  { key: 'finished', label: 'Completed' },
];

function groupOf(state: string): MissionGroup['key'] {
  if (state === 'active' || state === 'paused' || state === 'draft') return state;
  return 'finished';
}

/** The list by state: Active, Paused, Drafts, then Completed (every mission
 *  that ended), each newest change first. Empty groups are left out. */
export function missionGroups(missions: readonly Mission[]): MissionGroup[] {
  const sorted = [...missions].sort((a, b) => b.updated_at - a.updated_at || b.id - a.id);
  return GROUPS.map((g) => ({ ...g, missions: sorted.filter((m) => groupOf(m.state) === g.key) })).filter(
    (g) => g.missions.length > 0,
  );
}

/** The missions whose row reads its detail for a reason line: the ones
 *  still going, newest change first, at most `cap`. */
export function missionsToExplain(missions: readonly Mission[], cap = 12): Mission[] {
  return missions
    .filter((m) => !isFinal(m.state))
    .sort((a, b) => b.updated_at - a.updated_at || b.id - a.id)
    .slice(0, cap);
}

/** A span in words: "45 min", "2 h", "1 h 30 min". */
export function durationWords(secs: number): string {
  const mins = Math.max(1, Math.round(secs / 60));
  if (mins < 60) return `${mins} min`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

/** The first wave that still has work in it, or `null` when none does. */
export function currentWave(detail: MissionDetail): number | null {
  for (const w of wavesOf(detail)) {
    if (w.nodes.some((n) => n.state !== 'done' && n.state !== 'rejected')) return w.wave;
  }
  return null;
}

/** `no_progress_secs`' default (`MissionPolicy::default`). */
export const POLICY_DEFAULT_NO_PROGRESS_SECS = 3600;
/** `max_planner_runs_per_hour`'s default. */
export const POLICY_DEFAULT_PLANNER_RUNS = 6;

/** The brake that paused it, when the loop's brake is the latest word on
 *  its state: "no progress in 2 h", "budget spent". */
export function brakeReason(detail: MissionDetail): string | null {
  const last = (detail.events ?? []).find((e) => e.kind === 'state' || e.kind === 'budget' || e.kind === 'no_progress');
  if (!last || last.kind === 'state') return null;
  if (last.kind === 'budget') return 'budget spent';
  const secs = detail.mission.policy?.no_progress_secs ?? POLICY_DEFAULT_NO_PROGRESS_SECS;
  return `no progress in ${durationWords(secs)}`;
}

/** Tasks the planner proposed that no one has accepted yet: proposals among
 *  the members, else the tasks its open create cards would add. */
export function proposedCount(detail: MissionDetail): number {
  const items = openProposals(detail).length;
  if (items > 0) return items;
  return (detail.plan?.cards ?? [])
    .filter((c) => c.state === 'open' && c.kind === 'create')
    .reduce((n, c) => n + (Array.isArray(c.payload?.tree) ? (c.payload.tree as unknown[]).length : 0), 0);
}

const PHASE_WORD: Record<string, string> = {
  running: 'Working',
  waiting: 'Idle · waiting',
  blocked: 'Failed · blocked',
};

/** A list row's second line: why the mission is where it is.
 *  "Needs you · 2 cards · wave 3", "Brake: no progress in 2 h",
 *  "Planner proposed 6 tasks · not accepted", "Working · wave 2 · continuous".
 *  `null` when there is nothing to add to the group's name. */
export function missionReason(m: Mission, detail?: MissionDetail | null): string | null {
  if (isFinal(m.state)) return m.state === 'completed' ? null : stateLabel(m.state);
  if (!detail) return null;
  const wave = currentWave(detail);
  const waveText = wave != null ? ` · wave ${wave}` : '';
  if (m.state === 'paused') {
    const brake = brakeReason(detail);
    if (brake) return `Brake: ${brake}`;
  }
  const proposed = proposedCount(detail);
  if (m.state === 'draft') return proposed > 0 ? `Planner proposed ${proposed} task${proposed === 1 ? '' : 's'} · not accepted` : null;
  if (waitsOnPerson(detail)) {
    const cards = (detail.plan?.cards ?? []).filter((c) => c.state === 'open').length;
    const what = cards > 0 ? `${cards} card${cards === 1 ? '' : 's'}` : 'an answer';
    return `Needs you · ${what}${waveText}`;
  }
  if (proposed > 0) return `Planner proposed ${proposed} task${proposed === 1 ? '' : 's'} · not accepted`;
  if (m.state !== 'active') return null;
  const phase = detail.phase ? (PHASE_WORD[detail.phase] ?? detail.phase) : null;
  if (!phase) return null;
  return `${phase}${waveText}${m.mode === 'continuous' ? ' · continuous' : ''}`;
}

/** The list footer: the loop's switch and the fleet's ceiling, read from any
 *  open mission's plan (both are fleet-wide). `null` without one. */
export function loopFooter(details: readonly (MissionDetail | null | undefined)[]): string | null {
  const a = details.find((d) => d?.plan?.autonomy)?.plan?.autonomy;
  if (!a) return null;
  return `Mission loop ${a.enabled ? 'on' : 'off'} · ceiling L${a.ceiling}`;
}

/** Planner runs in the hour before `now` (the planner's own limit counts
 *  `planned` events the same way). The events are the newest page, so the
 *  count is exact unless that page ends inside the hour. */
export function plannerRunsThisHour(events: readonly MissionEvent[], now: number): number {
  return events.filter((e) => e.kind === 'planned' && e.at >= now - 3600).length;
}

/** The Planner line's value: "2 of 6 runs this hour". */
export function plannerLine(detail: MissionDetail, now = Math.floor(Date.now() / 1000)): string {
  const max = detail.mission.policy?.max_planner_runs_per_hour ?? POLICY_DEFAULT_PLANNER_RUNS;
  return `${plannerRunsThisHour(detail.events ?? [], now)} of ${max} runs this hour`;
}

/** "Pause on spent budget or 2 h without progress"; without a budget on
 *  the grant, only the time. */
export function brakesLine(policy: MissionPolicy | undefined, budgetMicros?: number | null): string {
  const time = `${durationWords(policy?.no_progress_secs ?? POLICY_DEFAULT_NO_PROGRESS_SECS)} without progress`;
  return budgetMicros ? `Pause on spent budget (${dollars(budgetMicros)}) or ${time}` : `Pause after ${time}`;
}

/** "You · 32bit", "Ana · 32bit", "person 7"; `null` when the mission has
 *  neither an owner nor an org. */
export function ownerLine(
  m: Pick<Mission, 'owner_person_id' | 'org_id'>,
  me: number | null,
  nameOf: (personId: number) => string | null,
  orgName: (orgId: number) => string | null,
): string | null {
  const id = m.owner_person_id;
  const who = id == null ? null : id === me ? 'You' : (nameOf(id) ?? `person ${id}`);
  const org = m.org_id != null ? orgName(m.org_id) : null;
  const parts = [who, org].filter((p): p is string => !!p);
  return parts.length ? parts.join(' · ') : null;
}

/** One run of a mission: a worker the loop or a person started on a task. */
export interface MissionRun {
  task_id: number;
  item_id: number | null;
  /** implement, review, test, … or the step kind. */
  role: string;
  attempt: number | null;
  /** The attempt's state when it is a task's latest; `started` when only
   *  the log knows it (an earlier attempt). */
  state: string;
  /** When it was started, when the log still holds that. */
  at: number | null;
  /** Who started it: `loop`, `person:3`. */
  actor: string | null;
}

/** The runs the mission's log and graph know of, newest first: every step
 *  that started a task, with each task's latest attempt for its state. */
export function runsOf(detail: MissionDetail): MissionRun[] {
  const by = new Map<number, MissionRun>();
  for (const e of detail.events ?? []) {
    if (e.kind !== 'step') continue;
    const p = (e.payload ?? {}) as Record<string, unknown>;
    const id = typeof p.task_id === 'number' ? p.task_id : null;
    if (id == null || by.has(id)) continue;
    by.set(id, {
      task_id: id,
      item_id: e.work_item_id ?? null,
      role: String(p.role ?? p.step ?? 'run'),
      attempt: null,
      state: 'started',
      at: e.at,
      actor: e.actor,
    });
  }
  for (const n of detail.graph?.nodes ?? []) {
    const a = n.attempt;
    if (!a) continue;
    const seen = by.get(a.task_id);
    by.set(a.task_id, {
      task_id: a.task_id,
      item_id: n.item_id,
      role: a.role ?? seen?.role ?? 'run',
      attempt: a.attempt ?? null,
      state: a.state,
      at: seen?.at ?? null,
      actor: seen?.actor ?? null,
    });
  }
  return [...by.values()].sort((a, b) => b.task_id - a.task_id);
}

/** A run's state as one of the six words, with the raw state after " · "
 *  when the word alone hides it. */
export function runStateLabel(state: string): string {
  switch (state) {
    case 'running':
      return 'Working';
    case 'queued':
      return 'Working · queued';
    case 'started':
      // Only the log knows it: an earlier attempt, since retried.
      return 'Idle · earlier run';
    case 'done':
      return 'Done';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Idle · cancelled';
    default:
      return `Idle · ${state}`;
  }
}
