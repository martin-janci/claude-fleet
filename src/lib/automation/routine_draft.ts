// The routine editor's draft (FormsAutomation board, "New routine"): the
// form's fields as text, read from a saved routine and written back as the
// `RoutineInput` that `save` and `preview` take. RoutinesPanel owns the
// draft and saves it; RoutineEditor draws it.
import {
  capSecsOf,
  eventFilterInput,
  eventFilterOf,
  microsOf,
  scheduleCron,
  schedulePick,
  zoneOffsetMin,
  type EventFilter,
  type RoutineInput,
  type RoutineTrigger,
  type ScheduleDays,
} from '../routines';

export interface RoutineDraft {
  /** Absent for a new routine. */
  id?: number;
  name: string;
  trigger: RoutineTrigger;
  /** The picker's days and time; `custom` edits `cron` as text. */
  days: ScheduleDays;
  time: string;
  cron: string;
  event: string;
  host_alias: string;
  project_id: string;
  profile: string;
  prompt: string;
  budget_run: string;
  budget_day: string;
  overlap: 'skip' | 'parallel';
  filter: EventFilter;
  /** Time cap in minutes; empty = none (G3.8). */
  cap_min: string;
  fallback_host: string;
  retry_once: boolean;
  autonomy: number;
  /** The schedule's zone (G7.4's Time zone select); '' = this device's. */
  zone: string;
}

function dollarsField(m: number | undefined): string {
  return m === undefined ? '' : (m / 1_000_000).toFixed(2);
}

export function draftOf(i: RoutineInput, id?: number): RoutineDraft {
  const pick = schedulePick(i.cron);
  return {
    id,
    name: i.name,
    trigger: i.trigger,
    days: pick.days,
    time: pick.time,
    cron: i.cron ?? '',
    event: i.event ?? 'stuck',
    host_alias: i.host_alias,
    project_id: i.project_id ? String(i.project_id) : '',
    profile: i.profile ?? '',
    prompt: i.prompt,
    budget_run: dollarsField(i.budget_run_micros),
    budget_day: dollarsField(i.budget_day_micros),
    overlap: i.overlap === 'parallel' ? 'parallel' : 'skip',
    filter: eventFilterOf(i),
    cap_min: i.run_max_secs ? String(Math.round(i.run_max_secs / 60)) : '',
    fallback_host: i.fallback_host ?? '',
    retry_once: i.retry_once ?? false,
    autonomy: i.autonomy ?? 2,
    zone: i.time_zone ?? '',
  };
}

/** The cron line the draft means: the picker's, or the text of Custom. */
export const draftCron = (d: RoutineDraft): string => (d.days === 'custom' ? d.cron.trim() : (scheduleCron(d.days, d.time) ?? ''));

/** The zone the draft's schedule is read in: its own, else the device's. */
export const draftZone = (d: RoutineDraft, device: string | null): string | undefined => d.zone || device || undefined;

/** What the draft still lacks before it can be saved, as the line under the
 *  disabled verb ("A disabled verb always says why under it"); null when
 *  it is ready. */
export function draftMissing(d: RoutineDraft): string | null {
  if (d.name.trim() === '') return 'Name the routine.';
  if (d.trigger === 'cron' && draftCron(d) === '') return d.days === 'custom' ? 'Write the cron line.' : 'Pick a time.';
  if (d.host_alias === '') return 'Pick a host.';
  if (d.project_id === '') return 'Pick a project.';
  if (d.prompt.trim() === '') return 'Write the prompt.';
  return null;
}

/** The routine as `save` and `preview` read it. `enabled` is the saved
 *  routine's (a new one starts on). */
export function draftInput(d: RoutineDraft, enabled: boolean, device: string | null): RoutineInput {
  const zone = draftZone(d, device);
  return {
    name: d.name.trim(),
    enabled,
    trigger: d.trigger,
    cron: d.trigger === 'cron' ? draftCron(d) : undefined,
    utc_offset_min: zoneOffsetMin(Date.now() / 1000, zone),
    event: d.trigger === 'event' ? d.event : undefined,
    host_alias: d.host_alias,
    project_id: Number(d.project_id),
    profile: d.profile.trim() || undefined,
    prompt: d.prompt.trim(),
    budget_run_micros: microsOf(d.budget_run),
    budget_day_micros: microsOf(d.budget_day),
    overlap: d.overlap,
    ...eventFilterInput(d.trigger, d.event, d.filter),
    ...(zone ? { time_zone: zone } : {}),
    run_max_secs: capSecsOf(d.cap_min),
    fallback_host: d.fallback_host || undefined,
    retry_once: d.retry_once,
    autonomy: d.autonomy < 2 ? d.autonomy : undefined,
  };
}
