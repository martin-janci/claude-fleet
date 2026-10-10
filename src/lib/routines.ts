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
  /** A pull request event fires only for this repo: `owner/name` or `name` (M15 G2.4). */
  event_repo?: string;
  /** `anyone`: PRs of any session in its org; absent = its owner's. */
  event_author?: string;
  /** At most one run per PR (or session) in this many seconds. */
  event_rate_secs?: number;
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
  event_repo?: string;
  event_author?: EventAuthor;
  event_rate_secs?: number;
}

/** `preview`: the editor's dry run (gap plan G2.3, fleet-core
 *  `routines::RoutinePreview`). Nothing is written. */
export interface RoutinePreview {
  /** The schedule's next fires (unix seconds), at the offset below. */
  next_runs: number[];
  utc_offset_min: number;
  /** Why save would refuse it; absent when it would save. */
  problem?: string;
  /** The account the chosen login bills. */
  account?: RoutineAccount;
  /** Every login on the host on a known account, the host's own first. */
  logins: RoutineAccount[];
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
export const previewRoutine = (routine: RoutineInput, id?: number) =>
  call<RoutinePreview>({ action: 'preview', routine, ...(id !== undefined ? { routine_id: id } : {}) });
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
  if (r.trigger === 'event') {
    if (isPrEvent(r.event)) return `When ${eventLabel(r.event).replace(/^A /, 'a ')}`;
    return `When a session is ${eventWords(r.event)}`;
  }
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

// Pull request triggers (M15 step G2.4; fleet-core `routines::PR_EVENTS`):
// reconcile writes one on the timeline of the session that opened a PR when
// its review, checks or state change. Only they take a repo filter and
// `anyone`; every event may hold a rate.
export type EventAuthor = 'me' | 'anyone';
const PR_EVENT_LABELS: Record<string, string> = {
  pr_review: 'A pull request gets a review',
  pr_ci_failed: 'A pull request’s checks fail',
  pr_ci_passed: 'A pull request’s checks pass',
  pr_merged: 'A pull request is merged',
};
/** The events the editor offers, session ones first. */
export const EVENT_CHOICES: readonly string[] = ['stuck', 'lost', 'turn_done', ...Object.keys(PR_EVENT_LABELS)];
export function isPrEvent(e: string | undefined): boolean {
  return !!e && e in PR_EVENT_LABELS;
}
/** An event as the editor's select names it. */
export function eventLabel(e: string | undefined): string {
  return (e && PR_EVENT_LABELS[e]) || `A session is ${eventWords(e)}`;
}
/** The rate choices: absent is "every time". */
export const RATE_CHOICES: readonly { secs?: number; label: string }[] = [
  { label: 'every time' },
  { secs: 600, label: 'once per 10 minutes' },
  { secs: 3600, label: 'once per hour' },
  { secs: 86400, label: 'once per day' },
];

/** An event routine's filters, as the editor holds them. */
export interface EventFilter {
  repo: string;
  author: EventAuthor;
  /** Seconds as text, '' for every time. */
  rate: string;
}
export function eventFilterOf(r: Pick<RoutineInput, 'event_repo' | 'event_author' | 'event_rate_secs'>): EventFilter {
  return {
    repo: r.event_repo ?? '',
    author: r.event_author === 'anyone' ? 'anyone' : 'me',
    rate: r.event_rate_secs ? String(r.event_rate_secs) : '',
  };
}
/** The fields `save` sends for `event`: none for another trigger, and the
 *  repo and author only for a pull request event (the hub refuses them on
 *  a session one). */
export function eventFilterInput(
  trigger: string,
  event: string | undefined,
  f: EventFilter,
): Pick<RoutineInput, 'event_repo' | 'event_author' | 'event_rate_secs'> {
  if (trigger !== 'event') return {};
  const rate = Number(f.rate);
  const out: Pick<RoutineInput, 'event_repo' | 'event_author' | 'event_rate_secs'> =
    Number.isInteger(rate) && rate > 0 ? { event_rate_secs: rate } : {};
  if (!isPrEvent(event)) return out;
  const repo = f.repo.trim();
  if (repo) out.event_repo = repo;
  if (f.author === 'anyone') out.event_author = 'anyone';
  return out;
}
/** "only in acme/web · anyone's · once per PR per hour", or '' when nothing narrows it. */
export function eventFilterWords(r: Pick<RoutineRow, 'event' | 'event_repo' | 'event_author' | 'event_rate_secs'>): string {
  const pr = isPrEvent(r.event);
  const parts: string[] = [];
  if (pr && r.event_repo) parts.push(`only in ${r.event_repo}`);
  if (pr) parts.push(r.event_author === 'anyone' ? 'anyone’s' : 'mine');
  const rate = RATE_CHOICES.find((c) => c.secs === r.event_rate_secs);
  if (r.event_rate_secs)
    parts.push(rate ? rate.label.replace('once per', `once per ${pr ? 'PR' : 'session'} per`) : `once per ${r.event_rate_secs}s`);
  return parts.join(' · ');
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

/** How many runs `get` returns at most (fleet-core `routines::RUNS_SHOWN`):
 *  a detail holding this many may have more. */
export const ROUTINE_RUNS_SHOWN = 20;

/** What deleting a routine loses, for the destructive confirm (G1.4): its
 *  runs go with it (fleet-core `delete_routine`), the sessions they started
 *  stay. `loss` is the runs counted, `lead` says it in one sentence. */
export function routineDeleteLoss(detail: Pick<RoutineDetail, 'runs'>): { loss: number; lead: string } {
  const n = detail.runs.length;
  const runs =
    n === 0
      ? 'It has no runs yet.'
      : n >= ROUTINE_RUNS_SHOWN
        ? `Its ${n} or more runs go with it.`
        : `Its ${n === 1 ? 'run goes' : `${n} runs go`} with it.`;
  return { loss: n, lead: `${runs} Sessions it started keep running. This can't be undone.` };
}

// ---- the schedule picker and its next run (gap plan G2.3) -----------------

/** The picker's days: each a cron day-of-week field, `hourly` and `custom`
 *  aside. */
export type ScheduleDays = 'daily' | 'weekdays' | 'weekends' | '0' | '1' | '2' | '3' | '4' | '5' | '6' | 'hourly' | 'custom';

export const SCHEDULE_DAYS: { id: ScheduleDays; label: string }[] = [
  { id: 'weekdays', label: 'Weekdays' },
  { id: 'daily', label: 'Every day' },
  { id: 'weekends', label: 'Weekends' },
  ...['1', '2', '3', '4', '5', '6', '0'].map((d) => ({ id: d as ScheduleDays, label: DAYS[Number(d)] })),
  { id: 'hourly', label: 'Every hour' },
  { id: 'custom', label: 'Custom (cron)' },
];

const DOW: Partial<Record<ScheduleDays, string>> = { daily: '*', weekdays: '1-5', weekends: '0,6' };

/** A cron line as the picker reads it: days and an `HH:MM` time (for
 *  `hourly`, the minute as `:MM`); a line it cannot show is `custom`. */
export function schedulePick(cron: string | undefined): { days: ScheduleDays; time: string } {
  const c = (cron ?? '').trim();
  const custom = { days: 'custom' as const, time: '09:00' };
  if (c === '@hourly') return { days: 'hourly', time: '00' };
  if (c === '@daily') return { days: 'daily', time: '00:00' };
  const p = c.split(/\s+/);
  if (p.length !== 5) return custom;
  const [min, hour, dom, mon, dow] = p;
  if (!/^\d{1,2}$/.test(min) || Number(min) > 59 || dom !== '*' || mon !== '*') return custom;
  if (hour === '*' && dow === '*') return { days: 'hourly', time: pad(Number(min)) };
  if (!/^\d{1,2}$/.test(hour) || Number(hour) > 23) return custom;
  const time = `${pad(Number(hour))}:${pad(Number(min))}`;
  const days = (Object.keys(DOW) as ScheduleDays[]).find((k) => DOW[k] === dow) ?? (dow === '6,0' ? 'weekends' : /^[0-6]$/.test(dow) ? (dow as ScheduleDays) : null);
  return days ? { days, time } : custom;
}

/** The cron line the picker means; `null` for `custom` or a bad time. */
export function scheduleCron(days: ScheduleDays, time: string): string | null {
  if (days === 'custom') return null;
  if (days === 'hourly') {
    const m = /^:?(\d{1,2})$/.exec(time.trim()) ?? /^\d{1,2}:(\d{2})$/.exec(time.trim());
    if (!m || Number(m[1]) > 59) return null;
    return `${Number(m[1])} * * * *`;
  }
  const m = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
  if (!m || Number(m[1]) > 23 || Number(m[2]) > 59) return null;
  return `${Number(m[2])} ${Number(m[1])} * * ${DOW[days] ?? days}`;
}

/** The device's IANA time zone ("Europe/Bratislava"), or null. */
export function deviceZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

/** Minutes east of UTC in `timeZone` (the device's when absent) at `sec`. */
export function zoneOffsetMin(sec: number, timeZone?: string): number {
  if (!timeZone) return -new Date(sec * 1000).getTimezoneOffset();
  const name = new Intl.DateTimeFormat('en-US', { timeZone, timeZoneName: 'longOffset' })
    .formatToParts(new Date(sec * 1000))
    .find((p) => p.type === 'timeZoneName')?.value;
  const m = /GMT([+-])(\d{2}):?(\d{2})?/.exec(name ?? '');
  if (!m) return 0;
  return (m[1] === '-' ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3] ?? 0));
}

