// Operator settings stored in the backend `settings` table (playbooks, GC,
// reconcile cadence). The backend registry (`service::settings`) owns the key
// list, defaults and validation; this module mirrors the keys so the Settings
// dialog and tests can address them by name, and keeps the last map the
// backend returned in a store.
import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export const SETTING_KEYS = {
  reconcileIntervalSecs: 'reconcile.interval_secs',
  reconcileStaleWorkingSecs: 'reconcile.stale_working_secs',
  reconcileStaleWorkingTtlSecs: 'reconcile.stale_working_ttl_secs',
  sessionsLostTtlSecs: 'sessions.lost_ttl_secs',
  restoreBatchSize: 'restore.batch_size',
  restoreStaggerMs: 'restore.stagger_ms',
  automationPaused: 'automation.paused',
  automationDailyBudget: 'automation.daily_budget',
  orchestratorEnabled: 'orchestrator.enabled',
  orchestratorMaxLevel: 'orchestrator.max_level',
  playbookPressEnter: 'playbooks.press_enter',
  playbookOomRecreate: 'playbooks.oom_recreate',
  playbookOomMaxAttempts: 'playbooks.oom_max_attempts',
  gcEnabled: 'gc.enabled',
  gcBgIdleSecs: 'gc.bg_idle_secs',
  gcShellIdleSecs: 'gc.shell_idle_secs',
  gcWorkIdleSecs: 'gc.work_idle_secs',
  gcSweepIntervalSecs: 'gc.sweep_interval_secs',
  gcExternalLostTtlSecs: 'gc.external_lost_ttl_secs',
  projectsBasePath: 'projects.base_path',
  projectsLayout: 'projects.layout',
  repairAutoOnTick: 'repair.auto_on_tick',
  repairTickIntervalSecs: 'repair.tick_interval_secs',
  tasksMaxAgeSecs: 'tasks.max_age_secs',
  downloadsMaxFileMb: 'downloads.max_file_mb',
  downloadsMaxTotalMb: 'downloads.max_total_mb',
  downloadsKeepSecs: 'downloads.keep_secs',
  voiceEnabled: 'voice.enabled',
  voiceMaxCaptureSecs: 'voice.max_capture_secs',
  voiceClaimTtlSecs: 'voice.claim_ttl_secs',
  moveMaxTranscriptMb: 'move.max_transcript_mb',
  moveMaxBundleMb: 'move.max_bundle_mb',
  moveIgnoredEntryKb: 'move.ignored_entry_kb',
  moveIgnoredTotalMb: 'move.ignored_total_mb',
  moveMaxSessionStateMb: 'move.max_session_state_mb',
  moveWaitMaxMins: 'move.wait_max_mins',
  usageEnabled: 'usage.enabled',
  usageIntervalSecs: 'usage.interval_secs',
  accountsPauseAt: 'accounts.pause_at',
  usagePricesJson: 'usage.prices_json',
  reportsMaxRows: 'reports.max_rows',
  reportsMaxAgeSecs: 'reports.max_age_secs',
  healthContextRedPct: 'health.context_red_pct',
  healthVersionMaxAgeSecs: 'health.version_max_age_secs',
  healthDiskLowPct: 'health.disk_low_pct',
  healthClaudeMaxBehind: 'health.claude_max_behind',
  healthHooksSilentSecs: 'health.hooks_silent_secs',
  provisionForceGitTree: 'provision.force_git_tree',
  provisionInstallAg: 'provision.install_ag',
  workRetentionJournalDays: 'work.retention.journal_days',
  workRetentionTrackerItemsDays: 'work.retention.tracker_items_days',
  workRetentionTimelineWorkEventsDays: 'work.retention.timeline_work_events_days',
  workRecentDays: 'work.recent_days',
  workSyncIntervalSecs: 'work.sync_interval_secs',
  catalogScanCheckSecs: 'catalog.scan_check_secs',
  catalogScanMaxAgeSecs: 'catalog.scan_max_age_secs',
  catalogAuto: 'catalog.auto',
  catalogAutoPush: 'catalog.auto_push',
  workDescribeCacheSecs: 'work.describe_cache_secs',
  workTrustedBranchProjects: 'work.trusted_branch_projects',
  workEvidenceSnippets: 'work.evidence_snippets',
  workSessionStartContext: 'work.session_start_context',
  workClassifyNudge: 'work.classify_nudge',
  workSummaryModel: 'work.summary_model',
  workDraftCommitMessages: 'work.draft_commit_messages',
  workDraftBriefs: 'work.draft_briefs',
  workDraftReleaseNotes: 'work.draft_release_notes',
  workCatchUpSummaries: 'work.catch_up_summaries',
  workTidyDoneDays: 'work.tidy_done_days',
  workTidyIdleHours: 'work.tidy_idle_hours',
  workTidyIdleUnlinkedDays: 'work.tidy_idle_unlinked_days',
  workAutoTidy: 'work.auto_tidy',
  workAutoTidyReasons: 'work.auto_tidy_reasons',
  decideJevEnabled: 'decide.jev.enabled',
  decideJevStatusMap: 'decide.jev.status_map',
  decideJevWorkLink: 'decide.jev.work_link',
  decideJevStartProject: 'decide.jev.start_project',
  decideJevSiblingRepos: 'decide.jev.sibling_repos',
  decideJevHostPlacement: 'decide.jev.host_placement',
  decideJevQuickAnswer: 'decide.jev.quick_answer',
  decideJevAdoptTarget: 'decide.jev.adopt_target',
  decideJevRestoreTarget: 'decide.jev.restore_target',
  decideJevDuplicate: 'decide.jev.duplicate',
  decideJevWorkPlacement: 'decide.jev.work_placement',
  decideJevRelatedSession: 'decide.jev.related_session',
  decideJevControlRoute: 'decide.jev.control_route',
  decideJevSummaryCheck: 'decide.jev.summary_check',
  decideJevTurnOutcome: 'decide.jev.turn_outcome',
  decideJevMissionTriage: 'decide.jev.mission_triage',
  decideJevRoutineRunOutcome: 'decide.jev.routine_run_outcome',
  decideJevPrTriage: 'decide.jev.pr_triage',
  decideJevMainTicket: 'decide.jev.main_ticket',
  decideJevTrackerDuplicate: 'decide.jev.tracker_duplicate',
  decideJevUnassigned: 'decide.jev.unassigned',
  decideJevUnassignedReply: 'decide.jev.unassigned_reply',
  decideJevTimeoutMs: 'decide.jev.timeout_ms',
  decideJevBreakerFailures: 'decide.jev.breaker_failures',
  decideJevBreakerOpenSecs: 'decide.jev.breaker_open_secs',
  decideJevDailyTokenBudget: 'decide.jev.daily_token_budget',
  decideJevModel: 'decide.jev.model',
  decideRetentionDays: 'decide.retention_days',
  updateTrack: 'update.track',
  updateHubMode: 'update.hub.mode',
  updateAgentMode: 'update.agent.mode',
  updateDesktopMode: 'update.desktop.mode',
  updateMobileMode: 'update.mobile.mode',
  updateCheckIntervalSecs: 'update.check_interval_secs',
  updateWindow: 'update.window',
  updateRolloutWaveSecs: 'update.rollout_wave_secs',
  updateMirror: 'update.mirror',
  budgetOrgDailyUsd: 'budget.org_daily_usd',
  budgetOrgMonthlyUsd: 'budget.org_monthly_usd',
  notifyDesktop: 'notify.desktop',
  notifyPhone: 'notify.phone',
  notifySound: 'notify.sound',
  notifyQuietHours: 'notify.quiet_hours',
  notifyQuietExcept: 'notify.quiet_except',
} as const;

