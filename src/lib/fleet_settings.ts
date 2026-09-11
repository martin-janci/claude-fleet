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
  projectsBasePath: 'projects.base_path',
  projectsLayout: 'projects.layout',
  repairAutoOnTick: 'repair.auto_on_tick',
  repairTickIntervalSecs: 'repair.tick_interval_secs',
} as const;

/** Derived, read-only entry in the `get_fleet_settings` map: JSON object of
 *  host alias → resolved projects root (setting → env var → default). */
export const PROJECTS_RESOLVED_KEY = 'projects.resolved_base';

/** Derived, read-only entry: `$CLAUDE_FLEET_PROJECTS_BASE` as the app sees it
 *  (trimmed), or `''` when unset. */
export const PROJECTS_LOCAL_ENV_KEY = 'projects.local_env_base';

/** Mirror of `settings::MAX_PATH_LEN`. */
export const BASE_PATH_MAX_LEN = 1024;

export type ProjectsLayout = 'github' | 'flat';

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
  'projects.base_path': '{}',
  'projects.layout': 'github',
  'repair.auto_on_tick': 'false',
  'repair.tick_interval_secs': '600',
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

/** Parse a JSON `alias → path` map setting; `{}` on anything malformed. */
export function settingPathMap(map: FleetSettings, key: string): Record<string, string> {
  try {
    const v: unknown = JSON.parse(map[key] ?? '{}');
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {};
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>).filter(
        (e): e is [string, string] => typeof e[1] === 'string',
      ),
    );
  } catch {
    return {};
  }
}

export function settingLayout(map: FleetSettings): ProjectsLayout {
  return map[SETTING_KEYS.projectsLayout] === 'flat' ? 'flat' : 'github';
}

/** Client-side mirror of `settings::validate_base_path` (the backend stays
 *  authoritative). Returns an error message, or null when acceptable. */
export function basePathError(path: string): string | null {
  const p = path.trim();
  if (p === '') return null; // blank = no override
  if (!(p.startsWith('/') || p === '~' || p.startsWith('~/'))) {
    return 'must be absolute or start with ~/';
  }
  if (p.length > BASE_PATH_MAX_LEN) return 'path is too long';
  if (p.split('/').includes('..')) return "must not contain '..'";
  // C0, DEL and C1 (U+0080..U+009F): Rust's `char::is_control`.
  const isControl = (ch: string) => {
    const c = ch.charCodeAt(0);
    return c < 32 || (c >= 127 && c <= 159);
  };
  if ([...p].some(isControl)) return 'must not contain control characters';
  return null;
}

/** Mirror of `projects::Layout::default_root`: the root used when neither the
 *  setting nor (on `local`) `$CLAUDE_FLEET_PROJECTS_BASE` names one. */
export function projectsDefaultRoot(layout: ProjectsLayout): string {
  return layout === 'flat' ? '~/projects' : '~/projects/github.com';
}

/** Mirror of `projects::Layout::project_dir`: one project's directory. */
export function projectDir(root: string, layout: ProjectsLayout, owner: string, repo: string): string {
  const r = root.replace(/\/+$/, '');
  return layout === 'flat' ? `${r}/${repo}` : `${r}/${owner}/${repo}`;
}

/** Example project path under `root` for the preview line. */
export function projectPathPreview(root: string, layout: ProjectsLayout): string {
  const r = root.replace(/\/+$/, '');
  return layout === 'flat' ? `${r}/<repo>` : `${r}/<owner>/<repo>`;
}

/** Seconds ↔ the hours shown in the Settings inputs (TTLs are long). */
export function secsToHours(secs: number): number {
  return Math.round((secs / 3600) * 100) / 100;
}

export function hoursToSecs(hours: number): number {
  return Math.max(0, Math.round(hours * 3600));
}