/** "UTC+02:00" from minutes east. */
export function offsetWords(min: number): string {
  const a = Math.abs(min);
  return `UTC${min < 0 ? '−' : '+'}${pad(Math.floor(a / 60))}:${pad(a % 60)}`;
}

/** A fire as the board says it: "Mon 12 Oct, 08:30", in `timeZone` (the
 *  device's when absent). */
export function nextRunLabel(sec: number, timeZone?: string): string {
  const d = new Date(sec * 1000);
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone,
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(d);
  const v = (t: string) => parts.find((p) => p.type === t)?.value ?? '';
  return `${v('weekday')} ${v('day')} ${v('month')}, ${v('hour')}:${v('minute')}`;
}

/**
 * The first of `runs` whose wall-clock time moved because a clock change
 * lies between now and it: a routine keeps the UTC offset it was saved at
 * (fleet-core `routines::cron`), so past a daylight-saving change it fires
 * an hour off. `null` when none moved.
 */
export function clockChange(
  runs: readonly number[],
  savedOffsetMin: number,
  timeZone?: string,
): { at: number; shiftMin: number } | null {
  for (const at of runs) {
    const shift = zoneOffsetMin(at, timeZone) - savedOffsetMin;
    if (shift !== 0) return { at, shiftMin: shift };
  }
  return null;
}

/** The editor's dry-run line: "Dry run: on mercury, in acme/web, as
 *  me@x.com, at most $2.00 a run." */
export function dryRunLine(
  d: { host_alias: string; profile?: string; budget_run_micros?: number },
  project: string,
  account: string | null,
): string {
  const as = account ?? (d.profile ? `profile ${d.profile}` : "the host's own login");
  const cap = d.budget_run_micros !== undefined ? `at most ${dollars(d.budget_run_micros)} a run` : 'no limit a run';
  return `Dry run: on ${d.host_alias}, in ${project}, as ${as}, ${cap}.`;
}