/** Mirror of `settings::DECIDE_MODES`: what a decision feature's mode may be
 *  (`auto` is not offered, decision D36). */
export const DECIDE_MODES = ['off', 'shadow', 'assist'] as const;

/** Mirror of `settings::UPDATE_TRACKS`: the release track the hub follows. */
export const UPDATE_TRACKS = ['stable', 'beta', 'nightly', 'dev'] as const;
/** Mirror of `settings::UPDATE_MODES`. */
export const UPDATE_MODES = ['manual', 'notify', 'automatic'] as const;
/** Mirror of `settings::UPDATE_MOBILE_MODES`: a phone never installs silently. */
export const UPDATE_MOBILE_MODES = ['manual', 'notify'] as const;

/** Mirror of `settings::DECIDE_JEV_MODELS`. */
export const DECIDE_JEV_MODELS = ['jev-1.13.0', 'jev-latest'] as const;

/** Mirror of `settings::AUTO_TIDY_REASONS`: the tidy reasons auto-tidy may
 *  act on (the ones whose action is a safe kill). */
export const AUTO_TIDY_REASONS = ['done_idle', 'pr_merged_idle', 'not_planned'] as const;

/** The `work.auto_tidy_reasons` comma list as a set of known reasons. */
export function parseAutoTidyReasons(raw: string | undefined): Set<string> {
  const known = new Set<string>(AUTO_TIDY_REASONS);
  return new Set(
    (raw ?? '')
      .split(',')
      .map((r) => r.trim())
      .filter((r) => known.has(r)),
  );
}

