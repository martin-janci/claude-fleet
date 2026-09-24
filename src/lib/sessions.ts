import { writable } from 'svelte/store';
import { createRowStore } from './row_store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import { readPref, writePref } from './prefs';

/** The `claude_status` vocabulary (pane_intel `ClaudeStatus`). Anything the
 *  backend has not classified arrives as `null`. */
export const CLAUDE_STATUSES = [
  'working',
  'blocked',
  'completed',
  'failed',
  'stopped',
  'idle',
] as const;
export type ClaudeStatus = (typeof CLAUDE_STATUSES)[number];

/** The `stuck_kind` vocabulary (pane_intel `StuckKind`). */
export const STUCK_KINDS = ['auth_menu', 'reconnect', 'trust_prompt', 'oom', 'press_enter'] as const;
export type StuckKind = (typeof STUCK_KINDS)[number];

/** Reduced PR check status populated by reconcile (migration 019). */
export type CiStatus = 'passing' | 'failing' | 'pending';

export interface SessionRow {
  id: number;
  tmux_name: string;
  host_alias: string;
  project_id: number | null;
  worktree_id: number | null;
  created_at: number;
  last_activity_at: number;
  status: string;
  notes: string | null;
  account_uuid: string | null;
  kind: string;
  reviews_session_id: number | null;
  worktree_key: string | null;
  lost_at: number | null;
  /** Why the row was marked lost: "host_reboot" | "tmux_server_gone" |
   *  "missing" | "killed" | null (never lost, or still live). */
  lost_reason?: string | null;
  // Claude agent fields — null when claude CLI not installed or session not managed by Claude Code
  claude_session_id: string | null;
  claude_status: ClaudeStatus | null;
  effort_level: string | null;
  pr_url: string | null;
  current_activity: string | null;
  // Pane-tail intel (migration 012): context window usage 0..100 and the
  // detected stuck state. Both are authoritative when the pane was observed
  // on the last reconcile pass and preserved otherwise.
  context_pct: number | null;
  stuck_kind: StuckKind | null;
  // Display label set by the in-session agent via the `set_friendly_name`
  // MCP tool. When the sidebar toggle is on, this is shown instead of
  // tmux_name; null falls back to tmux_name.
  friendly_name: string | null;
  // Safe-kill flow (migration 017): values "requested" | "ready" | "failed" | null.
  safe_kill_state: string | null;
  safe_kill_nonce: string | null;
  safe_kill_detail: string | null;
  safe_kill_requested_at: number | null;
  // Lifecycle + outcome fields (migration 019), unix seconds unless noted.
  /** When claude_status last entered idle/completed/stopped; null while working. */
  idle_since: number | null;
  /** When the current stuck_kind episode began; null when not stuck. */
  stuck_since: number | null;
  /** When a stuck playbook last acted on this session. */
  last_playbook_at: number | null;
  /** First 200 chars of the last prompt sent through fleet. */
  last_prompt: string | null;
  /** When fleet created the session (null for tmux-discovered rows). */
  started_at: number | null;
  /** Last Stop hook (turn completed). */
  last_turn_at: number | null;
  ci_status: CiStatus | null;
  // Orchestration fields (migration 020).
  /** Completed turns, bumped by every Stop hook. */
  turn_seq: number;
  /** Unix secs of the last Stop hook. */
  last_stop_at: number | null;
  /** Requester session that dispatched the task this session works on. */
  parent_session_id: number | null;
  /** Labels set via `set_session_tags`; empty when none. */
  tags: string[];
  // Token usage + ESTIMATED cost (migration 025), summed from the Claude
  // transcript by the backend every `usage.interval_secs`. The backend
  // always sends them; they are optional here so rows built client-side
  // (tests, optimistic patches) need not spell out zeros — read them with
  // `?? 0` / the helpers below.
  usage_input_tokens?: number;
  usage_output_tokens?: number;
  usage_cache_write_tokens?: number;
  usage_cache_read_tokens?: number;
  /** Estimated cost in millionths of a USD (built-in per-model price table). */
  usage_cost_micros?: number;
  /** Model of the most recent counted message. */
  usage_model?: string | null;
  /** Unix secs the usage totals last changed. */
  usage_updated_at?: number | null;
  // Current-conversation context (migration 037), flattened on the wire.
  /** Model of the current conversation (SessionStart / transcript). */
  model: string | null;
  /** Prompt size of the latest request: input + cache read + cache write. */
  context_tokens: number | null;
  /** Context window of `model` (200 000 or 1 000 000). */
  context_window: number | null;
  /** Who wrote the context value last. */
  context_source: 'transcript' | 'hook' | 'pane' | null;
  /** Unix secs of the last context write. */
  context_at: number | null;
  /** True after a compaction or resume until the next usage line. */
  context_stale: boolean;
  /** tmux pane id (`%17`) reconcile last saw for this row. */
  tmux_pane_id: string | null;
  /** Bumped by the backend on every write (migration 042); orders a
   *  command's return value against a row event. Absent on rows built
   *  client-side and on rows from a hub older than the column. */
  row_version?: number;
  // Pane dialog (migration 040): the permission/question dialog a blocked
  // pane is showing, derived alongside current_activity. Null whenever the
  // pane shows no such dialog.
  pending_input: {
    kind: 'permission' | 'input';
    question: string | null;
    options: { n: number; label: string; selected: boolean }[];
  } | null;
  /** The session's primary work link (migration 046), set through the work
   *  commands (`work.ts`). Absent from a hub older than the work graph. */
  work?: SessionWork | null;
  /** Keys the user said this session does NOT work on (sticky "Not this").
   *  Key recognition (`work_keys.ts`) must not show them. */
  work_rejected?: string[];
  /** The session's top link SUGGESTION (work graph M4): a guess nobody has
   *  decided. Never a work group — only `work` groups a session. */
  work_suggested?: SessionWork | null;
}

