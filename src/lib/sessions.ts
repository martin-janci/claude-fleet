import { writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';

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
}

export const sessions = writable<SessionRow[]>([]);

export async function loadSessions(): Promise<Result<SessionRow[]>> {
  const r = await invokeCmd<SessionRow[]>('list_sessions');
  if (r.ok) sessions.set(r.value);
  return r;
}

export async function killSession(name: string): Promise<Result<void>> {
  const r = await invokeCmd<void>('kill_session', { args: { name } });
  if (r.ok) await loadSessions();
  return r;
}

export async function renameSession(
  oldName: string,
  newName: string,
): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('rename_session', {
    args: { old_name: oldName, new_name: newName },
  });
  if (r.ok) await loadSessions();
  return r;
}

export async function restartSession(name: string): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('restart_session', { args: { name } });
  if (r.ok) await loadSessions();
  return r;
}

export interface NewSessionArgs {
  project_id: number;
  worktree_id: number | null;
  name: string;
}

export async function newSession(args: NewSessionArgs): Promise<Result<SessionRow>> {
  const r = await invokeCmd<SessionRow>('new_session', { args });
  if (r.ok) void loadSessions();
  return r;
}
