// Routines (Orbit Fleet redesign 8.6; the backend is 8.5's
// `service::routines`): a saved prompt that starts a session on a cron
// schedule, a session event or Run now. The Automation screen's Routines tab
// (RoutinesPanel) edits them; the Inbox lists the ones whose newest run
// failed, with Fix, Retry and Pause (RoutineFailures). Every call is the one
// `routines` command, which routes to the hub's `routines` tool on a paired
// desktop. Mirrors `fleet_core::store::RoutineRow` / `RoutineRunRow`.
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import type { AccountRow } from './accounts';
import { sessions } from './sessions';
import { selectSessionExplicitly } from './selection';
import { goTo } from './destination';
import { automationTab } from './automation';
import type { RunRow } from './runs';
import type { OfState } from './kit/status';

export type RoutineTrigger = 'cron' | 'event' | 'manual';
export type RoutineOverlap = 'skip' | 'parallel';

/** One routine. Optional fields are absent on the wire. */
export interface RoutineRow {
  id: number;
  org_id?: number;
  owner_person_id?: number;
  name: string;
  enabled: boolean;
  trigger: RoutineTrigger | string;
  cron?: string;
  utc_offset_min: number;
  event?: string;
  host_alias: string;
  project_id: number;
  profile?: string;
  prompt: string;
  budget_run_micros?: number;
  budget_day_micros?: number;
  overlap: RoutineOverlap | string;
  next_run_at?: number;
  skip_next: boolean;
  /** Why the scheduler turned it off (over its budget); absent for a person's switch. */
  paused_reason?: string;
  created_at: number;
  updated_at: number;
}

/** One run of a routine. */
export interface RoutineRunRow {
  id: number;
  routine_id: number;
  /** cron | event | run_now */
  trigger: string;
  trigger_ref?: string;
  /** running | done | failed | skipped */
  state: string;
  reason?: string;
  session_id?: number;
  cost_micros: number;
  scheduled_for?: number;
  started_at: number;
  finished_at?: number;
  /** did_work | nothing | failed | needs_person (8.10). */
  outcome?: string;
  outcome_source?: string;
}

/** `get`: the routine and its last runs. */
export interface RoutineDetail {
  routine: RoutineRow;
  runs: RoutineRunRow[];
  may_change: boolean;
  account?: RoutineAccount;
}

/** The account a routine's login bills (fleet-core
 *  `account_limits::LoginAccount`): the login's own fields are flattened
 *  into it, there is no nested `login`. */
export interface RoutineAccount {
  host_alias: string;
  /** `null` / absent = the host's own login; else the profile's name. */
  profile?: string | null;
  account_uuid: string;
  used_pct?: number | null;
  email?: string;
  /** At or past `accounts.pause_at`. */
  over: boolean;
}

/** Who a routine runs as: the profile's name, else the account the way
 *  `accountLabel` names it (nickname, email, then the uuid's first 8). */
export function routineAccountLabel(a: RoutineAccount, row?: Pick<AccountRow, 'nickname' | 'email'> | null): string {
  const profile = a.profile?.trim();
  if (profile) return profile;
  const nickname = row?.nickname?.trim();
  if (nickname) return nickname;
  const email = a.email?.trim() || row?.email?.trim();
  if (email) return email;
  return a.account_uuid.slice(0, 8);
}

/** `failing`: a routine whose newest run failed, for the Inbox. */
export interface FailingRoutine {
  routine: RoutineRow;
  run: RoutineRunRow;
  may_change: boolean;
}

/** What `save` writes: the whole routine. */
export interface RoutineInput {
  name: string;
  enabled?: boolean;
  trigger: RoutineTrigger;
  cron?: string;
  utc_offset_min?: number;
  event?: string;
  host_alias: string;
  project_id: number;
  profile?: string;
  prompt: string;
  budget_run_micros?: number;
  budget_day_micros?: number;
  overlap?: RoutineOverlap;
}

type Args = { action: string; routine_id?: number; routine?: RoutineInput; enabled?: boolean; skip?: boolean; limit?: number };

function call<T>(args: Args): Promise<Result<T>> {
  return invokeCmd<T>('routines', { args });
}

export const listRoutines = () => call<RoutineRow[]>({ action: 'list' });
export const getRoutine = (id: number) => call<RoutineDetail>({ action: 'get', routine_id: id });
export const failingRoutines = () => call<FailingRoutine[]>({ action: 'failing' });
export const saveRoutine = (routine: RoutineInput, id?: number) =>
  call<RoutineRow>({ action: 'save', routine, ...(id !== undefined ? { routine_id: id } : {}) });
