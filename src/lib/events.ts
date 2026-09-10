import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionRow, SessionEvent } from './sessions';
import type { HostRow, HostEvent } from './hosts';
import type { AccountRow } from './accounts';
import type { ProjectRow, WorktreeRow, ProjectEvent } from './projects';

export type RowEventHandlers = {
  // ── per-event handlers (delivered one call per event, in arrival order) ──
  onSessionCreated?: (row: SessionRow) => void;
  onSessionUpdated?: (row: SessionRow) => void;
  onSessionKilled?: (payload: { id: number }) => void;
  onHostAdded?: (row: HostRow) => void;
  onHostProbed?: (row: HostRow) => void;
  onHostRemoved?: (payload: { alias: string }) => void;
  onAccountUpserted?: (row: AccountRow) => void;
  onProjectUpdated?: (row: ProjectRow) => void;
  onWorktreeUpdated?: (row: WorktreeRow) => void;
  onWorktreeRemoved?: (payload: { id: number }) => void;
  // ── batched handlers (one call per flush per store, events in order) ──
  // Prefer these for store wiring: the backend's reconcile tick emits one
  // `session:updated` per session, and delivering each one straight into
  // `sessions.update` meant N store flushes (and N sidebar re-derives) per
  // tick. Everything that arrives in the same task is coalesced into a single
  // call per kind, so the store notifies its subscribers once.
  onSessionEvents?: (events: SessionEvent[]) => void;
  onHostEvents?: (events: HostEvent[]) => void;
  onAccountEvents?: (rows: AccountRow[]) => void;
  onProjectEvents?: (events: ProjectEvent[]) => void;
};

type Queued =
  | { name: 'session:created' | 'session:updated'; payload: SessionRow }
  | { name: 'session:killed'; payload: { id: number } }
  | { name: 'host:added' | 'host:probed'; payload: HostRow }
  | { name: 'host:removed'; payload: { alias: string } }
  | { name: 'account:upserted'; payload: AccountRow }
  | { name: 'project:updated'; payload: ProjectRow }
  | { name: 'worktree:updated'; payload: WorktreeRow }
  | { name: 'worktree:removed'; payload: { id: number } };

/**
 * Subscribe to every row-change event from the backend. Returns a single
 * unsubscribe function that tears them all down.
 *
 * Each handler is optional — if you only care about session events, just pass
 * the session handlers. The listeners not declared are simply never created.
 *
 * Delivery is batched per microtask: events that arrive synchronously (a
 * reconcile burst) are queued and flushed together. Per-event handlers still
 * see every event, in order; the batched `on*Events` handlers get one call
 * per flush with that kind's events in order. A `killed` after an `updated`
 * of the same id inside one batch therefore still removes the row.
 */
export async function subscribeToRowEvents(handlers: RowEventHandlers): Promise<UnlistenFn> {
  let queue: Queued[] = [];
  let scheduled = false;
  let disposed = false;

  const flush = () => {
    scheduled = false;
    if (disposed) {
      queue = [];
      return;
    }
    const batch = queue;
    queue = [];
    const sessionEvents: SessionEvent[] = [];
    const hostEvents: HostEvent[] = [];
    const accountRows: AccountRow[] = [];
    const projectEvents: ProjectEvent[] = [];
    for (const ev of batch) {
      switch (ev.name) {
        case 'session:created':
          handlers.onSessionCreated?.(ev.payload);
          sessionEvents.push({ type: 'created', row: ev.payload });
          break;
        case 'session:updated':
          handlers.onSessionUpdated?.(ev.payload);
          sessionEvents.push({ type: 'updated', row: ev.payload });
          break;
        case 'session:killed':
          handlers.onSessionKilled?.(ev.payload);
          sessionEvents.push({ type: 'killed', id: ev.payload.id });
          break;
        case 'host:added':
          handlers.onHostAdded?.(ev.payload);
          hostEvents.push({ type: 'added', row: ev.payload });
          break;
        case 'host:probed':
          handlers.onHostProbed?.(ev.payload);
          hostEvents.push({ type: 'probed', row: ev.payload });
          break;
        case 'host:removed':
          handlers.onHostRemoved?.(ev.payload);
          hostEvents.push({ type: 'removed', alias: ev.payload.alias });
          break;
        case 'account:upserted':
          handlers.onAccountUpserted?.(ev.payload);
          accountRows.push(ev.payload);
          break;
        case 'project:updated':
          handlers.onProjectUpdated?.(ev.payload);
          projectEvents.push({ type: 'project_updated', row: ev.payload });
          break;
        case 'worktree:updated':
          handlers.onWorktreeUpdated?.(ev.payload);
          projectEvents.push({ type: 'worktree_updated', row: ev.payload });
          break;
        case 'worktree:removed':
          handlers.onWorktreeRemoved?.(ev.payload);
          projectEvents.push({ type: 'worktree_removed', id: ev.payload.id });
          break;
      }
    }
    if (sessionEvents.length > 0) handlers.onSessionEvents?.(sessionEvents);
    if (hostEvents.length > 0) handlers.onHostEvents?.(hostEvents);
    if (accountRows.length > 0) handlers.onAccountEvents?.(accountRows);
    if (projectEvents.length > 0) handlers.onProjectEvents?.(projectEvents);
  };

  const enqueue = (ev: Queued) => {
    queue.push(ev);
    if (!scheduled) {
      scheduled = true;
      queueMicrotask(flush);
    }
  };

  const wanted = {
    session: !!(
      handlers.onSessionCreated ||
      handlers.onSessionUpdated ||
      handlers.onSessionKilled ||
      handlers.onSessionEvents
    ),
    host: !!(
      handlers.onHostAdded ||
      handlers.onHostProbed ||
      handlers.onHostRemoved ||
      handlers.onHostEvents
    ),
    account: !!(handlers.onAccountUpserted || handlers.onAccountEvents),
    project: !!(
      handlers.onProjectUpdated ||
      handlers.onWorktreeUpdated ||
      handlers.onWorktreeRemoved ||
      handlers.onProjectEvents
    ),
  };

  const sub = <N extends Queued['name']>(
    name: N,
    want: boolean,
  ): Promise<UnlistenFn | null> => {
    if (!want) return Promise.resolve(null);
    return listen<Extract<Queued, { name: N }>['payload']>(name, (e) =>
      enqueue({ name, payload: e.payload } as Queued),
    );
  };
  // Register all listeners concurrently — each `listen` is its own IPC
  // round-trip; awaiting them serially needlessly delayed event flow on mount.
  const unlisteners = await Promise.all([
    sub('session:created', wanted.session),
    sub('session:updated', wanted.session),
    sub('session:killed', wanted.session),
    sub('host:added', wanted.host),
    sub('host:probed', wanted.host),
    sub('host:removed', wanted.host),
    sub('account:upserted', wanted.account),
    sub('project:updated', wanted.project),
    sub('worktree:updated', wanted.project),
    sub('worktree:removed', wanted.project),
  ]);
  return () => {
    disposed = true;
    queue = [];
    for (const u of unlisteners) u?.();
  };
}
