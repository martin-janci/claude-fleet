// Gap plan G3.9: Control's slash commands, run by the desktop itself.
//
// `/task`, `/done`, `/assign` and `/start` each map onto ONE backend action
// the Work view already uses (`create_work_task` + `edit_work_item`,
// `set_work_status`, `link_session_work`, `preview_start_work` +
// `start_work`), so typing them in Control does exactly what the matching
// button does, at once and without a model in between. The person typed the
// command and pressed Enter: that is the decision, so nothing here is a
// proposal. `/plan` is NOT run here: planning subtasks is the agent's work
// (`work_link` `propose_tree`, accepted on a card), so it goes to Control's
// agent as its project command does (`CONTROL_COMMANDS` in
// crates/fleet-core/src/service/operator.rs). The phone sends every one of
// them to the agent, which applies the same grammar from those bodies.
//
// Grammar (the menu's `usage`, `control_commands.ts`):
//   /task <title> [due:<day>] [@owner]
//   /done #KEY
//   /assign #KEY @session
//   /start #KEY [@host]
//   /health [@host]          (Thread room: host health from the host rows)
//   /graph [#KEY | mission]  (Thread room: a mission's task graph card)
// `/health` and `/graph` only read: the desktop answers from what it holds,
// so they cost no model turn. `/tidy` goes to the agent, which shows the
// batch and waits for a go. The phone sends all three to the agent, whose
// commands (`CONTROL_COMMANDS`) answer the same questions.
//   /debug [@host] [30m|6h|3d] [part:<component>] [session:<name>] <focus>
// `/debug` queues the self debug agent (the active `self-debug` fleet agent)
// with the focus as its request and the rest as its scope, then follows the
// run and shows its findings, worst first, when it is done.
//   /propose <focus>
// `/propose` runs the proposer (the active `proposer` fleet agent) now, with
// the focus as its request, and shows what it proposed; each proposal waits
// in the Inbox for Approve.
// `#KEY` is a task key (`TASK-219`, `PD-2592`); the `#` is optional.
import { get, writable } from 'svelte/store';
import type { Result } from './result';
import { createWorkTask, editWorkItem, linkSessionWork, setWorkStatus, type WorkItemStatus } from './work';
import { workTask, showTaskInWorkView } from './work_view';
import { previewStartWork, previewIsClean, startFromPreview, baseStartArgs } from './start_preview';
import { sessions, type SessionRow } from './sessions';
import { sessionActionBlocked } from './share';
import { hubActionBlocked } from './hub';
import { hosts } from './hosts';
import { selectView } from './control_views';
import { openMissionInControl } from './control_room';
import { fleetLoads, loadParts, type LoadLevel } from './host_health';
import { fleetSettings, settingInt, SETTING_KEYS } from './fleet_settings';
import { getMission, listMissions, type Mission } from './missions';
import { graphFromMission, graphProgress, type GraphCard } from './task_graph_card';
import { controlVersions, versionsSummary } from './control_versions';
import { agentsStatus, runFleetAgent, SCOPE_COMPONENTS, type AgentJob, type RunScope } from './agents';
import { openRuns } from './automation';
import { errorText } from './error_copy';

export type ControlCommand =
  | { cmd: 'task'; title: string; due?: string; owner?: string }
  | { cmd: 'done'; key: string }
  | { cmd: 'assign'; key: string; session: string }
  | { cmd: 'start'; key: string; host?: string }
  | { cmd: 'health'; host?: string }
  | { cmd: 'graph'; target?: string }
  | { cmd: 'debug'; request?: string; scope?: RunScope }
  | { cmd: 'propose'; request?: string };

/** What a typed line is to Control: one of its commands, a command of its
 *  own typed wrong (`usage` says how), or not one it runs (`null`: `/plan`,
 *  a built-in like `/clear`, a plain prompt), which goes to the agent. */
export type ParsedCommand = { ok: true; command: ControlCommand } | { ok: false; usage: string } | null;

export const USAGE: Record<ControlCommand['cmd'], string> = {
  task: '/task <title> [due:<day>] [@owner]',
  done: '/done #KEY',
  assign: '/assign #KEY @session',
  start: '/start #KEY [@host]',
  health: '/health [@host]',
  graph: '/graph [#KEY or a mission name]',
  debug: '/debug [@host] [30m|6h|3d] [part:<component>] [session:<name>] <focus>',
  propose: '/propose <focus>',
};