/** `SessionRow.work`: the primary link's summary. `key` is the item's key or
 *  the link's own reference; null for an item named by title only. */
export interface SessionWork {
  link_id: number;
  item_id: number | null;
  key: string | null;
  title: string;
  /** `manual` | `started` | `agent` — tolerant: a newer hub may add more. */
  source: string;
  /** The tracker item's status (work graph M3); absent for a bare key, a
   *  local item, or a hub older than M3. `todo` | `in_progress` | `done`. */
  status_category?: string | null;
  /** The tracker's own status name ("In Review"). */
  status_name?: string | null;
  url?: string | null;
  /** The tracker no longer answers for the item (deleted or not visible). */
  unavailable?: boolean;
  /** Work graph M4: `confirmed` | `suggested` (absent from older hubs). */
  state?: string;
  /** `explicit` | `strong` | `weak`. */
  strength?: string | null;
  /** The detection rule that made it (`R3`, `R5` …). */
  rule?: string | null;
  /** A suggestion shown pre-selected. */
  preselected?: boolean;
  /** Live suggestions still to decide. */
  suggestions?: number;
}

type UsageFields = Partial<
  Pick<
    SessionRow,
    'usage_input_tokens' | 'usage_output_tokens' | 'usage_cache_write_tokens' | 'usage_cache_read_tokens'
  >
>;

/** Every token counter of a session summed (0 for a row without usage). */
export function sessionUsageTokens(s: UsageFields): number {
  return (
    (s.usage_input_tokens ?? 0) +
    (s.usage_output_tokens ?? 0) +
    (s.usage_cache_write_tokens ?? 0) +
    (s.usage_cache_read_tokens ?? 0)
  );
}