export const deleteRoutine = (id: number) => call<{ removed: boolean }>({ action: 'delete', routine_id: id });
export const setRoutineEnabled = (id: number, enabled: boolean) =>
  call<RoutineRow>({ action: 'set_enabled', routine_id: id, enabled });
export const skipNextRun = (id: number, skip = true) => call<RoutineRow>({ action: 'skip_next', routine_id: id, skip });
export const runRoutineNow = (id: number) => call<RoutineRunRow>({ action: 'run_now', routine_id: id });

// ---- the Inbox ------------------------------------------------------------

/** The routines whose newest run failed, newest first. An older hub (no
 *  `failing` action) or a refused read leaves it empty: the Inbox is never
 *  wrong about sessions because of it. */
export const failing = writable<FailingRoutine[]>([]);

export async function loadFailing(): Promise<void> {
  const r = await failingRoutines();
  failing.set(r.ok && Array.isArray(r.value) ? r.value : []);
}

/** How many failed runs the Inbox shows: added to its Needs you count. */
export const failingCount = derived(failing, ($f) => $f.length);

/** Retry: run it again now; the newer run takes it out of the Inbox. */
export async function retryRoutine(f: FailingRoutine): Promise<Result<RoutineRunRow>> {
  const r = await runRoutineNow(f.routine.id);
  await loadFailing();
  return r;
}

/** Pause: switch it off; a person's pause takes it out of the Inbox. */
export async function pauseRoutine(f: FailingRoutine): Promise<Result<RoutineRow>> {
  const r = await setRoutineEnabled(f.routine.id, false);
  await loadFailing();
  return r;
}

/** A request to show the Routines (the Automation screen's tab), on one
 *  routine and tab when set. `at` makes two identical requests distinct. */
export interface RoutinesRequest {
  select?: number;
  tab?: RoutineTab;
  /** Open a new routine from this template. */
  template?: string;
  at: number;
}
export type RoutineTab = 'runs' | 'definition' | 'limits';

export const routinesRequest = writable<RoutinesRequest | null>(null);

/** Open the Routines: Automation's Routines tab (8.4), on `r`'s routine. */
export function openRoutines(r: Omit<RoutinesRequest, 'at'> = {}): void {
  routinesRequest.set({ ...r, at: Date.now() });
  automationTab.set('routines');
  goTo('automation');
}

/**
 * Fix: what a failed run needs is in its session, so Fix opens it there;
 * a run that never started a session (an unreachable host, a refused
 * start) opens the routine's definition instead, where the host, account
 * and prompt are changed. Answers what it opened.
 */
export function fixRoutine(f: FailingRoutine): 'session' | 'definition' {
  const id = f.run.session_id;
  const s = id !== undefined ? get(sessions).find((x) => x.id === id) : undefined;
  if (s && s.status !== 'ghost') {
    selectSessionExplicitly(s);
    return 'session';
  }
  openRoutines({ select: f.routine.id, tab: 'definition' });
  return 'definition';
}

// ---- words ----------------------------------------------------------------

const DAYS = ['Sundays', 'Mondays', 'Tuesdays', 'Wednesdays', 'Thursdays', 'Fridays', 'Saturdays'];
const pad = (n: number) => String(n).padStart(2, '0');

/** A cron line in words: `30 7 * * 1-5` → `Weekdays 07:30`. A line it does
 *  not know is shown as it is. */
export function cronWords(cron: string | undefined): string {
  const c = (cron ?? '').trim();
  if (c === '@hourly') return 'Hourly';
  if (c === '@daily') return 'Daily 00:00';
  if (c === '@weekly') return 'Sundays 00:00';
  const p = c.split(/\s+/);
  if (p.length !== 5) return c;
  const [min, hour, dom, mon, dow] = p;
  if (!/^\d+$/.test(min)) return c;
  if (hour === '*' && dom === '*' && mon === '*' && dow === '*') return `Hourly at :${pad(Number(min))}`;
  if (!/^\d+$/.test(hour) || dom !== '*' || mon !== '*') return c;
  const at = `${pad(Number(hour))}:${pad(Number(min))}`;
  if (dow === '*') return `Daily ${at}`;
  if (dow === '1-5') return `Weekdays ${at}`;
  if (dow === '0,6' || dow === '6,0') return `Weekends ${at}`;
  if (/^[0-6]$/.test(dow)) return `${DAYS[Number(dow)]} ${at}`;
  return c;
}

/** When it runs, in words. */
export function triggerWords(r: Pick<RoutineRow, 'trigger' | 'cron' | 'event'>): string {
  if (r.trigger === 'cron') return cronWords(r.cron);
  if (r.trigger === 'event') return `When a session is ${eventWords(r.event)}`;
  return 'Run now only';
}