const KEY = /^#?([A-Za-z][A-Za-z0-9_]*-\d+)$/;
const WEEKDAYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];

function ymd(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** `due:` as a `YYYY-MM-DD` in `now`'s local calendar: `today`,
 *  `tomorrow`, a weekday (the coming one; today's name is today), or a
 *  date. `null` when it does not parse. */
export function parseDue(raw: string, now: Date = new Date()): string | null {
  const v = raw.trim().toLowerCase();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const plus = (n: number) => ymd(new Date(today.getFullYear(), today.getMonth(), today.getDate() + n));
  if (v === 'today') return plus(0);
  if (v === 'tomorrow') return plus(1);
  const wd = WEEKDAYS.findIndex((w) => w === v || w.slice(0, 3) === v);
  if (wd >= 0) return plus((wd - today.getDay() + 7) % 7);
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v);
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return d.getMonth() === Number(m[2]) - 1 ? ymd(d) : null;
}

function keyOf(tok: string | undefined): string | null {
  const m = KEY.exec(tok ?? '');
  return m ? m[1].toUpperCase() : null;
}

/** Read a line typed in Control. Pure; `now` dates `due:`. */
export function parseControlCommand(text: string, now: Date = new Date()): ParsedCommand {
  const m = /^\/([a-z]+)(?:\s+([\s\S]*))?$/.exec(text.trim());
  if (!m) return null;
  const name = m[1];
  const toks = (m[2] ?? '').split(/\s+/).filter(Boolean);
  const bad = (cmd: ControlCommand['cmd']): ParsedCommand => ({ ok: false, usage: USAGE[cmd] });
  switch (name) {
    case 'task': {
      let due: string | undefined;
      let owner: string | undefined;
      const words: string[] = [];
      for (const t of toks) {
        if (/^due:/i.test(t)) {
          const d = parseDue(t.slice(4), now);
          if (d === null || due !== undefined) return bad('task');
          due = d;
        } else if (t.startsWith('@') && t.length > 1 && owner === undefined) {
          owner = t.slice(1);
        } else words.push(t);
      }
      const title = words.join(' ');
      if (!title) return bad('task');
      return { ok: true, command: { cmd: 'task', title, ...(due ? { due } : {}), ...(owner ? { owner } : {}) } };
    }
    case 'done': {
      const key = keyOf(toks[0]);
      return key && toks.length === 1 ? { ok: true, command: { cmd: 'done', key } } : bad('done');
    }
    case 'assign': {
      const key = keyOf(toks.find((t) => !t.startsWith('@')));
      const at = toks.filter((t) => t.startsWith('@') && t.length > 1);
      if (!key || at.length !== 1 || toks.length !== 2) return bad('assign');
      return { ok: true, command: { cmd: 'assign', key, session: at[0].slice(1) } };
    }
    case 'start': {
      const key = keyOf(toks.find((t) => !t.startsWith('@')));
      const at = toks.filter((t) => t.startsWith('@') && t.length > 1);
      if (!key || at.length > 1 || toks.length !== 1 + at.length) return bad('start');
      return { ok: true, command: { cmd: 'start', key, ...(at[0] ? { host: at[0].slice(1) } : {}) } };
    }
    case 'health': {
      if (toks.length > 1 || (toks[0] !== undefined && !(toks[0].startsWith('@') && toks[0].length > 1))) return bad('health');
      return { ok: true, command: { cmd: 'health', ...(toks[0] ? { host: toks[0].slice(1) } : {}) } };
    }
    case 'graph': {
      const target = toks.join(' ');
      return { ok: true, command: { cmd: 'graph', ...(target ? { target } : {}) } };
    }
    case 'debug': {
      // The scope is read from the leading words only: once a plain word
      // starts the request, `2h` in "why does the 2h backup fail" is text.
      const scope: RunScope = {};
      const words: string[] = [];
      for (const t of toks) {
        const span = /^(\d+)([mhd])$/i.exec(t);
        if (words.length > 0) words.push(t);
        else if (t.startsWith('@') && t.length > 1 && scope.host === undefined) scope.host = t.slice(1);
        else if (span && scope.minutes === undefined) {
          const n = Number(span[1]) * ({ m: 1, h: 60, d: 1440 } as Record<string, number>)[span[2].toLowerCase()];
          if (n < 1 || n > DEBUG_MAX_MINUTES) return bad('debug');
          scope.minutes = n;
        } else if (/^part:/i.test(t)) {
          const c = t.slice(5).toLowerCase();
          if (!(SCOPE_COMPONENTS as readonly string[]).includes(c) || scope.component !== undefined) return bad('debug');
          scope.component = c;
        } else if (/^session:./i.test(t) && scope.session === undefined) scope.session = t.slice(8);
        else words.push(t);
      }
      const request = words.join(' ');
      return {
        ok: true,
        command: { cmd: 'debug', ...(request ? { request } : {}), ...(Object.keys(scope).length > 0 ? { scope } : {}) },
      };
    }
    case 'propose': {
      const request = toks.join(' ');
      return { ok: true, command: { cmd: 'propose', ...(request ? { request } : {}) } };
    }
    default:
      return null;
  }
}

/** The longest window `/debug` takes: a week, as the hub allows. */
export const DEBUG_MAX_MINUTES = 7 * 24 * 60;

/** What a run left to show under the composer. */
export interface CommandReceipt {
  id: number;
  /** The line as typed. */
  typed: string;
  state: 'running' | 'done' | 'failed';
  /** One sentence: what happened, or why not. */
  line: string;
  /** Where the result lives: a task in Work, a session, Control's Hosts view or a mission. */
  open?:
    | { kind: 'task'; taskId: string }
    | { kind: 'session'; id: number; label: string }
    | { kind: 'hosts' }
    | { kind: 'mission'; id: number }
    | { kind: 'runs' };
  /** `/health`: one line per host, worst first; `/debug`: one per finding;
   *  `/propose`: one per proposal. */
  rows?: { label: string; text: string; level: LoadLevel }[];
  /** `/graph`: the mission's task graph, drawn as the chat's graph card. */
  graph?: GraphCard;
  /** Puts back what the command changed, when it can be put back. */
  undo?: () => Promise<Result<unknown>>;
}

export const commandReceipts = writable<CommandReceipt[]>([]);
export const MAX_COMMAND_RECEIPTS = 3;
let nextId = 1;

function put(r: CommandReceipt): void {
  commandReceipts.update((rs) => [...rs.filter((x) => x.id !== r.id), r].slice(-MAX_COMMAND_RECEIPTS));
}

export function dismissCommandReceipt(id: number): void {
  commandReceipts.update((rs) => rs.filter((x) => x.id !== id));
}

/** Run Undo on receipt `id`; the receipt then says it was undone. */
export async function undoCommand(id: number): Promise<void> {
  const r = get(commandReceipts).find((x) => x.id === id);
  if (!r?.undo) return;
  const undo = r.undo;
  put({ ...r, undo: undefined, line: `${r.line} Undoing…` });
  const out = await undo();
  put({ ...r, undo: undefined, line: out.ok ? 'Undone.' : `Undo failed: ${errorText(out.error)}` });
}

function sessionLabel(s: SessionRow): string {
  return s.friendly_name || s.tmux_name;
}

/** The live sessions `name` means: by its shown name or its tmux name. */
export function sessionsNamed(name: string, rows: readonly SessionRow[]): SessionRow[] {
  const want = name.toLowerCase();
  return rows.filter(
    (s) => s.lost_at == null && (s.tmux_name.toLowerCase() === want || (s.friendly_name ?? '').toLowerCase() === want),
  );
}

async function resolveItem(key: string): Promise<{ ok: true; itemId: number; status: string; title: string } | { ok: false; line: string }> {
  const r = await workTask(`ref:${key}`);
  if (!r.ok) return { ok: false, line: r.error.code === 'E_NOTFOUND' ? `No task ${key}.` : errorText(r.error) };
  const t = r.value.task;
  if (t.item_id == null) return { ok: false, line: `${key} is not a task Fleet can change here.` };
  return { ok: true, itemId: t.item_id, status: t.status_category ?? 'todo', title: t.title ?? key };
}

function health(host: string | undefined): Omit<CommandReceipt, 'id' | 'typed'> {
  const settings = get(fleetSettings);
  const now = Math.floor(Date.now() / 1000);
  const all = fleetLoads(get(hosts), host ?? null, now, {
    diskLowPct: settingInt(settings, SETTING_KEYS.healthDiskLowPct),
    memLowPct: settingInt(settings, SETTING_KEYS.healthMemLowPct),
    cpuBusyPct: settingInt(settings, SETTING_KEYS.healthCpuBusyPct),
  });
  const loads = host ? all.filter((l) => l.alias.toLowerCase() === host.toLowerCase()) : all;
  if (loads.length === 0) return { state: 'failed', line: host ? `No host named ${host}.` : 'No hosts yet.' };
  const rank = (l: LoadLevel) => (l === 'crit' ? 0 : l === 'warn' ? 1 : 2);
  const rows = [...loads]
    .sort((a, b) => rank(a.level) - rank(b.level) || a.alias.localeCompare(b.alias))
    .map((l) => ({ label: l.alias, text: loadParts(l).join(' · '), level: l.level }));
  const trouble = loads.filter((l) => l.trouble);
  const line =
    trouble.length === 0
      ? loads.length === 1
        ? `${loads[0].alias} is fine.`
        : `All ${loads.length} hosts are fine.`
      : trouble.map((l) => `${l.alias}: ${l.trouble}`).join(' · ');
  // F0.2: the versions Control runs against, last, amber when the hub is
  // older than this desktop.
  const v = versionsSummary(get(controlVersions));
  if (v) rows.push({ label: 'versions', text: v.text, level: v.hubBehind ? 'warn' : 'ok' });
  return { state: 'done', line, rows, open: { kind: 'hosts' } };
}

/** The mission `/graph` means: by its root task's key, by name, or the only active one. */
async function missionFor(target: string | undefined): Promise<{ ok: true; mission: Mission } | { ok: false; line: string }> {
  const list = await listMissions();
  if (!list.ok) return { ok: false, line: errorText(list.error) };
  const missions = list.value;
  if (target === undefined) {
    const active = missions.filter((m) => m.state === 'active');
    if (active.length === 1) return { ok: true, mission: active[0] };
    if (active.length === 0) return { ok: false, line: 'No active mission. Name one: /graph <mission>.' };
    return { ok: false, line: `${active.length} missions are active: name one (${active.map((m) => m.name).join(', ')}).` };
  }
  const key = keyOf(target);
  if (key) {
    const it = await resolveItem(key);
    if (!it.ok) return { ok: false, line: it.line };
    const m = missions.find((x) => x.root_item_id === it.itemId);
    return m ? { ok: true, mission: m } : { ok: false, line: `${key} is not a mission's task.` };
  }
  const want = target.toLowerCase();
  const exact = missions.filter((m) => m.name.toLowerCase() === want);
  const near = exact.length > 0 ? exact : missions.filter((m) => m.name.toLowerCase().includes(want));
  if (near.length === 1) return { ok: true, mission: near[0] };
  if (near.length === 0) return { ok: false, line: `No mission named ${target}.` };
  return { ok: false, line: `${near.length} missions match ${target}: ${near.map((m) => m.name).join(', ')}.` };
}

async function graph(target: string | undefined): Promise<Omit<CommandReceipt, 'id' | 'typed'>> {
  const found = await missionFor(target);
  if (!found.ok) return { state: 'failed', line: found.line };
  const m = found.mission;
  const open = { kind: 'mission' as const, id: m.id };
  const d = await getMission(m.id);
  if (!d.ok) return { state: 'failed', line: errorText(d.error), open };
  const g = graphFromMission(d.value);
  if (!g) return { state: 'done', line: `${m.name} has no tasks yet.`, open };
  return { state: 'done', line: `${m.name} · ${graphProgress(g)}`, open, graph: g };
}

type Outcome = Omit<CommandReceipt, 'id' | 'typed'>;

/** How often `/debug` and `/propose` look at their run, and for how long (the agent's own
 *  timeout plus a pass to pick it up). Test seam. */
export const debugPoll = { everyMs: 10_000, forMs: 20 * 60_000 };

/** One entry of a fleet job's `result.actions` (runner `queue_actions`). */
interface QueuedAction {
  do?: unknown;
  state?: unknown;
  why?: unknown;
}

/** What became of a run's actions, as one sentence: how many wait in the
 *  Inbox, run on their own, were kept (the rule is at watch) or were
 *  skipped and why. An agent's own memory (`record`) is not counted. Null
 *  when the result names no actions. */
export function queuedTail(result: string | null): string | null {
  let actions: QueuedAction[] = [];
  try {
    const r = JSON.parse(result ?? '{}') as { actions?: unknown };
    if (!Array.isArray(r.actions)) return null;
    actions = r.actions as QueuedAction[];
  } catch {
    return null;
  }
  const real = actions.filter((a) => a.do !== 'record');
  const count = (state: string) => real.filter((a) => a.state === state).length;
  const n = (k: number, one: string, many: string) => (k === 1 ? `1 ${one}` : `${k} ${many}`);
  const parts: string[] = [];
  const pending = count('pending');
  if (pending > 0) parts.push(`${n(pending, 'proposal waits', 'proposals wait')} in the Inbox.`);
  const approved = count('approved');
  if (approved > 0) parts.push(`${approved === 1 ? '1 goes ahead on its own' : `${approved} go ahead on their own`} (the rule is at act).`);
  const kept = count('kept');
  if (kept > 0) parts.push(`${n(kept, 'proposal', 'proposals')} kept, not queued: the rule is at watch.`);
  const skipped = real.filter((a) => a.state === 'skipped' || (typeof a.state === 'string' && a.state.startsWith('not queued')));
  if (skipped.length > 0) {
    const whys = [...new Set(skipped.map((a) => (typeof a.why === 'string' && a.why ? a.why : String(a.state))))];
    parts.push(`Skipped ${skipped.length}: ${whys.join('; ')}.`);
  }
  return parts.length > 0 ? parts.join(' ') : null;
}

interface Finding {
  title?: unknown;
  severity?: unknown;
  component?: unknown;
  cause?: unknown;
}

function findingLevel(sev: unknown): LoadLevel {
  // The self debug agent says critical | major | minor; other agents may say high | medium.
  return sev === 'critical' || sev === 'high' ? 'crit' : sev === 'major' || sev === 'medium' ? 'warn' : 'ok';
}

/** A finished self-debug job as a receipt: its summary, then a row per finding. */
export function debugOutcome(job: Pick<AgentJob, 'state' | 'error' | 'result'>): Outcome {
  const open = { kind: 'runs' as const };
  if (job.state !== 'done') {
    return { state: 'failed', line: `The self debug run ${job.state.replace('_', ' ')}${job.error ? `: ${job.error}` : '.'}`, open };
  }
  let answer: { summary?: unknown; findings?: unknown } = {};
  try {
    answer = (JSON.parse(job.result ?? '{}') as { answer?: typeof answer }).answer ?? {};
  } catch {
    // An unreadable result still finished: say so below.
  }
  const findings = Array.isArray(answer.findings) ? (answer.findings as Finding[]) : [];
  const rank = (l: LoadLevel) => (l === 'crit' ? 0 : l === 'warn' ? 1 : 2);
  const rows = findings
    .map((f) => ({
      label: typeof f.component === 'string' ? f.component : 'fleet',
      text: [f.title, f.cause].filter((x) => typeof x === 'string' && x).join(' · '),
      level: findingLevel(f.severity),
    }))
    .sort((a, b) => rank(a.level) - rank(b.level));
  const summary = typeof answer.summary === 'string' && answer.summary ? answer.summary : null;
  const said = summary ?? (rows.length === 0 ? 'No problems found.' : `${rows.length} findings.`);
  const tail = queuedTail(job.result);
  const line = tail ? `${said} ${tail}` : said;
  return { state: 'done', line, open, ...(rows.length > 0 ? { rows } : {}) };
}

interface Proposal {
  title?: unknown;
  kind?: unknown;
  severity?: unknown;
  why?: unknown;
}

/** A finished proposer job as a receipt: its summary, then a row per proposal. */
export function proposeOutcome(job: Pick<AgentJob, 'state' | 'error' | 'result'>): Outcome {
  const open = { kind: 'runs' as const };
  if (job.state !== 'done') {
    return { state: 'failed', line: `The proposer run ${job.state.replace('_', ' ')}${job.error ? `: ${job.error}` : '.'}`, open };
  }
  let answer: { summary?: unknown; proposals?: unknown } = {};
  try {
    answer = (JSON.parse(job.result ?? '{}') as { answer?: typeof answer }).answer ?? {};
  } catch {
    // An unreadable result still finished: say so below.
  }
  const proposals = Array.isArray(answer.proposals) ? (answer.proposals as Proposal[]) : [];
  const rows = proposals.map((p) => ({
    label: typeof p.kind === 'string' ? p.kind : 'proposal',
    text: [p.title, p.why].filter((x) => typeof x === 'string' && x).join(' · '),
    level: findingLevel(p.severity),
  }));
  const summary = typeof answer.summary === 'string' && answer.summary ? answer.summary : null;
  // What was queued, from the run's own record of it (a rule at watch keeps
  // them, a setting no agent may change is skipped), not the answer's count.
  const tail = queuedTail(job.result);
  const waits =
    rows.length === 0
      ? 'Nothing to propose.'
      : (tail ?? (rows.length === 1 ? '1 proposal waits in the Inbox.' : `${rows.length} proposals wait in the Inbox.`));
  return { state: 'done', line: summary ? `${summary} ${rows.length > 0 ? waits : ''}`.trim() : waits, open, ...(rows.length > 0 ? { rows } : {}) };
}

/** Run one fleet agent now and follow it until it answers. */
async function follow(
  c: { definition: string; name: string; doing: string; request?: string; scope?: RunScope },
  outcome: (job: AgentJob) => Outcome,
  update: (o: Outcome) => void,
): Promise<Outcome> {
  const st = await agentsStatus('fleet');
  if (!st.ok) return { state: 'failed', line: errorText(st.error) };
  const rule = st.value.rules.find((r) => r.active && r.definition === c.definition);
  if (!rule) return { state: 'failed', line: `No ${c.name} yet: add one under Automation, Agents.` };
  const q = await runFleetAgent(rule.id, c.request, c.scope);
  if (!q.ok) return { state: 'failed', line: errorText(q.error) };
  const key = q.value.job;
  const open = { kind: 'runs' as const };
  update({ state: 'running', line: `Queued: the ${c.name} is ${c.doing}.`, open });
  const until = Date.now() + debugPoll.forMs;
  while (Date.now() < until) {
    await new Promise((r) => setTimeout(r, debugPoll.everyMs));
    const s = await agentsStatus('fleet');
    if (!s.ok) continue;
    const job = s.value.jobs.find((j) => j.dedup_key === key);
    if (!job || job.state === 'pending') continue;
    if (job.state === 'running') {
      update({ state: 'running', line: `The ${c.name} is ${c.doing}…`, open });
      continue;
    }
    return outcome(job);
  }
  return { state: 'done', line: 'Still running: its answer will be in Automation, Runs.', open };
}

async function run(c: ControlCommand, update: (o: Outcome) => void): Promise<Outcome> {
  switch (c.cmd) {
    case 'debug':
      return follow(
        { definition: 'self-debug', name: 'self debug agent', doing: 'reading the fleet', request: c.request, scope: c.scope },
        debugOutcome,
        update,
      );
    case 'propose':
      return follow({ definition: 'proposer', name: 'proposer', doing: 'reading what the agents found', request: c.request }, proposeOutcome, update);
    case 'health':
      return health(c.host);
    case 'graph':
      return graph(c.target);
    case 'task': {
      const r = await createWorkTask({ title: c.title });
      if (!r.ok) return { state: 'failed', line: errorText(r.error) };
      const item = r.value;
      const name = item.key ?? `#${item.id}`;
      const open = { kind: 'task' as const, taskId: `item:${item.id}` };
      if (c.due === undefined && c.owner === undefined) {
        return { state: 'done', line: `Created ${name} · ${item.title}.`, open };
      }
      const e = await editWorkItem(item.id, {
        ...(c.due !== undefined ? { due_at: c.due } : {}),
        ...(c.owner !== undefined ? { assignees: [c.owner] } : {}),
      });
      if (!e.ok) return { state: 'failed', line: `Created ${name}, but its owner and due date were not set: ${errorText(e.error)}`, open };
      const extra = [c.owner ? `owner ${c.owner}` : null, c.due ? `due ${c.due}` : null].filter(Boolean).join(', ');
      return { state: 'done', line: `Created ${name} · ${item.title} (${extra}).`, open };
    }
    case 'done': {
      const it = await resolveItem(c.key);
      if (!it.ok) return { state: 'failed', line: it.line };
      const open = { kind: 'task' as const, taskId: `item:${it.itemId}` };
      if (it.status === 'done') return { state: 'done', line: `${c.key} was already done.`, open };
      const r = await setWorkStatus(it.itemId, 'done');
      if (!r.ok) return { state: 'failed', line: errorText(r.error), open };
      const back = (it.status === 'in_progress' ? 'in_progress' : 'todo') as WorkItemStatus;
      return { state: 'done', line: `${c.key} · ${it.title} is done.`, open, undo: () => setWorkStatus(it.itemId, back) };
    }
    case 'assign': {
      const found = sessionsNamed(c.session, get(sessions));
      if (found.length === 0) return { state: 'failed', line: `No live session named ${c.session}.` };
      if (found.length > 1) return { state: 'failed', line: `${found.length} sessions are named ${c.session}: assign it from Work.` };
      const s = found[0];
      // The same gate as the session's own Link button: a session shared to
      // you below drive cannot be linked from here either.
      const blocked = hubActionBlocked('link_session_work') ?? sessionActionBlocked(s, 'link_session_work');
      if (blocked !== null) return { state: 'failed', line: blocked };
      const r = await linkSessionWork(s.id, { key: c.key });
      if (!r.ok) return { state: 'failed', line: errorText(r.error) };
      return {
        state: 'done',
        line: `${c.key} is assigned to ${sessionLabel(s)}.`,
        open: { kind: 'session', id: s.id, label: sessionLabel(s) },
      };
    }
    case 'start': {
      const base = baseStartArgs({ item_id: null, key: c.key, project_id: null });
      if (c.host) base.host_alias = c.host;
      const p = await previewStartWork(base);
      if (!p.ok) return { state: 'failed', line: errorText(p.error) };
      const preview = p.value;
      const open = { kind: 'task' as const, taskId: preview.item_id != null ? `item:${preview.item_id}` : `ref:${c.key}` };
      if (!previewIsClean(preview)) {
        const why = preview.missing ? `pick a ${preview.missing}` : (preview.conflicts[0]?.message ?? 'it needs a choice');
        return { state: 'failed', line: `${c.key} did not start: ${why}. Start it from Work.`, open };
      }
      const r = await startFromPreview(base, preview);
      if (!r.ok) return { state: 'failed', line: errorText(r.error), open };
      const s = r.value;
      return {
        state: 'done',
        line: `Started ${sessionLabel(s)} on ${s.host_alias} for ${c.key}.`,
        open: { kind: 'session', id: s.id, label: sessionLabel(s) },
      };
    }
  }
}

/**
 * Control's composer hands every slash line here first. `true`: the line
 * was Control's to run (or to refuse with its usage) and must not reach the
 * agent; `false`: send it on as typed.
 */
export function takeControlCommand(text: string, now: Date = new Date()): boolean {
  const parsed = parseControlCommand(text, now);
  if (parsed === null) return false;
  const id = nextId++;
  const typed = text.trim();
  if (!parsed.ok) {
    put({ id, typed, state: 'failed', line: `Usage: ${parsed.usage}` });
    return true;
  }
  put({ id, typed, state: 'running', line: 'Running…' });
  void run(parsed.command, (o) => put({ id, typed, ...o })).then(
    (out) => put({ id, typed, ...out }),
    (e: unknown) => put({ id, typed, state: 'failed', line: e instanceof Error ? e.message : String(e) }),
  );
  return true;
}

/** Follow a receipt's link. */
export function openCommandResult(r: CommandReceipt, focus: (id: number, label: string) => void): void {
  if (!r.open) return;
  if (r.open.kind === 'task') showTaskInWorkView(r.open.taskId);
  else if (r.open.kind === 'hosts') selectView('hosts');
  else if (r.open.kind === 'mission') openMissionInControl(r.open.id);
  else if (r.open.kind === 'runs') openRuns('list');
  else focus(r.open.id, r.open.label);
}

/** Test seam. */
export function resetCommandReceiptsForTests(): void {
  commandReceipts.set([]);
  nextId = 1;
}
