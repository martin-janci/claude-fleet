import { writable } from 'svelte/store';
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

/** Reduced PR check status populated by reconcile (migration 018). */
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
  // Lifecycle + outcome fields (migration 018), unix seconds unless noted.
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
}

export const sessions = writable<SessionRow[]>([]);

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

// `force: true` (the sidebar Refresh button) makes the backend run a fleet
// reconcile pass now; the default returns stored rows while the last pass is
// within the configured interval, so window-focus reloads stay cheap.
export async function loadSessions(opts: { force?: boolean } = {}): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions', { force: opts.force ?? false });
  if (r.ok) sessions.set(r.value);
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

export async function restartSession(hostAlias: string, name: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) acceptCommandRow(r.value);
  return r;
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
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('new_session', { args });
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export async function newSessionAbortable(
  args: NewSessionArgs,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>('new_session', { args }, signal);
  if (r.ok) acceptCommandRow(r.value);
  return r;
}

export async function bootstrapSessions(): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
  if (r.ok) sessions.set(r.value);
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

// Recently-removed session ids. Both the optimistic `removeSession()` and the
// `session:killed` event delete a row; without a tombstone, a `session:updated`
// event still in flight for that id would re-insert the dead row ("ghost
// session"). Entries expire so a genuinely new id is never blocked.
const tombstones = new Map<number, number>();
const TOMBSTONE_MS = 5000;

/** Test hook: forget every tombstone so one test's kill can't shadow the
 *  next test's merge of the same id. Not for production code. */
export function resetTombstonesForTests(): void {
  tombstones.clear();
}

function isTombstoned(id: number): boolean {
  const t = tombstones.get(id);
  if (t === undefined) return false;
  if (Date.now() - t > TOMBSTONE_MS) {
    tombstones.delete(id);
    return false;
  }
  return true;
}

/** Pure merge step shared by the single-row and batched paths. Returns the
 *  input array untouched when the row is tombstoned or stale. */
function mergeInto(arr: SessionRow[], row: SessionRow): SessionRow[] {
  if (!row) return arr;
  if (isTombstoned(row.id)) return arr;
  const i = arr.findIndex((s) => s.id === row.id);
  if (i === -1) return [...arr, row];
  // Monotonic guard: don't let a staler payload (e.g. a command return
  // value that raced a newer `session:updated` event) clobber a fresher
  // row. Equal timestamps still apply — they may carry a status change.
  if (row.last_activity_at < arr[i].last_activity_at) return arr;
  const next = arr.slice();
  next[i] = row;
  return next;
}

function removeFrom(arr: SessionRow[], id: number): SessionRow[] {
  tombstones.set(id, Date.now());
  const next = arr.filter((s) => s.id !== id);
  return next.length === arr.length ? arr : next;
}

export function mergeSession(row: SessionRow): void {
  if (!row) return;
  if (isTombstoned(row.id)) return;
  sessions.update((arr) => mergeInto(arr, row));
}

export function removeSession(id: number): void {
  sessions.update((arr) => removeFrom(arr, id));
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
      next = ev.type === 'killed' ? removeFrom(next, ev.id) : mergeInto(next, ev.row);
    }
    return next;
  });
}

/** Apply a row returned by a mutation command (rename/restart/new). Unlike an
 *  event, a command result is the authoritative response to a request the
 *  user just made, so it clears any tombstone for that id before merging. */
function acceptCommandRow(row: SessionRow | null | undefined): void {
  if (!row) return;
  tombstones.delete(row.id);
  mergeSession(row);
}

export async function relatedSessions(sessionId: number): Promise<Result<SessionRow[]>> {
  return invokeCmd<SessionRow[]>('related_sessions', { args: { session_id: sessionId } });
}

export async function sendPrompt(
  hostAlias: string,
  tmuxName: string,
  prompt: string,
): Promise<Result<void>> {
  return invokeCmd<void>('send_prompt', {
    args: { host_alias: hostAlias, tmux_name: tmuxName, prompt },
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
): Promise<Result<NewBgSessionResult>> {
  const r = await invokeCmd<NewBgSessionResult>('new_bg_session', {
    args: { host_alias: hostAlias, name, prompt },
  });
  if (r.ok && r.value?.session) acceptCommandRow(r.value.session);
  return r;
}

/** Fetch recent log output from a background Claude session (no PTY). */
export async function peekSession(
  hostAlias: string,
  claudeSessionId: string,
): Promise<Result<string>> {
  return invokeCmd<string>('peek_session', {
    args: { host_alias: hostAlias, claude_session_id: claudeSessionId },
  });
}

/** Delete all Claude Code state for a project and remove it from the DB. */
export async function purgeProject(
  hostAlias: string,
  projectPath: string,
  projectId: number,
): Promise<Result<void>> {
  return invokeCmd<void>('purge_project', {
    args: { host_alias: hostAlias, project_path: projectPath, project_id: projectId },
  });
}