const EVENT_WORDS: Record<string, string> = {
  turn_done: 'done with a turn',
  stuck: 'stuck',
  lost: 'lost',
  needs_input: 'waiting for you',
};
export function eventWords(e: string | undefined): string {
  return (e && EVENT_WORDS[e]) || e || 'event';
}

/** On / Paused / Paused by fleet, the routine's switch in words. */
export function routineStateWords(r: Pick<RoutineRow, 'enabled' | 'paused_reason'>): string {
  if (r.enabled) return 'On';
  return r.paused_reason ? 'Paused by fleet' : 'Paused';
}

/** A run's result in the status words (Working, Failed, Done, Needs you). */
export function runWords(run: Pick<RoutineRunRow, 'state' | 'outcome' | 'reason'>): string {
  if (run.state === 'running') return 'Working';
  if (run.state === 'skipped') return run.reason ? `Skipped: ${run.reason}` : 'Skipped';
  if (run.state === 'failed' || run.outcome === 'failed') return run.reason ? `Failed: ${run.reason}` : 'Failed';
  if (run.outcome === 'needs_person') return 'Needs you';
  if (run.outcome === 'nothing') return 'Nothing to do';
  return 'Done';
}

/** Who read a run's outcome, when it was Jev (step 8.10): its answer is a
 *  reading of the run's last screen, which the exit and the rules override.
 *  `undefined` for the exit and the rules, which say it themselves. */
export function runSourceHint(run: Pick<RoutineRunRow, 'outcome' | 'outcome_source'>): string | undefined {
  if (run.outcome_source !== 'jev' || !run.outcome) return undefined;
  return "Jev read this from the run's last screen. Open the run's session to check it.";
}

/** `$0.38` from micro-dollars. */
export function dollars(micros: number | undefined): string {
  return `$${((micros ?? 0) / 1_000_000).toFixed(2)}`;
}

/** Dollars typed by a person as micro-dollars; empty or bad is no limit. */
export function microsOf(text: string): number | undefined {
  const t = text.trim().replace(/^\$/, '');
  if (t === '' || !/^\d+(\.\d{1,2})?$/.test(t)) return undefined;
  return Math.round(Number(t) * 1_000_000);
}

// ---- templates ------------------------------------------------------------

/** The device's offset east of UTC, in minutes, which a cron line is read at. */
export function deviceOffsetMin(): number {
  return -new Date().getTimezoneOffset();
}

/** The first template (step 8.6): every weekday morning, review the open
 *  PRs that changed since yesterday; comment on real defects, never push. */
export function morningPrSweep(host_alias: string, project_id: number, repo?: string): RoutineInput {
  return {
    name: 'Morning PR sweep',
    enabled: true,
    trigger: 'cron',
    cron: '30 7 * * 1-5',
    utc_offset_min: deviceOffsetMin(),
    host_alias,
    project_id,
    prompt:
      `Review every open PR in ${repo ?? 'this repository'} that changed since yesterday: ` +
      'CI, merge conflicts and open review threads. Comment on real defects only. Never push.',
    budget_run_micros: 2_000_000,
    budget_day_micros: 5_000_000,
    overlap: 'skip',
  };
}

export const TEMPLATES: { id: string; label: string; description: string }[] = [
  { id: 'morning-pr-sweep', label: 'Morning PR sweep', description: 'Weekdays 07:30 · CI, conflicts and review threads' },
];

/** How often the failed runs are read for the badge. */
export const FAILING_EVERY_MS = 60_000;

/**
 * Keep `failing` fresh for the Needs you badge, whatever view shows: read it
 * now and every minute while the window is visible. An older hub answers
 * E_INVALID for `failing`, which leaves it empty and stops the reads.
 * Returns the stop.
 */
export function trackFailingRoutines(
  deps: { load?: () => Promise<Result<FailingRoutine[]>>; doc?: Document | null; every?: number } = {},
): () => void {
  const load = deps.load ?? failingRoutines;
  const doc = deps.doc === undefined ? (typeof document === 'undefined' ? null : document) : deps.doc;
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const beat = async () => {
    timer = null;
    if (stopped) return;
    if (doc?.visibilityState !== 'hidden') {
      const r = await load();
      if (stopped) return;
      failing.set(r.ok && Array.isArray(r.value) ? r.value : []);
      if (!r.ok && r.error.code === 'E_INVALID') return;
    }
    timer = setTimeout(() => void beat(), deps.every ?? FAILING_EVERY_MS);
  };
  void beat();
  return () => {
    stopped = true;
    if (timer !== null) clearTimeout(timer);
  };
}

// ---- the Automation list and detail (board Automation) ---------------------

/** A cron line's days and time apart: `30 7 * * 1-5` → Weekdays, 07:30.
 *  `at` is absent for a line without one fixed time. */
