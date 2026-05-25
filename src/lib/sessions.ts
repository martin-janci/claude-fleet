import { writable } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';
import { readPref, writePref } from './prefs';

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
  claude_status: string | null;
  effort_level: string | null;
  pr_url: string | null;
  current_activity: string | null;
  // Display label set by the in-session agent via the `set_friendly_name`
  // MCP tool. When the sidebar toggle is on, this is shown instead of
  // tmux_name; null falls back to tmux_name.
  friendly_name: string | null;
  // Safe-kill flow (migration 017): values "requested" | "ready" | "failed" | null.
  safe_kill_state: string | null;
  safe_kill_nonce: string | null;
  safe_kill_detail: string | null;
  safe_kill_requested_at: number | null;
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

export async function loadSessions(): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
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

export async function bootstrapSessions(): Promise<void> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
  if (r.ok) sessions.set(r.value);
}

// Recently-removed session ids. Both the optimistic `removeSession()` and the
// `session:killed` event delete a row; without a tombstone, a `session:updated`
// event still in flight for that id would re-insert the dead row ("ghost
// session"). Entries expire so a genuinely new id is never blocked.
const tombstones = new Map<number, number>();
const TOMBSTONE_MS = 5000;

function isTombstoned(id: number): boolean {
  const t = tombstones.get(id);
  if (t === undefined) return false;
  if (Date.now() - t > TOMBSTONE_MS) {
    tombstones.delete(id);
    return false;
  }
  return true;
}

export function mergeSession(row: SessionRow): void {
  if (!row) return;
  if (isTombstoned(row.id)) return;
  sessions.update((arr) => {
    const i = arr.findIndex((s) => s.id === row.id);
    if (i === -1) return [...arr, row];
    // Monotonic guard: don't let a staler payload (e.g. a command return
    // value that raced a newer `session:updated` event) clobber a fresher
    // row. Equal timestamps still apply — they may carry a status change.
    if (row.last_activity_at < arr[i].last_activity_at) return arr;
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function removeSession(id: number): void {
  tombstones.set(id, Date.now());
  sessions.update((arr) => arr.filter((s) => s.id !== id));
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
}

/** Launch a supervised Claude background session on `hostAlias`. */
export async function newBgSession(
  hostAlias: string,
  name: string,
  prompt: string,
): Promise<Result<NewBgSessionResult>> {
  return invokeCmd<NewBgSessionResult>('new_bg_session', {
    args: { host_alias: hostAlias, name, prompt },
  });
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