/** Compact token count: 950 → "950", 1_234 → "1.2k", 123_456 → "123k", 4_560_000 → "4.56M". */
export function formatTokens(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n <= 0) return '0';
  if (n < 1_000) return String(Math.round(n));
  if (n < 10_000) return `${(n / 1_000).toFixed(1)}k`;
  if (n < 1_000_000) return `${Math.round(n / 1_000)}k`;
  if (n < 10_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

/** Estimated cost from micro-USD: "$0.00", "<$0.01", "$1.23", "$1,234". */
export function formatCostMicros(micros: number | null | undefined): string {
  if (micros == null || !Number.isFinite(micros) || micros <= 0) return '$0.00';
  const usd = micros / 1_000_000;
  if (usd < 0.01) return '<$0.01';
  if (usd < 100) return `$${usd.toFixed(2)}`;
  return `$${Math.round(usd).toLocaleString('en-US')}`;
}

// Monotonic guard: a payload carrying a lower row_version than the row we
// hold is a stale snapshot (a command return value that raced a newer
// `session:updated`). Equal versions still apply. A payload without one is
// never rejected for it — only a KNOWN older version is. Shared by the
// `rows` store's `isStale` option and by `loadSessions`'s own list/event
// reconciliation below, so both use exactly the same rule.
function sessionIsStale(incoming: SessionRow, current: SessionRow): boolean {
  return (
    incoming.row_version !== undefined &&
    current.row_version !== undefined &&
    incoming.row_version < current.row_version
  );
}

/** Human label for a ghost row's `lost_reason`, or null when the reason has
 *  no dedicated wording (e.g. "missing", "killed", or none recorded). */
export function lostReasonLabel(reason: string | null | undefined): string | null {
  switch (reason) {
    case 'host_reboot':
      return 'host rebooted';
    case 'tmux_server_gone':
      return 'tmux server stopped';
    default:
      return null;
  }
}

const rows = createRowStore<SessionRow, number>({
  key: (s) => s.id,
  // Both the optimistic `removeSession()` and the `session:killed` event
  // delete a row; a `session:updated` still in flight for that id would
  // otherwise re-insert the dead row ("ghost session").
  tombstoneMs: 5000,
  isStale: sessionIsStale,
});
export const sessions = rows.store;
export const resetTombstonesForTests = rows.resetTombstonesForTests;

/** True once the first successful `list_sessions` has populated the store.
 *  Consumers that react to *transitions* (Attention.svelte) treat everything
 *  before this as baseline, so a launch never replays every already-stuck
 *  row as a fresh alert. */
export const sessionsLoaded = writable<boolean>(false);

// Sidebar filter — when false, background (`kind === 'bg'`) sessions are
// hidden from the tree. Defaults to true (shown). Persisted across restarts.
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';
export const showBgAgents = writable<boolean>(readPref('show-bg-agents', true, isBool));
showBgAgents.subscribe((v) => writePref('show-bg-agents', v));

// Sidebar display toggle — when true, sessions render their agent-set
// `friendly_name` (falling back to `tmux_name` when unset) instead of the raw
// tmux_name. Defaults to true so a newly populated friendly_name is visible
// without the user having to discover the toggle. Persisted across restarts.
export const showFriendlyNames = writable<boolean>(
  readPref('show-friendly-names', true, isBool),
);
showFriendlyNames.subscribe((v) => writePref('show-friendly-names', v));

// Sidebar density toggle — when true, session rows show their second
// (details) line: host, tmux name / worktree, elapsed, badges, last prompt.
export const showRowDetails = writable<boolean>(readPref('rows.details', true, isBool));
showRowDetails.subscribe((v) => writePref('rows.details', v));

// Sidebar grouping — `project` (the tree by repository) or `work` (sessions
// that carry a work key grouped by it first, the rest still under their
// project; see work_keys.ts). Persisted across restarts.
export type SidebarGroupBy = 'project' | 'work';
const isGroupBy = (v: unknown): v is SidebarGroupBy => v === 'project' || v === 'work';
export const sidebarGroupBy = writable<SidebarGroupBy>(readPref('sidebar.group', 'project', isGroupBy));
sidebarGroupBy.subscribe((v) => writePref('sidebar.group', v));

// `force: true` (the sidebar Refresh button) makes the backend run a fleet
// reconcile pass now; the default returns stored rows while the last pass is
// within the configured interval, so window-focus reloads stay cheap.
export async function loadSessions(opts: { force?: boolean } = {}): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions', { force: opts.force ?? false });
  if (r.ok) {
    // The list owns ORDER (the backend's `ORDER BY last_activity_at DESC`);
    // events own CONTENT. Rebuilding from the current store's position would
    // freeze every row at wherever it first landed — position has to be
    // taken from the list every time, and content still has to lose to a
    // `session:updated` that raced this call and is strictly newer.
    sessions.update((cur) => {
      const byId = new Map(cur.map((s) => [s.id, s] as const));
      const next: SessionRow[] = [];
      for (const listed of r.value) {
        if (rows.isTombstoned(listed.id)) continue;
        const current = byId.get(listed.id);
        next.push(current && sessionIsStale(listed, current) ? current : listed);
      }
      return next;
    });
    sessionsLoaded.set(true);
  }
  return r;
}

