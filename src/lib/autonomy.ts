// The autonomy dial in Control's composer (Thread room, 2026-10-14; board
// Autonomy): one chip instead of three settings. Each position sets the
// mission loop's ceiling (`orchestrator.max_level`), whether Control's chat
// goes on with the recommended option and after how long
// (`control.auto_continue`, `control.auto_continue_minutes`) and whether
// Control hears what changed (`control.react_to_events`), together, through
// the same `set_fleet_setting` path Settings uses. Settings → Control API
// keeps each one for whoever wants them apart; a mix the dial does not name
// reads as Custom. The hub reads the same position (`service::autonomy`,
// `autonomy_positions_mirror_the_dial` keeps the rows in step) and
// pre-approves the operator's calls by it (`guard::operator_must_confirm`):
// Custom grants what Ask first does.
import { setFleetSetting, settingBool, settingInt, SETTING_KEYS, type FleetSettings, type SettingKey } from './fleet_settings';

export type AutonomyId = 'ask' | 'brief' | 'autopilot';

export interface AutonomyPosition {
  id: AutonomyId;
  label: string;
  /** What it allows, one line (the board's words). */
  blurb: string;
  maxLevel: number;
  autoContinue: boolean;
  /** Only read when `autoContinue`. */
  minutes: number;
  reactToEvents: boolean;
}

export const AUTONOMY: readonly AutonomyPosition[] = [
  {
    id: 'ask',
    label: 'Ask first',
    blurb: 'Proposes every step and waits. Still hears its sessions and tells you what happened.',
    maxLevel: 1,
    autoContinue: false,
    minutes: 10,
    reactToEvents: true,
  },
  {
    id: 'brief',
    label: 'Within your brief',
    blurb: 'Prompts and briefs the sessions it started or briefed without asking you, and goes on with a recommended choice after 10 minutes. Asks before starting, stopping or restarting anything, unless “Control looks after its sessions” is on: then it restarts a stuck one itself.',
    maxLevel: 2,
    autoContinue: true,
    minutes: 10,
    reactToEvents: true,
  },
  {
    id: 'autopilot',
    label: 'Autopilot',
    blurb: "Also starts sessions inside a mission's grant (its hosts, hours and budget) and creates and starts its tasks, and goes on with a recommended choice after 2 minutes. Kills, restarts and deletes still ask, unless “Control looks after its sessions” is on: then it also restarts a stuck session and retires a finished one after pushing its work.",
    maxLevel: 3,
    autoContinue: true,
    minutes: 2,
    reactToEvents: true,
  },
];

/** Where the settings put the dial; `custom` for a mix it does not name. */
export function autonomyOf(map: FleetSettings): AutonomyId | 'custom' {
  const level = settingInt(map, SETTING_KEYS.orchestratorMaxLevel);
  const ac = settingBool(map, SETTING_KEYS.controlAutoContinue);
  const minutes = settingInt(map, SETTING_KEYS.controlAutoContinueMinutes);
  const react = settingBool(map, SETTING_KEYS.controlReactToEvents);
  const hit = AUTONOMY.find(
    (p) => p.maxLevel === level && p.autoContinue === ac && (!ac || p.minutes === minutes) && p.reactToEvents === react,
  );
  return hit?.id ?? 'custom';
}

export function autonomyLabel(id: AutonomyId | 'custom'): string {
  return AUTONOMY.find((p) => p.id === id)?.label ?? 'Custom';
}

/** The writes a position makes, in order; a value already set is skipped. */
export function autonomyWrites(id: AutonomyId, map: FleetSettings): [SettingKey, string][] {
  const p = AUTONOMY.find((x) => x.id === id)!;
  const want: [SettingKey, string][] = [
    [SETTING_KEYS.orchestratorMaxLevel, String(p.maxLevel)],
    [SETTING_KEYS.controlAutoContinue, String(p.autoContinue)],
    ...(p.autoContinue ? ([[SETTING_KEYS.controlAutoContinueMinutes, String(p.minutes)]] as [SettingKey, string][]) : []),
    [SETTING_KEYS.controlReactToEvents, String(p.reactToEvents)],
  ];
  return want.filter(([k, v]) => map[k] !== v);
}

/** Set a position. Answers an error message, or null. A failed write stops
 *  the rest, and says which setting it was. */
export async function setAutonomy(id: AutonomyId, map: FleetSettings): Promise<string | null> {
  for (const [key, value] of autonomyWrites(id, map)) {
    const r = await setFleetSetting(key, value);
    if (!r.ok) return `Couldn't set ${key}: ${r.error.message}`;
  }
  return null;
}
