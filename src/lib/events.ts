import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';
import type { AccountRow } from './accounts';
import type { ProjectRow, WorktreeRow } from './projects';

export type RowEventHandlers = {
  onSessionCreated?: (row: SessionRow) => void;
  onSessionUpdated?: (row: SessionRow) => void;
  onSessionKilled?: (payload: { id: number }) => void;
  onHostAdded?: (row: HostRow) => void;
  onHostProbed?: (row: HostRow) => void;
  onHostRemoved?: (payload: { alias: string }) => void;
  onAccountUpserted?: (row: AccountRow) => void;
  onProjectUpdated?: (row: ProjectRow) => void;
  onWorktreeUpdated?: (row: WorktreeRow) => void;
};

/**
 * Subscribe to every row-change event from the backend. Returns a single
 * unsubscribe function that tears them all down.
 *
 * Each handler is optional — if you only care about session events, just pass
 * the session handlers. The listeners not declared are simply never created.
 */
export async function subscribeToRowEvents(handlers: RowEventHandlers): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];
  const sub = async <T>(name: string, handler: ((payload: T) => void) | undefined): Promise<void> => {
    if (!handler) return;
    const u = await listen<T>(name, (e) => handler(e.payload));
    unlisteners.push(u);
  };
  await sub<SessionRow>('session:created', handlers.onSessionCreated);
  await sub<SessionRow>('session:updated', handlers.onSessionUpdated);
  await sub<{ id: number }>('session:killed', handlers.onSessionKilled);
  await sub<HostRow>('host:added', handlers.onHostAdded);
  await sub<HostRow>('host:probed', handlers.onHostProbed);
  await sub<{ alias: string }>('host:removed', handlers.onHostRemoved);
  await sub<AccountRow>('account:upserted', handlers.onAccountUpserted);
  await sub<ProjectRow>('project:updated', handlers.onProjectUpdated);
  await sub<WorktreeRow>('worktree:updated', handlers.onWorktreeUpdated);
  return () => {
    for (const u of unlisteners) u();
  };
}
