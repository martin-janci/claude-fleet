// Tidy-up (work graph M7): what fleet suggests cleaning up, and the user's
// choices. The backend plans (`work { action: tidy }`) and applies
// (`work_link { action: tidy_apply }`); this module holds the last report,
// the vocabulary, and the pure helpers the sheet and Attention use.
//
// Nothing here acts on its own: every destructive action is a person's
// confirm in the sheet (or the backend's opt-in auto-tidy).

import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { acceptCommandRow, type SessionRow } from './sessions';
import { ALL_SCOPES } from './orgs';

/** `service::gc::tidy::TidyReason` — tolerant of values a newer hub adds. */
export type TidyReason =
  | 'done_idle'
  | 'pr_merged_idle'
  | 'not_planned'
  | 'duplicate_worktree'
  | 'ghost_expiring'
  | (string & {});

/** `service::gc::tidy::TidyAction`. */
export type TidyAction = 'archive' | 'safe_kill' | 'kill' | 'resume_or_expire' | (string & {});

/** The sheet's per-row choice: an action, or leaving the row alone. */
export type TidyChoice = 'safe_kill' | 'kill' | 'archive' | 'snooze' | 'never';

/** Reason order: the backend's ranking, the sheet's group order. */
export const TIDY_REASONS = [
  'done_idle',
  'pr_merged_idle',
  'not_planned',
  'duplicate_worktree',
  'ghost_expiring',
] as const;

export const TIDY_REASON_LABELS: Record<string, string> = {
  done_idle: 'Done and idle',
  pr_merged_idle: 'PR merged, idle',
  not_planned: "Won't do / duplicate",
  duplicate_worktree: 'Duplicate on one worktree',
  ghost_expiring: 'Lost session about to expire',
};

export const TIDY_CHOICE_LABELS: Record<TidyChoice, string> = {
  safe_kill: 'Safe kill',
  kill: 'Kill',
  archive: 'Archive only',
  snooze: 'Snooze 7 d',
  never: 'Never for this work',
};

export function tidyReasonLabel(r: string): string {
  return TIDY_REASON_LABELS[r] ?? r.replace(/_/g, ' ');
}

/** One suggestion (`service::gc::tidy::TidyCandidate`). */
export interface TidyCandidate {
  session_id: number;
  link_id?: number | null;
  host_alias: string;
  tmux_name: string;
  kind?: string;
  reason: TidyReason;
  secondary?: TidyReason[];
  action: TidyAction;
  since: number;
  label?: string | null;
  key?: string | null;
  item_status?: string | null;
  branch?: string | null;
  pr_url?: string | null;
  idle_secs?: number;
  expires_at?: number | null;
  archived?: boolean;
  /** Auto-tidy (when on) would act on it. */
  auto?: boolean;
}

/** `work { action: tidy }`. */
export interface TidyReport {
  candidates: TidyCandidate[];
  auto_tidy: boolean;
  auto_reasons: string[];
  done_days: number;
  idle_hours: number;
}

export interface TidyApplyItem {
  session_id: number;
  action: TidyChoice;
  link_id?: number | null;
  days?: number;
}

export interface TidyApplyResult {
  session_id: number;
  action: string;
  ok: boolean;
  outcome?: string | null;
  error?: string | null;
}

/** Work open again with past sessions (`store::ReopenedWork`). */
export interface ReopenedWork {
  item_id: number;
  key?: string | null;
  title: string;
  status_name?: string | null;
  url?: string | null;
  reopened_at: number;
  past_sessions: number;
  live_sessions?: number;
  last_host?: string | null;
}

export const EMPTY_REPORT: TidyReport = {
  candidates: [],
  auto_tidy: false,
  auto_reasons: [],
  done_days: 2,
  idle_hours: 4,
};

/** The last tidy report and reopened list (refreshed by `refreshTidy`). */
export const tidyReport = writable<TidyReport>(EMPTY_REPORT);
export const reopenedWork = writable<ReopenedWork[]>([]);
/** Successful reads of the reopened list: the first is the baseline a
 *  "reopened" toast is measured against. */
export const reopenedLoads = writable(0);

/** Re-read the candidates and the reopened list. Errors keep the last value
 *  (an older hub without the actions answers E_INVALID: nothing to show). */
