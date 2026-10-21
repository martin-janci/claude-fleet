// The routine editor's draft (FormsAutomation board, "New routine"): the
// form's fields as text, read from a saved routine and written back as the
// `RoutineInput` that `save` and `preview` take. RoutinesPanel owns the
// draft and saves it; RoutineEditor draws it.
import {
  budgetProblem,
  capProblem,
  capSecsOf,
  deviceZone,
  eventFilterInput,
  eventFilterOf,
  microsOf,
  scheduleCron,
  schedulePick,
  zoneOffsetMin,
  type EventFilter,
  type RoutineInput,
  type RoutineMemoryMode,
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
  /** The notebooks its runs read and rewrite (migration 204); '' = none. */
  memory?: RoutineMemoryMode | '';
  /** The fleet notes each finished run in the notebook. */
  memory_auto?: boolean;
  /** The saved routine keeps a notebook: clearing `memory` then sends
   *  `memory: ''`, since a change without it keeps the routine's. */
  saved_memory?: boolean;
  /** The schedule's zone (G7.4's Time zone select); '' = this device's. */
  zone: string;
  /** A routine saved without a zone at an offset no zone here stands for
   *  (UTC+05:30 when this device is at +02:00): saving re-reads it in
   *  `zone`, which the editor warns about (review w05-3). */
  saved_offset?: number;
}

/** The zone a routine saved without one is read in, so opening and saving
 *  it does not move when it fires (review w05-3): this device's when its
 *  offset is the device's (now, or across the year's clock change, as an
 *  older desktop saved it), `UTC` at 0 (the hub's default, an MCP save),
 *  else the fixed `Etc/GMT∓h` zone of a whole-hour offset. `offset` is set
 *  when none fits. */
export function zoneOfSaved(
  i: Pick<RoutineInput, 'time_zone' | 'utc_offset_min'>,
  device: string | undefined = deviceZone() ?? undefined,
): { zone: string; offset?: number } {
  if (i.time_zone) return { zone: i.time_zone };
  if (i.utc_offset_min === undefined) return { zone: '' };
  const off = i.utc_offset_min;
  const year = new Date().getUTCFullYear();
  const deviceOffsets = [Date.now() / 1000, Date.UTC(year, 0, 1) / 1000, Date.UTC(year, 6, 1) / 1000].map((t) => zoneOffsetMin(t, device));
  if (deviceOffsets.includes(off)) return { zone: '' };
  if (off === 0) return { zone: 'UTC' };
  if (off % 60 === 0) {
    const fixed = `Etc/GMT${off > 0 ? '-' : '+'}${Math.abs(off) / 60}`;
    try {
      new Intl.DateTimeFormat('en-US', { timeZone: fixed });
      return { zone: fixed };
    } catch {
      // Past ±12/14 h: no such zone; warn instead.
    }
  }
  return { zone: '', offset: off };
}

function dollarsField(m: number | undefined): string {
  return m === undefined ? '' : (m / 1_000_000).toFixed(2);
}

export function draftOf(i: RoutineInput, id?: number): RoutineDraft {
  const pick = schedulePick(i.cron);
  const z = zoneOfSaved(i);
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
    memory: i.memory ?? '',
    memory_auto: i.memory_auto ?? false,
    ...(i.memory ? { saved_memory: true } : {}),
    zone: z.zone,
    ...(z.offset !== undefined ? { saved_offset: z.offset } : {}),
  };
}

const pad2 = (n: number) => String(n).padStart(2, '0');

/** The time field as `days` reads it: the minute alone for `hourly`,
 *  `HH:MM` for the rest (`custom` keeps it as it is). */
export function timeFor(days: ScheduleDays, time: string): string {
  const t = time.trim();
  const hm = /^(\d{1,2}):(\d{2})$/.exec(t);
  const mm = /^:?(\d{1,2})$/.exec(t);
  if (days === 'custom') return time;
  if (days === 'hourly') return hm ? hm[2] : mm && Number(mm[1]) <= 59 ? pad2(Number(mm[1])) : '00';
  if (hm) return time;
  return mm && Number(mm[1]) <= 59 ? `09:${pad2(Number(mm[1]))}` : '09:00';
}

/** Repeats changed to `next` (review w05-7): Custom starts from the line
 *  the picker meant, a picker leaving Custom reads that line's time, and
 *  the time is kept in the form the new choice shows. */
export function withDays(d: RoutineDraft, next: ScheduleDays): void {
  const prev = d.days;
  if (next === prev) return;
  if (next === 'custom') {
    d.cron = scheduleCron(prev, d.time) ?? d.cron;
  } else if (prev === 'custom') {
    const pick = schedulePick(d.cron);
    if (pick.days !== 'custom') d.time = pick.time;
  }
  d.days = next;
  d.time = timeFor(next, d.time);
}

/** A draft the editor held when its tab closed under it (review w05-2):
 *  RoutinesPanel puts a changed one here as it unmounts and takes it back
 *  when it mounts again, so a tab switch does not throw it away. */
let stashed: { draft: RoutineDraft; base: string } | null = null;
export function stashDraft(v: { draft: RoutineDraft; base: string } | null): void {
  stashed = v;
}
export function takeStashedDraft(): { draft: RoutineDraft; base: string } | null {
  const v = stashed;
  stashed = null;
  return v;
}

/** The cron line the draft means: the picker's, or the text of Custom. */
export const draftCron = (d: RoutineDraft): string => (d.days === 'custom' ? d.cron.trim() : (scheduleCron(d.days, d.time) ?? ''));

/** The zone the draft's schedule is read in: its own, else the device's. */
export const draftZone = (d: RoutineDraft, device: string | null): string | undefined => d.zone || device || undefined;

/** What the draft still lacks before it can be saved, as the line under the
 *  disabled verb ("A disabled verb always says why under it"); null when
 *  it is ready. */
export function draftMissing(d: RoutineDraft, projectIds?: readonly number[]): string | null {
  if (d.name.trim() === '') return 'Name the routine.';
  if (d.trigger === 'cron' && draftCron(d) === '') return d.days === 'custom' ? 'Write the cron line.' : 'Pick a time.';
  if (d.host_alias === '') return 'Pick a host.';
  if (d.project_id === '') return 'Pick a project.';
  // A routine whose project was deleted (R02-1) keeps its id: pick another.
  if (projectIds && !projectIds.includes(Number(d.project_id))) return 'Pick a project: its project is gone.';
  if (d.prompt.trim() === '') return 'Write the prompt.';
  if (budgetProblem(d.budget_run)) return 'Fix the budget per run.';
  if (budgetProblem(d.budget_day)) return 'Fix the budget per day.';
  if (capProblem(d.cap_min)) return 'Fix the time cap.';
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
    // Only when it keeps a notebook, or kept one (a change without
    // `memory` keeps the routine's): a hub that predates memory refuses
    // the fields, and every other routine still saves there.
    ...(d.memory ? { memory: d.memory, memory_auto: d.memory_auto ?? false } : d.saved_memory ? { memory: '' as const } : {}),
  };
}
