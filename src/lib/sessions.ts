import { writable } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type Result } from './result';

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
}

export const sessions = writable<SessionRow[]>([]);

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

export async function renameSession(
  hostAlias: string,
  oldName: string,
  newName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', {
    args: { host_alias: hostAlias, old_name: oldName, new_name: newName },
  });
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function restartSession(hostAlias: string, name: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', {
    args: { host_alias: hostAlias, name },
  });
  if (r.ok) mergeSession(r.value);
  return r;
}

export interface NewSessionArgs {
  host_alias: string;
  project_id: number;
  worktree_id: number | null;
  name: string;
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('new_session', { args });
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function newSessionAbortable(
  args: NewSessionArgs,
  signal?: AbortSignal,
): Promise<Result<SessionRow>> {
  const r = await invokeCmdAbortable<SessionRow>('new_session', { args }, signal);
  if (r.ok) mergeSession(r.value);
  return r;
}

export async function bootstrapSessions(): Promise<void> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
  if (r.ok) sessions.set(r.value);
}

export function mergeSession(row: SessionRow): void {
  sessions.update((arr) => {
    const i = arr.findIndex((s) => s.id === row.id);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

export function removeSession(id: number): void {
  sessions.update((arr) => arr.filter((s) => s.id !== id));
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