export async function refreshTidy(): Promise<void> {
  const [t, r] = await Promise.all([
    invokeCmd<TidyReport>('work_tidy'),
    invokeCmd<ReopenedWork[]>('work_reopened'),
  ]);
  if (t.ok && t.value) tidyReport.set({ ...EMPTY_REPORT, ...t.value });
  if (r.ok && Array.isArray(r.value)) {
    reopenedWork.set(r.value);
    reopenedLoads.update((n) => n + 1);
  }
}

/** Items of `list` not in `seen` (a newly reopened item toasts once). */
export function newlyReopened(seen: ReadonlySet<number>, list: ReopenedWork[]): ReopenedWork[] {
  return list.filter((w) => !seen.has(w.item_id));
}

// ── pure helpers ──

/** The choices a candidate's row offers. A kill is offered only for a live
 *  session; a plain kill only where the backend itself chose one (a shared
 *  worktree, bg / shell / review rows) — everywhere else a kill is the safe
 *  kill. Snooze / never / archive need a link to live on. */
export function choicesFor(c: TidyCandidate): TidyChoice[] {
  const out: TidyChoice[] = [];
  if (c.action === 'safe_kill') out.push('safe_kill');
  if (c.action === 'kill') out.push('kill');
  if (c.link_id != null) {
    if (c.action !== 'resume_or_expire' && !c.archived) out.push('archive');
    out.push('snooze', 'never');
  }
  return out;
}

/** The preselected choice: the backend's action when the row offers it
 *  (`resume_or_expire` preselects nothing destructive: snooze). */
export function defaultChoice(c: TidyCandidate): TidyChoice | null {
  const choices = choicesFor(c);
  if ((choices as string[]).includes(c.action)) return c.action as TidyChoice;
  return choices[0] ?? null;
}

/** Whether a row starts ticked: every row whose default is an action,
 *  except a lost session (Resume is its own button). */
export function preselected(c: TidyCandidate): boolean {
  return c.action !== 'resume_or_expire' && defaultChoice(c) !== null;
}

/** Candidates grouped by primary reason, in reason order. */
export function groupByReason(cands: TidyCandidate[]): { reason: string; items: TidyCandidate[] }[] {
  const order = new Map<string, number>(TIDY_REASONS.map((r, i) => [r, i]));
  const groups = new Map<string, TidyCandidate[]>();
  for (const c of cands) {
    const list = groups.get(c.reason) ?? [];
    list.push(c);
    groups.set(c.reason, list);
  }
  return [...groups.entries()]
    .sort((a, b) => (order.get(a[0]) ?? 99) - (order.get(b[0]) ?? 99))
    .map(([reason, items]) => ({ reason, items }));
}

/** "4 h", "2 d". */
export function formatIdle(secs: number | undefined | null): string {
  const s = Math.max(0, secs ?? 0);
  if (s >= 86_400) return `${Math.floor(s / 86_400)} d`;
  if (s >= 3_600) return `${Math.floor(s / 3_600)} h`;
  return `${Math.max(1, Math.floor(s / 60))} min`;
}

/** The items to send for the ticked rows. */
export function applyItems(
  cands: TidyCandidate[],
  ticked: ReadonlySet<number>,
  choice: ReadonlyMap<number, TidyChoice>,
): TidyApplyItem[] {
  const out: TidyApplyItem[] = [];
  for (const c of cands) {
    if (!ticked.has(c.session_id)) continue;
    const action = choice.get(c.session_id) ?? defaultChoice(c);
    if (!action) continue;
    const item: TidyApplyItem = { session_id: c.session_id, action };
    if (c.link_id != null) item.link_id = c.link_id;
    if (action === 'snooze') item.days = 7;
    out.push(item);
  }
  return out;
}

// ── commands ──

export async function applyTidy(items: TidyApplyItem[]): Promise<Result<{ results: TidyApplyResult[] }>> {
  const r = await invokeCmd<{ results: TidyApplyResult[] }>('tidy_apply', { args: { items } });
  void refreshTidy();
  return r;
}

async function rowCmd(cmd: string, args: Record<string, unknown>): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>(cmd, { args });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Collapse a live session into its group's Done (tmux keeps running). */
export function archiveSession(sessionId: number): Promise<Result<SessionRow>> {
  return rowCmd('archive_session_work', { session_id: sessionId });
}

/** Un-archive (one click) — also the touch an attach sends. */
export function unarchiveSession(sessionId: number): Promise<Result<SessionRow>> {
  return rowCmd('unarchive_session_work', { session_id: sessionId });
}

