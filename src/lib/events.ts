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
  const sub = <T>(
    name: string,
    handler: ((payload: T) => void) | undefined,
  ): Promise<UnlistenFn | null> => {
    if (!handler) return Promise.resolve(null);
    return listen<T>(name, (e) => handler(e.payload));
  };
  // Register all listeners concurrently — each `listen` is its own IPC
  // round-trip; awaiting them serially needlessly delayed event flow on mount.
  const unlisteners = await Promise.all([
    sub<SessionRow>('session:created', handlers.onSessionCreated),
    sub<SessionRow>('session:updated', handlers.onSessionUpdated),
    sub<{ id: number }>('session:killed', handlers.onSessionKilled),
    sub<HostRow>('host:added', handlers.onHostAdded),
    sub<HostRow>('host:probed', handlers.onHostProbed),
    sub<{ alias: string }>('host:removed', handlers.onHostRemoved),
    sub<AccountRow>('account:upserted', handlers.onAccountUpserted),
    sub<ProjectRow>('project:updated', handlers.onProjectUpdated),
    sub<WorktreeRow>('worktree:updated', handlers.onWorktreeUpdated),
  ]);
  return () => {
    for (const u of unlisteners) u?.();
  };
}