/** Toggle one reason in the `work.auto_tidy_reasons` comma list, keeping
 *  the backend's order. */
export function toggleAutoTidyReason(raw: string | undefined, reason: string): string {
  const set = parseAutoTidyReasons(raw);
  if (set.has(reason)) set.delete(reason);
  else set.add(reason);
  return AUTO_TIDY_REASONS.filter((r) => set.has(r)).join(',');
}

/** Derived, read-only entry in the `get_fleet_settings` map: JSON object of
 *  host alias → resolved projects root (setting → env var → default). */
export const PROJECTS_RESOLVED_KEY = 'projects.resolved_base';

/** Derived, read-only entry: `$CLAUDE_FLEET_PROJECTS_BASE` as the app sees it
 *  (trimmed), or `''` when unset. */
export const PROJECTS_LOCAL_ENV_KEY = 'projects.local_env_base';

/** Mirror of `settings::MAX_PATH_LEN`. */
export const BASE_PATH_MAX_LEN = 1024;

/** Mirror of `settings::MAX_SECS` (ten years): the cap on a `Kind::Secs` value. */
export const MAX_SECS = 10 * 365 * 24 * 3600;

/** Mirror of `settings::MOVE_MAX_TRANSCRIPT_MB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_MAX_TRANSCRIPT_MB_MAX = 4096;

/** Mirror of `settings::MOVE_MAX_BUNDLE_MB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_MAX_BUNDLE_MB_MAX = 4096;

/** Mirror of `settings::MOVE_IGNORED_ENTRY_KB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_IGNORED_ENTRY_KB_MAX = 1_048_576;

/** Mirror of `settings::MOVE_IGNORED_TOTAL_MB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_IGNORED_TOTAL_MB_MAX = 1024;

/** Mirror of `settings::MOVE_MAX_SESSION_STATE_MB_MAX` (`Kind::Int { min: 1, max }`). */
export const MOVE_MAX_SESSION_STATE_MB_MAX = 4096;

/** Mirror of the `reports.max_rows` bounds (`Kind::Int { min: 100, max }`). */
export const REPORTS_MAX_ROWS_MIN = 100;
export const REPORTS_MAX_ROWS_MAX = 100_000;
/** Mirror of `settings::MOVE_WAIT_MAX_MINS_MAX` (`Kind::Int { min: 1, max }`): a week. */
export const MOVE_WAIT_MAX_MINS_MAX = 10_080;

export type ProjectsLayout = 'github' | 'flat';

export type SettingKey = (typeof SETTING_KEYS)[keyof typeof SETTING_KEYS];

/** Defaults mirrored from `service::settings::SPECS`, used until the first
 *  `get_fleet_settings` round-trip completes. */