export function cronParts(cron: string | undefined): { days: string; at?: string } {
  const words = cronWords(cron);
  const m = /^(.*) (\d\d:\d\d)$/.exec(words);
  return m ? { days: m[1], at: m[2] } : { days: words };
}

/** The newest run of each routine, from `list_runs` rows newest first. */
export function lastRunByRoutine(runs: readonly RunRow[]): Map<number, RunRow> {
  const out = new Map<number, RunRow>();
  for (const r of runs) {
    if (r.routine_id === undefined) continue;
    const had = out.get(r.routine_id);
    if (!had || r.started_at > had.started_at) out.set(r.routine_id, r);
  }
  return out;
}

const LAST_WORDS: Record<string, string> = {
  ok: 'last run OK',
  failed: 'last run failed',
  needs_person: 'last run needs you',
  nothing_to_do: 'last run had nothing to do',
};

function inWords(secs: number): string {
  if (secs < 60) return `${Math.max(0, Math.round(secs))}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h`;
  return `${Math.round(secs / 86400)}d`;
}

/** A routine's state as the list's dot says it: running now, its newest run
 *  failed, paused, or on. */
export function routineDot(r: Pick<RoutineRow, 'enabled'>, last?: Pick<RunRow, 'outcome'>): OfState {
  if (last?.outcome === 'running') return 'working';
  if (last?.outcome === 'failed') return 'failed';
  if (last?.outcome === 'needs_person') return 'waiting';
  return r.enabled ? 'done' : 'idle';
}

/** The list row's second line: "Weekdays · last run OK · next in 18h",
 *  "Daily · running now on mercury", "Fridays 16:00 · paused by you". */
export function routineLine(
  r: Pick<RoutineRow, 'enabled' | 'paused_reason' | 'trigger' | 'cron' | 'event' | 'next_run_at' | 'skip_next' | 'host_alias'>,
  last: Pick<RunRow, 'outcome'> | undefined,
  nowSec: number,
): string {
  const when = r.trigger === 'cron' ? cronParts(r.cron) : null;
  if (!r.enabled) {
    const head = when ? cronWords(r.cron) : triggerWords(r);
    return `${head} · ${r.paused_reason ? 'paused by fleet' : 'paused by you'}`;
  }
  const parts = [when ? when.days : triggerWords(r)];
  if (last?.outcome === 'running') return `${parts[0]} · running now on ${r.host_alias}`;
  parts.push(last ? (LAST_WORDS[last.outcome] ?? 'last run done') : 'not run yet');
  if (r.skip_next) parts.push('next one skipped');
  else if (r.next_run_at) parts.push(r.next_run_at > nowSec ? `next in ${inWords(r.next_run_at - nowSec)}` : 'due now');
  return parts.join(' · ');
}

/** A run's state for its dot in the Runs list. */
export function runDot(run: Pick<RoutineRunRow, 'state' | 'outcome'>): OfState {
  if (run.state === 'running') return 'working';
  if (run.state === 'skipped') return 'idle';
  if (run.state === 'failed' || run.outcome === 'failed') return 'failed';
  if (run.outcome === 'needs_person') return 'waiting';
  return 'done';
}

/** "$0.42 · 6 min": what the finished runs cost and took on average. */
export function averageRun(runs: readonly RoutineRunRow[]): string | null {
  const done = runs.filter((r) => r.finished_at !== undefined && r.state !== 'skipped');
  if (done.length === 0) return null;
  const cost = done.reduce((s, r) => s + r.cost_micros, 0) / done.length;
  const secs = done.reduce((s, r) => s + (r.finished_at! - r.started_at), 0) / done.length;
  return `${dollars(Math.round(cost))} · ${secs < 60 ? `${Math.round(secs)}s` : `${Math.round(secs / 60)} min`}`;
}

/** When it runs next, in words: "in 18h", "Next one skipped", "Paused". */
export function nextRunWords(r: Pick<RoutineRow, 'enabled' | 'trigger' | 'next_run_at' | 'skip_next'>, nowSec: number): string {
  if (!r.enabled) return 'Paused';
  if (r.trigger === 'manual') return 'Only with Run now';
  if (r.trigger === 'event') return 'On its event';
  if (r.skip_next) return 'Next one skipped';
  if (!r.next_run_at) return 'Not scheduled';
  return r.next_run_at > nowSec ? `in ${inWords(r.next_run_at - nowSec)}` : 'Due now';
}

/** What the routine's runs spent since `since` (unix seconds). */
export function spentSince(runs: readonly RoutineRunRow[], since: number): number {
  return runs.filter((r) => r.started_at >= since).reduce((s, r) => s + r.cost_micros, 0);
}
