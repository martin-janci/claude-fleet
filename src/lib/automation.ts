// Automation (Orbit Fleet redesign step 8.4, the Automation board): what
// runs on the fleet's behalf, read-only. The built-in routines are 8.1's
// loops (`fleet_health.loops`), the Runs are 8.3's (`list_runs`), and the
// built-in agents are the operator, the mission orchestrator and Jev. Pause
// all is `automation.paused`: every loop that acts on its own stands still
// until it is cleared. Routines a person writes (8.5, 8.6) join the Routines
// tab beside the built-in ones.
import { derived, writable } from 'svelte/store';
import { healthCheck, type LoopHealth } from './ipc';
import { listRuns, type RunRow } from './runs';
import { fleetSettings, setFleetSetting, SETTING_KEYS, settingBool, type FleetSettings } from './fleet_settings';
import type { Result } from './result';

export type AutomationTab = 'routines' | 'runs' | 'agents';

export const automationTab = writable<AutomationTab>('routines');

/** `automation.paused`, as the settings store holds it. */
export const automationPaused = derived(fleetSettings, ($s) => settingBool($s, SETTING_KEYS.automationPaused));

/** Pause every loop that acts on its own, or let them run again. */
export function setAutomationPaused(on: boolean): Promise<Result<FleetSettings>> {
  return setFleetSetting(SETTING_KEYS.automationPaused, on ? 'true' : 'false');
}

/** Local midnight today, in unix seconds. */
export function startOfToday(nowMs: number = Date.now()): number {
  const d = new Date(nowMs);
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

/** "$4.10" from micro-USD. */
export function money(micros: number): string {
  return `$${(micros / 1_000_000).toFixed(2)}`;
}

/** What the runs cost together; a task's spend is its session's, so absent. */
export function spendMicros(runs: readonly Pick<RunRow, 'cost_micros'>[]): number {
  return runs.reduce((sum, r) => sum + (r.cost_micros ?? 0), 0);
}

/** Today's runs, every page of them up to `cap`. */
export async function runsToday(nowMs: number = Date.now(), cap = 1000): Promise<Result<RunRow[]>> {
  const since = startOfToday(nowMs);
  const out: RunRow[] = [];
  for (let offset = 0; offset < cap; offset += 200) {
    const r = await listRuns({ since, limit: 200, offset });
    if (!r.ok) return r;
    out.push(...r.value.runs);
    if (out.length >= r.value.total || r.value.runs.length === 0) break;
  }
  return { ok: true, value: out };
}

function inWords(secs: number): string {
  if (secs < 60) return `${Math.max(0, Math.round(secs))}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86400) return `${Math.round(secs / 3600)}h`;
  return `${Math.round(secs / 86400)}d`;
}

/** A built-in routine's line: "last run 3m ago · next in 5m", "paused",
 *  "failed 2m ago: …", "not run yet here". */
export function loopLine(loop: LoopHealth, nowSec: number, paused: boolean): string {
  const parts: string[] = [];
  if (paused && loop.pausable) parts.push('paused');
  if (loop.last_run_at) {
    const ago = `${inWords(nowSec - loop.last_run_at)} ago`;
    parts.push(loop.result === 'error' ? `failed ${ago}${loop.last_error ? `: ${loop.last_error}` : ''}` : `last run ${ago}`);
  } else {
    parts.push('not run yet here');
  }
  if (loop.next_run_at && !(paused && loop.pausable)) {
    parts.push(loop.next_run_at > nowSec ? `next in ${inWords(loop.next_run_at - nowSec)}` : 'due now');
  }
  return parts.join(' · ');
}

/** How often a loop runs, from its last and next run. */
export function loopEvery(loop: LoopHealth): string | null {
  if (!loop.last_run_at || !loop.next_run_at || loop.next_run_at <= loop.last_run_at) return null;
  return `every ${inWords(loop.next_run_at - loop.last_run_at)}`;
}

export interface BuiltInAgent {
  id: 'operator' | 'orchestrator' | 'jev';
  name: string;
  does: string;
  state: string;
}

/** Jev's switches that are on (`decide.jev.*` modes other than off). */
export function jevFeaturesOn(settings: FleetSettings): number {
  return Object.entries(settings).filter(
    ([k, v]) => k.startsWith('decide.jev.') && k !== SETTING_KEYS.decideJevEnabled && (v === 'shadow' || v === 'assist'),
  ).length;
}

/** The three agents fleet runs itself, with what each is doing now. */
export function builtInAgents(
  settings: FleetSettings,
  loops: readonly LoopHealth[],
  runs: readonly RunRow[],
  nowSec: number,
): BuiltInAgent[] {
  const lastOperator = runs.filter((r) => r.kind === 'operator').sort((a, b) => b.started_at - a.started_at)[0];
  const missions = loops.find((l) => l.name === 'missions');
  const jevOn = settingBool(settings, SETTING_KEYS.decideJevEnabled);
  const features = jevFeaturesOn(settings);
  return [
    {
      id: 'operator',
      name: 'Operator',
      does: 'The fleet agent you talk to in Control; hands work to sessions and missions.',
      state: lastOperator ? `last ran ${inWords(nowSec - lastOperator.started_at)} ago` : 'no run today',
    },
    {
      id: 'orchestrator',
      name: 'Orchestrator',
      does: 'Runs missions: plans, dispatches tasks, applies the brakes.',
      state: missions ? loopLine(missions, nowSec, settingBool(settings, SETTING_KEYS.automationPaused)) : 'not reported',
    },
    {
      id: 'jev',
      name: 'Jev',
      does: 'Proposes the small calls a person would otherwise make; a person confirms.',
      state: jevOn ? `on · ${features} use case${features === 1 ? '' : 's'}` : 'off',
    },
  ];
}

export interface AutomationState {
  loops: LoopHealth[];
  paused: boolean;
  today: RunRow[];
}

/** The loops and today's runs, read together. */
export async function loadAutomation(nowMs: number = Date.now()): Promise<Result<AutomationState>> {
  const [h, r] = await Promise.all([healthCheck(), runsToday(nowMs)]);
  if (!h.ok) return h;
  if (!r.ok) return r;
  return { ok: true, value: { loops: h.value.loops ?? [], paused: h.value.automation_paused ?? false, today: r.value } };
}
