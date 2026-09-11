// Operator settings stored in the backend `settings` table (playbooks, GC,
// reconcile cadence). The backend registry (`service::settings`) owns the key
// list, defaults and validation; this module mirrors the keys so the Settings
// dialog and tests can address them by name, and keeps the last map the
// backend returned in a store.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export const SETTING_KEYS = {
  reconcileIntervalSecs: 'reconcile.interval_secs',
  playbookPressEnter: 'playbooks.press_enter',
  playbookOomRecreate: 'playbooks.oom_recreate',
  gcEnabled: 'gc.enabled',
  gcBgIdleSecs: 'gc.bg_idle_secs',
  gcShellIdleSecs: 'gc.shell_idle_secs',
  gcWorkIdleSecs: 'gc.work_idle_secs',
  gcSweepIntervalSecs: 'gc.sweep_interval_secs',
} as const;

export type SettingKey = (typeof SETTING_KEYS)[keyof typeof SETTING_KEYS];

/** Defaults mirrored from `service::settings::SPECS`, used until the first
 *  `get_fleet_settings` round-trip completes. */
export const SETTING_DEFAULTS: Record<SettingKey, string> = {
  'reconcile.interval_secs': '20',
  'playbooks.press_enter': 'false',
  'playbooks.oom_recreate': 'false',
  'gc.enabled': 'false',
  'gc.bg_idle_secs': '86400',
  'gc.shell_idle_secs': '604800',
  'gc.work_idle_secs': '0',
  'gc.sweep_interval_secs': '300',
};

export type FleetSettings = Record<string, string>;

export const fleetSettings = writable<FleetSettings>({ ...SETTING_DEFAULTS });

export async function loadFleetSettings(): Promise<Result<FleetSettings>> {
  const r = await invokeCmd<FleetSettings>('get_fleet_settings');
  if (r.ok && r.value) fleetSettings.set({ ...SETTING_DEFAULTS, ...r.value });
  return r;
}

export async function setFleetSetting(key: SettingKey, value: string): Promise<Result<FleetSettings>> {
  const r = await invokeCmd<FleetSettings>('set_fleet_setting', { key, value });
  if (r.ok && r.value) fleetSettings.set({ ...SETTING_DEFAULTS, ...r.value });
  return r;
}

export function settingBool(map: FleetSettings, key: SettingKey): boolean {
  return (map[key] ?? SETTING_DEFAULTS[key]) === 'true';
}

export function settingSecs(map: FleetSettings, key: SettingKey): number {
  const n = Number.parseInt(map[key] ?? SETTING_DEFAULTS[key], 10);
  return Number.isFinite(n) && n >= 0 ? n : Number.parseInt(SETTING_DEFAULTS[key], 10);
}

/** Seconds ↔ the hours shown in the Settings inputs (TTLs are long). */
export function secsToHours(secs: number): number {
  return Math.round((secs / 3600) * 100) / 100;
}

export function hoursToSecs(hours: number): number {
  return Math.max(0, Math.round(hours * 3600));
}