export async function killSession(hostAlias: string, name: string): Promise<Result<number>> {
  const r = await invokeCmd<number>('kill_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) removeSession(r.value);
  return r;
}

/** Ask Claude to safely persist all work, then delete the worktree and kill
 *  the tmux session. The command returns the row with `safe_kill_state =
 *  "requested"`; subsequent transitions ("ready" → row will be killed via the
 *  Stop hook; "failed" → user must resolve) arrive via `session:updated`. */
export async function safeKillSession(
  hostAlias: string,
  tmuxName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('safe_kill_session', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export interface DirtyFile {
  status: string;
  path: string;
}

export interface SafeKillInspection {
  has_worktree: boolean;
  worktree_path: string | null;
  branch: string | null;
  upstream: string | null;
  dirty_files: DirtyFile[];
  unpushed_commits: number;
  safe_to_remove: boolean;
  error: string | null;
}

/** Pre-flight: cheap git inspect that drives the safe-remove dialog. */
export async function inspectSafeKill(
  hostAlias: string,
  tmuxName: string,
): Promise<Result<SafeKillInspection>> {
  return invokeCmd<SafeKillInspection>('inspect_safe_kill', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
  });
}

/** Direct remove. `force=false` errors out if anything is dirty — only safe
 *  when the inspection said `safe_to_remove`. `force=true` is the explicit
 *  "discard local work and kill" path. */
export async function discardKillSession(
  hostAlias: string,
  tmuxName: string,
  force: boolean,
): Promise<Result<number>> {
  const r = await invokeCmd<number>('discard_kill_session', {
    args: { host_alias: hostAlias, tmux_name: tmuxName },
    force,
  });
  if (r.ok) removeSession(r.value);
  return r;
}

export async function renameSession(
  hostAlias: string,
  oldName: string,
  newName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', {
    args: { host_alias: hostAlias, old_name: oldName, new_name: newName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/**
 * Set the session's display label (`friendly_name`). An empty or
 * whitespace-only value clears it, so the row falls back to the tmux name.
 * Unlike `renameSession` this never touches tmux and keeps the row id.
 */
export async function setFriendlyName(
  hostAlias: string,
  tmuxName: string,
  friendlyName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('set_session_friendly_name', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, friendly_name: friendlyName },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export async function restartSession(hostAlias: string, name: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** What `repair_session` found and did (mirrors `service::repair::RepairReport`). */
export interface RepairReport {
  session_id: number | null;
  host_alias: string;
  tmux_name: string;
  /** Project root the repair resolved on the host (shows which base path / setting was used). */
  project_root: string;
  /** The verified directory the pane runs in (user-facing form). */
  cwd: string;
  /** The same directory as the host resolves it (`pwd -P`), when known. */
  cwd_physical: string | null;
  /** True when nothing needed doing. */
  healthy: boolean;
  /** Ordered, human-readable actions that were applied. */
  actions: string[];
  warnings: string[];
  /** Automatic check only: the workspace needs an explicit repair (Repair
   *  workspace); nothing git-side was applied. `deferred` says what it would do. */
  needs_explicit_repair: boolean;
  deferred: string[];
  /** `branch_local` | `branch_remote` | `branch_from_base:<start>` when a worktree was (re)created. */
  branch_source: string | null;
  /** `created` | `respawned` when tmux was touched. */
  tmux: string | null;
  tmux_alive: boolean;
  /** The tmux session is confirmed gone (not merely unknown). */
  tmux_dead: boolean;
  /** A live pane's reported working directory no longer exists. */
  tmux_cwd_stale: boolean;
  worktree_row_updated: boolean;
  /** Alive sessions on the same host sharing this workspace. */
  sibling_session_ids: number[];
  /** Set when this worktree's registered directory was found missing: which
   *  conditions of the automatic stale-entry removal held. */
  vanished_guard?: VanishedGuard | null;
}

/** Mirrors `service::repair::VanishedGuard`: an automatic repair may drop the
 *  worktree's own stale registration only when every field is true. */
export interface VanishedGuard {
  dir_absent: boolean;
  parent_exists: boolean;
  same_filesystem: boolean;
  repo_ok: boolean;
  under_root: boolean;
  not_locked: boolean;
  no_other_session: boolean;
  /** No other worktree under the same parent is missing too. */
  siblings_present: boolean;
  /** The parent's dev:inode equals the one recorded while healthy. */
  fingerprint_matches: boolean;
  /** `match` | `mismatch` | `missing` | `stat_failed`. */
  fingerprint_check: string;
}

/** Make the session's directory a healthy git worktree on its branch and its
 *  tmux session run there (recreating tmux when it is gone). A no-op on a
 *  healthy session; the backend emits the row events for anything it fixed,
 *  so nothing is merged here. */
export async function repairSession(
  sessionId: number,
  opts: {
    /** `true` only for the Repair workspace button: an explicit repair that
     *  may unregister a stale entry, adopt a moved checkout, recreate the
     *  branch and respawn a live pane. Default `false`: the automatic check,
     *  which only creates what is confirmed missing. */
    explicit?: boolean;
  } = {},
): Promise<Result<RepairReport>> {
  return invokeCmd<RepairReport>('repair_session', {
    args: { session_id: sessionId, explicit: opts.explicit ?? false },
  });
}

export interface NewSessionArgs {
  host_alias: string;
  project_id: number;
  worktree_id: number | null;
  name: string;
  new_worktree?: string | null;
  /** Branch to fork a new worktree from; null/empty = repo default branch. */
  base_branch?: string | null;
  /** "work" (default) runs Claude Code; "shell" runs a plain login shell. */
  kind?: 'work' | 'shell';
  /** Optional command run on start for a shell session (null = bare shell). */
  start_command?: string | null;
  /**
   * Optional user-supplied sidebar label. When omitted or empty, the backend
   * derives one from the branch name so the sidebar never shows the raw
   * `dev-<owner>-<repo>--…` slug.
   */
  friendly_name?: string | null;
  /**
   * Resume this Claude conversation id instead of minting a fresh one (from
   * discover_lost_sessions). Rejected for a shell session and when a session
   * on the host already holds that conversation.
   */
  resume_claude_session_id?: string;
}

export async function newSessionAbortable(
  args: NewSessionArgs,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>('new_session', { args }, signal);
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

// ─── identity ────────────────────────────────────────────────────────────────

/** The stable identity of a session. `id` is the primary key; the
 *  host_alias + tmux_name pair is the fallback for rows whose id churned on
 *  re-discovery. A bare tmux_name is NOT an identity — default names are
 *  project-derived, so the same name on two hosts is the normal case. */
export interface SessionIdentity {
  id?: number | null;
  host_alias: string;
  tmux_name: string;
}

/** True when `a` and `b` denote the same session: same id, or (when either
 *  side has no usable id) same host_alias + tmux_name. */
export function sameSession(a: SessionIdentity, b: SessionIdentity): boolean {
  if (a.id != null && b.id != null) return a.id === b.id;
  return a.host_alias === b.host_alias && a.tmux_name === b.tmux_name;
}

/** Locate `ident` in `arr`: by id first, then by the host+name pair. */
export function findSession(arr: SessionRow[], ident: SessionIdentity): SessionRow | undefined {
  if (ident.id != null) {
    const byId = arr.find((s) => s.id === ident.id);
    if (byId) return byId;
  }
  return arr.find((s) => s.host_alias === ident.host_alias && s.tmux_name === ident.tmux_name);
}

export function mergeSession(row: SessionRow): void {
  rows.merge(row);
}

export function removeSession(id: number): void {
  rows.remove(id);
}

/** One backend row event, as delivered by `events.ts`. */
export type SessionEvent =
  | { type: 'created' | 'updated'; row: SessionRow }
  | { type: 'killed'; id: number };

/** Apply a burst of session events in ONE store update. The reconcile tick
 *  emits `session:updated` once per session, so without batching every tick
 *  costs N store flushes (and N sidebar re-derives). Events are applied in
 *  order, so a `killed` after an `updated` for the same id still removes the
 *  row, and an `updated` after a `killed` is dropped by the tombstone. */
export function applySessionEvents(events: readonly SessionEvent[]): void {
  if (events.length === 0) return;
  sessions.update((arr) => {
    let next = arr;
    for (const ev of events) {
      next = ev.type === 'killed' ? rows.removeFrom(next, ev.id) : rows.mergeInto(next, ev.row);
    }
    return next;
  });
}

/** Apply a row returned by a mutation command (rename/restart/new). Unlike an
 *  event, a command result is the authoritative response to a request the
 *  user just made, so it clears any tombstone for that id before merging. */
export function acceptCommandRow(row: SessionRow | null | undefined): void {
  rows.accept(row);
}

/**
 * Type `prompt` into a session's REPL and submit it.
 *
 * `opts.keys` presses one key instead — `Enter`, `Escape`, `C-c`, or a digit
 * `1`-`9` that picks that option of a `pending_input` dialog. A key is never
 * marked untrusted and is never recorded as a prompt, and `prompt` must be
 * empty alongside it (the backend refuses the pair with `E_VALIDATE`).
 * Answering a dialog has to go this way. The text path pastes through
 * `paste-buffer -p`, and the REPL has bracketed paste on (DECSET 2004), so
 * the pane receives `ESC [ 2 0 0 ~ 3 ESC [ 2 0 1 ~` — the first key a select
 * dialog sees is ESC, which cancels it. `send-keys 3` delivers one raw `3`.
 */
export async function sendPrompt(
  hostAlias: string,
  tmuxName: string,
  prompt: string,
  opts: { keys?: string } = {},
): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt, keys: opts.keys ?? null },
  });
}

export const DEFAULT_REVIEW_PROMPT = `Review the work in this worktree. Run \`git diff\` and \`git log\` against the base branch to see what changed.

Pass 1 — correctness: does the code do what it should? Any bugs?
Pass 2 — code quality: clarity, structure, test coverage.
Pass 3 — risk: anything dangerous, security-sensitive, or destructive?

Cite file:line for every point. End with an overall verdict: approve / approve-with-fixes / needs-rework.`;

export async function spawnReview(
  sourceSessionId: number,
  prompt: string,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>(
    'spawn_review',
    { args: { source_session_id: sourceSessionId, prompt } },
    signal,
  );
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function recreateSession(sessionId: number): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('recreate_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

/** Mirrors `service::sessions::restore::RestorePlanEntry`: one planned restore
 *  action. `action` is `"restore"` for a session the batch will attempt to
 *  resume, `"skip"` (with `reason` set) for one an explicit `sessionIds`
 *  request named that cannot be restored. */
export interface RestorePlanEntry {
  session_id: number;
  tmux_name: string | null;
  cwd: string | null;
  claude_session_id: string | null;
  friendly_name: string | null;
  action: 'restore' | 'skip';
  reason: string | null;
}

/** Mirrors `service::sessions::restore::RestoreOutcome`: the result of one
 *  restore attempt. */
export interface RestoreOutcome {
  session_id: number;
  tmux_name: string;
  ok: boolean;
  error: string | null;
}

/** Mirrors `service::sessions::restore::RestoreReport`. */
export interface RestoreReport {
  host_alias: string;
  dry_run: boolean;
  plan: RestorePlanEntry[];
  results: RestoreOutcome[];
}

/** Batch-restore a host's sessions lost to a reboot or a tmux server restart,
 *  over `recreate_session`. Pass `dryRun: true` first to get the plan (no
 *  ssh, no writes); `sessionIds` restricts the batch to those fleet session
 *  ids instead of every lost, resumable session on the host. The backend
 *  emits `session:updated` row events for anything it restores, so nothing
 *  is merged into the sessions store here. */
export async function restoreHostSessions(
  hostAlias: string,
  opts: { dryRun?: boolean; sessionIds?: number[] } = {},
): Promise<Result<RestoreReport>> {
  return invokeCmd<RestoreReport>('restore_host_sessions', {
    args: {
      host_alias: hostAlias,
      dry_run: opts.dryRun ?? false,
      session_ids: opts.sessionIds ?? null,
    },
  });
}

/** Mirrors `service::sessions::discover::LostCandidate`: one transcript the
 *  host has (possibly already held by a fleet row — `existing_session_id`),
 *  ranked and enriched from the store. `rank_hint` is relative to the host's
 *  last boot. `resumable` is true only when `new_session` would start the
 *  pane in exactly `cwd` (a registered worktree or the project root) —
 *  anywhere else `claude --resume` misses the transcript and a new, empty
 *  conversation starts instead. `derived_tmux_name` is set only when
 *  `resumable`, and is a hint for `new_session`'s `name` — it may already be
 *  taken by a second session on the same worktree. Restore a resumable
 *  candidate with `new_session({ hostAlias, projectId, worktreeId, name:
 *  derivedTmuxName, resumeClaudeSessionId: claudeSessionId })`. */
export interface LostCandidate {
  cwd: string;
  git_branch: string | null;
  claude_session_id: string;
  transcript_mtime: number;
  derived_tmux_name: string | null;
  project_id: number | null;
  worktree_id: number | null;
  existing_session_id: number | null;
  rank_hint: 'before_boot' | 'after_boot' | 'stale' | 'unknown';
  resumable: boolean;
}

/** Scan a host's Claude transcripts (`~/.claude/projects`) for lost
 *  conversations (one a fleet row already holds is flagged via
 *  `existing_session_id`) and rank/enrich them from the store. Read-only: no
 *  writes, nothing to merge into the sessions store. `limit` caps how many
 *  transcripts (newest first) are read; omit for the backend default (50). */
export async function discoverLostSessions(
  hostAlias: string,
  limit?: number,
): Promise<Result<LostCandidate[]>> {
  return invokeCmd<LostCandidate[]>('discover_lost_sessions', {
    args: { host_alias: hostAlias, limit: limit ?? null },
  });
}

export async function dismissGhostSession(sessionId: number): Promise<Result<void>> {
  const r = await invokeCmd<void>('dismiss_ghost_session', {
    args: { session_id: sessionId },
  });
  if (r.ok) removeSession(sessionId);
  return r;
}

// ─── background sessions ─────────────────────────────────────────────────────

export interface NewBgSessionResult {
  claude_session_id: string | null;
  /** The fleet row, registered by the post-launch reconcile (MCP-7). Null
   *  when the agent could not be matched yet — it appears on the next tick. */
  session?: SessionRow | null;
  warning?: string | null;
}

/** Launch a supervised Claude background session on `hostAlias`. */
export async function newBgSession(
  hostAlias: string,
  name: string,
  prompt: string,
  /** The session asking for this one; the new row becomes its child. Null
   *  from the desktop dialog — nobody asked for it from inside a session. */
  requesterSessionId: number | null = null,
): Promise<Result<NewBgSessionResult>> {
  const r = await invokeCmd<NewBgSessionResult>('new_bg_session', {
    args: {
      host_alias: hostAlias,
      name,
      prompt,
      requester_session_id: requesterSessionId,
    },
  });
  if (r.ok && r.value?.session) acceptCommandRow(r.value.session);
  return r;
}

/** True for a row with no attached tmux pane: a supervised background agent
 *  (`bg`) or an interactive Claude session running outside fleet entirely
 *  (`external`, e.g. Claude Desktop). Every "no PTY" check in the app should
 *  go through this instead of comparing `kind` directly. */
export function hasNoPane(s: Pick<SessionRow, 'kind'>): boolean {
  return s.kind === 'bg' || s.kind === 'external';
}

/** True for a background agent whose CLI process is gone (backend marks it
 *  `claude_status: 'stopped'` once its transcript has been quiet past
 *  `AGENT_INACTIVE_SECS`). Never true for `external` rows — those leave the
 *  list on their own when the process ends. */
export function isInactiveAgent(s: Pick<SessionRow, 'kind' | 'claude_status'>): boolean {
  return s.kind === 'bg' && s.claude_status === 'stopped';
}

/** Remove a `bg` agent row from the list without touching the underlying
 *  process (it does not use fleet). Refused by the backend for `external`
 *  rows and for a row still `working`. The row itself is removed by the
 *  `session:removed` event the backend emits, not by this call. */
export async function dismissAgentSession(sessionId: number): Promise<Result<null>> {
  return invokeCmd<null>('dismiss_agent_session', {
    args: { session_id: sessionId },
  });
}

/** Outcome of `purge_project` on one host (mirrors Rust `claude_cli::PurgeReport`). */
export interface PurgeReport {
  host_alias: string;
  /** The path fleet recorded from the projects scan. */
  logical_path: string;
  /** `pwd -P` on the target host; null when the directory no longer exists. */
  physical_path: string | null;
  /** Path forms whose Claude state was deleted. */
  purged: string[];
  /** Path forms Claude held no state for (not an error). */
  not_found: string[];
}

/** Delete Claude Code state for a project on every host in `hostAliases`
 *  (under both its physical and logical path forms). The backend removes the
 *  project from the DB only if every host succeeds; one report per host. */
export async function purgeProject(
  hostAliases: string[],
  projectPath: string,
  projectId: number,
): Promise<Result<PurgeReport[]>> {
  return invokeCmd<PurgeReport[]>('purge_project', {
    args: { host_aliases: hostAliases, project_path: projectPath, project_id: projectId },
  });
}