export async function dismissReopened(itemId: number): Promise<Result<{ dismissed: boolean }>> {
  const r = await invokeCmd<{ dismissed: boolean }>('dismiss_reopened', { args: { item_id: itemId } });
  if (r.ok) reopenedWork.update((list) => list.filter((w) => w.item_id !== itemId));
  return r;
}

// ── group-by-work (M7.3) ──

/** A work group's sessions split into those shown and those archived into
 *  its Done section. A row that needs the user is never collapsed away,
 *  archived or not. */
export function splitArchived<T extends { work?: { archived_at?: number | null } | null }>(
  sessions: T[],
  needsYou: (s: T) => boolean = () => false,
): { live: T[]; archived: T[] } {
  const live: T[] = [];
  const archived: T[] = [];
  for (const s of sessions) {
    if (s.work?.archived_at != null && !needsYou(s)) archived.push(s);
    else live.push(s);
  }
  return { live, archived };
}

/** Reopened work by key, for the group headers' badge. */
export function reopenedByKey(list: ReopenedWork[]): Map<string, ReopenedWork> {
  const out = new Map<string, ReopenedWork>();
  for (const w of list) if (w.key) out.set(w.key, w);
  return out;
}

/** "reopened · 2 past sessions". */
export function reopenedBadge(w: ReopenedWork): string {
  return `reopened · ${w.past_sessions} past session${w.past_sessions === 1 ? '' : 's'}`;
}

/** Dry run: the candidates auto-tidy would act on with `reasons` allowed —
 *  whether or not it is on (the backend's `auto` flag says only "on and
 *  allowed"). Safe kill or archive only, never a plain kill. */
export function autoTidyPreview(cands: TidyCandidate[], reasons: ReadonlySet<string>): TidyCandidate[] {
  return cands.filter((c) => (c.action === 'safe_kill' || c.action === 'archive') && reasons.has(c.reason));
}

/** Candidates whose session is in `scope` (`all` keeps every one; a
 *  candidate whose row is not loaded is kept). */
export function inScope(
  cands: TidyCandidate[],
  rows: readonly SessionRow[],
  scope: string,
  scopeOf: (s: SessionRow) => string,
): TidyCandidate[] {
  if (scope === ALL_SCOPES) return cands;
  const byId = new Map(rows.map((r) => [r.id, r]));
  return cands.filter((c) => {
    const r = byId.get(c.session_id);
    return !r || scopeOf(r) === scope;
  });
}

/**
 * A request to open the Tidy-up sheet from elsewhere (the Today view's Stale
 * section, work graph M9): `sessionIds` are ticked and the cursor starts on
 * the first of them; an empty list keeps the sheet's own preselection. The
 * sheet lives in the sidebar, so App expands a collapsed sidebar on a
 * request, and the sheet takes it once it has candidates. A request older
 * than {@link TIDY_REQUEST_TTL_MS} is dropped rather than opening the sheet
 * out of the blue later.
 */
export interface TidyRequest {
  sessionIds: number[];
  at: number;
}

export const TIDY_REQUEST_TTL_MS = 15_000;

export const tidyRequest = writable<TidyRequest | null>(null);

export function requestTidy(sessionIds: number[] = [], now: number = Date.now()): void {
  tidyRequest.set({ sessionIds, at: now });
}

/** Whether a request is still worth honouring at `now`. */
export function tidyRequestLive(r: TidyRequest | null, now: number = Date.now()): r is TidyRequest {
  return r !== null && now - r.at <= TIDY_REQUEST_TTL_MS;
}

/** Which rows a sheet opened by `r` ticks: the requested ones it can act on,
 *  or — with none requested — the usual preselection. */
export function requestedTicks(cands: readonly TidyCandidate[], r: TidyRequest): Set<number> {
  if (r.sessionIds.length === 0) return new Set(cands.filter(preselected).map((c) => c.session_id));
  const want = new Set(r.sessionIds);
  return new Set(
    cands.filter((c) => want.has(c.session_id) && choicesFor(c).length > 0).map((c) => c.session_id),
  );
}

/** The candidates behind the given sessions (the Today view's Stale rows). */
export function candidatesFor(cands: readonly TidyCandidate[], sessionIds: readonly number[]): TidyCandidate[] {
  const ids = new Set(sessionIds);
  return cands.filter((c) => ids.has(c.session_id));
}