export const SETTING_DEFAULTS: Record<SettingKey, string> = {
  'reconcile.interval_secs': '20',
  'reconcile.stale_working_secs': '1800',
  'reconcile.stale_working_ttl_secs': '86400',
  'sessions.lost_ttl_secs': '1209600',
  'restore.batch_size': '4',
  'restore.stagger_ms': '3000',
  'automation.paused': 'false',
  'automation.daily_budget': '0',
  'orchestrator.enabled': 'true',
  'orchestrator.max_level': '1',
  'playbooks.press_enter': 'false',
  'playbooks.oom_recreate': 'false',
  'playbooks.oom_max_attempts': '2',
  'gc.enabled': 'false',
  'gc.bg_idle_secs': '86400',
  'gc.shell_idle_secs': '604800',
  'gc.work_idle_secs': '0',
  'gc.sweep_interval_secs': '300',
  'gc.external_lost_ttl_secs': '3600',
  'projects.base_path': '{}',
  'projects.layout': 'github',
  'repair.auto_on_tick': 'false',
  'repair.tick_interval_secs': '600',
  'tasks.max_age_secs': '86400',
  'downloads.max_file_mb': '100',
  'downloads.max_total_mb': '2048',
  'downloads.keep_secs': '604800',
  'voice.enabled': 'false',
  'voice.max_capture_secs': '300',
  'voice.claim_ttl_secs': '1800',
  'move.max_transcript_mb': '200',
  'move.max_bundle_mb': '500',
  'move.ignored_entry_kb': '1024',
  'move.ignored_total_mb': '20',
  'move.max_session_state_mb': '200',
  'move.wait_max_mins': '240',
  'usage.enabled': 'true',
  'usage.interval_secs': '300',
  'accounts.pause_at': '90',
  'usage.prices_json': '{}',
  'reports.max_rows': '5000',
  'reports.max_age_secs': '604800',
  'health.context_red_pct': '85',
  'health.version_max_age_secs': '86400',
  'health.disk_low_pct': '90',
  'health.claude_max_behind': '30',
  'health.hooks_silent_secs': '3600',
  'provision.force_git_tree': 'false',
  'provision.install_ag': 'true',
  'work.retention.journal_days': '365',
  'work.retention.tracker_items_days': '180',
  'work.retention.timeline_work_events_days': '180',
  'work.recent_days': '14',
  'work.sync_interval_secs': '300',
  'catalog.scan_check_secs': '3600',
  'catalog.scan_max_age_secs': '86400',
  'catalog.auto': 'true',
  'catalog.auto_push': 'false',
  'work.describe_cache_secs': '300',
  'work.trusted_branch_projects': '[]',
  'work.evidence_snippets': 'true',
  'work.session_start_context': 'false',
  'work.classify_nudge': 'false',
  'work.summary_model': 'haiku',
  'work.draft_commit_messages': 'false',
  'work.draft_briefs': 'false',
  'work.draft_release_notes': 'false',
  'work.catch_up_summaries': 'false',
  'work.tidy_done_days': '2',
  'work.tidy_idle_hours': '4',
  'work.tidy_idle_unlinked_days': '7',
  'work.auto_tidy': 'false',
  'work.auto_tidy_reasons': 'done_idle,pr_merged_idle',
  'decide.jev.enabled': 'false',
  'decide.jev.status_map': 'off',
  'decide.jev.work_link': 'off',
  'decide.jev.start_project': 'off',
  'decide.jev.sibling_repos': 'off',
  'decide.jev.host_placement': 'off',
  'decide.jev.quick_answer': 'off',
  'decide.jev.adopt_target': 'off',
  'decide.jev.restore_target': 'off',
  'decide.jev.duplicate': 'off',
  'decide.jev.work_placement': 'off',
  'decide.jev.related_session': 'off',
  'decide.jev.control_route': 'off',
  'decide.jev.summary_check': 'off',
  'decide.jev.turn_outcome': 'off',
  'decide.jev.mission_triage': 'off',
  'decide.jev.routine_run_outcome': 'off',
  'decide.jev.pr_triage': 'off',
  'decide.jev.main_ticket': 'off',
  'decide.jev.tracker_duplicate': 'off',
  'decide.jev.unassigned': 'false',
  'decide.jev.unassigned_reply': 'false',
  'decide.jev.timeout_ms': '1500',
  'decide.jev.breaker_failures': '5',
  'decide.jev.breaker_open_secs': '300',
  'decide.jev.daily_token_budget': '2000000',
  'decide.jev.model': 'jev-1.13.0',
  'decide.retention_days': '90',
  'update.track': 'stable',
  'update.hub.mode': 'notify',
  'update.agent.mode': 'notify',
  'update.desktop.mode': 'notify',
  'update.mobile.mode': 'notify',
  'update.check_interval_secs': '21600',
  'update.window': '',
  'update.rollout_wave_secs': '3600',
  'update.mirror': 'false',
  'budget.org_daily_usd': '0',
  'budget.org_monthly_usd': '0',
  'notify.desktop': 'needs_you,failed,blocked,routine_failed',
  'notify.phone': 'needs_you,failed,blocked,routine_failed',
  'notify.sound': 'needs_you',
  'notify.quiet_hours': '',
  'notify.quiet_except': 'failed',
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

/** A non-negative integer setting that is a count or size, not seconds
 *  (`Kind::Int`); the default on garbage. */
export function settingInt(map: FleetSettings, key: SettingKey): number {
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

/** An hours input for a seconds setting where `0` means "never". Refuses
 *  what must not be sent silently: an empty or non-numeric value, a negative
 *  one (clamping it would store "never"), and a positive one so small it
 *  rounds to 0 s (also "never"). Values above the cap are passed through so
 *  the backend's range error is what the user sees. */
export function parseHoursInput(raw: string): { secs: number } | { error: string } {
  const t = raw.trim();
  if (t === '') return { error: 'enter a number of hours (0 = never)' };
  const hours = Number(t);
  if (!Number.isFinite(hours)) return { error: `"${t}" is not a number of hours` };
  if (hours < 0) return { error: 'hours must be 0 or more (0 = never)' };
  const secs = Math.round(hours * 3600);
  if (hours > 0 && secs === 0) {
    return { error: 'too small: that rounds to 0 seconds, which means never (enter 0 for never)' };
  }
  return { secs };
}

/** A whole-number input (`Kind::Int`). Refuses an empty or non-integer value
 *  instead of ignoring it; an out-of-range integer is passed through so the
 *  backend's range error is shown. */
/** `work.tidy_idle_unlinked_days`' bounds (`Kind::Int { min: 1, max: 90 }`). */
export const TIDY_IDLE_UNLINKED_DAYS_MIN = 1;
export const TIDY_IDLE_UNLINKED_DAYS_MAX = 90;

/** {@link parseIntInput} within `min..=max`, refused here with a message. */
export function parseBoundedIntInput(raw: string, min: number, max: number): { value: string } | { error: string } {
  const r = parseIntInput(raw);
  if ('error' in r) return r;
  const v = Number.parseInt(r.value, 10);
  if (v < min || v > max) return { error: `must be ${min}–${max}` };
  return r;
}

export function parseIntInput(raw: string): { value: string } | { error: string } {
  const t = raw.trim();
  if (t === '') return { error: 'enter a whole number' };
  if (!/^-?\d+$/.test(t)) return { error: `"${t}" is not a whole number` };
  return { value: String(Number.parseInt(t, 10)) };
}

/** The `usage.prices_json` textarea (`Kind::PriceMap`). Empty means "no
 *  overrides" (`{}`); text that is not a JSON object is refused here with a
 *  message. Anything object-shaped goes to the backend as typed, and its
 *  E_INVALID (bad model key, missing or out-of-range price) is what the row
 *  shows. */
export function parsePricesJsonInput(raw: string): { value: string } | { error: string } {
  const t = raw.trim();
  if (t === '') return { value: '{}' };
  let v: unknown;
  try {
    v = JSON.parse(t);
  } catch {
    return { error: 'not valid JSON' };
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) {
    return { error: 'must be a JSON object of model name to prices' };
  }
  return { value: t };
}
