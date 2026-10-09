import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { SessionRow, SessionEvent } from './sessions';
import type { SessionEvent as TimelineEvent } from './timeline';
import type { HostRow, HostEvent, HostHealth } from './hosts';
import type { AccountRow } from './accounts';
import type { ProjectRow, WorktreeRow, ProjectEvent } from './projects';
import type { TaskRow, TaskEvent } from './tasks';
import type { AccountUsageSnapshot } from './account_usage_store';
import type { AssetInventoryRow, CatalogSummary, SyncProgress } from './assets';
import type { MoveProgress } from './moveProgress';
import type { StartProgressFrame } from './start_steps';
import type { TrackerRow, WorkEvent, WorkItemRow } from './trackers';
import { parseWorkChanged, type WorkChanged } from './work_view';
import { parseGrantChanged, type GrantChanged } from './access';

/**
 * How long a flush waits for more events after the first one arrives. Tauri
 * delivers every emitted event as its own `webview.eval()` — a separate script
 * task — so a microtask flush would see exactly one event per batch. A short
 * timer spans the whole reconcile burst (tens of events a few hundred µs apart)
 * while adding no perceptible latency to a lone event.
 */
export const ROW_EVENT_FLUSH_MS = 16;

export type RowEventHandlers = {
  // ── per-event handlers (delivered one call per event, in arrival order) ──
  /** @deprecated Wire stores through the batched `on*Events` handlers below;
   *  per-event delivery costs one store flush per event. Kept for tests and
   *  for ad-hoc listeners that don't touch a store. */
  onSessionCreated?: (row: SessionRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onSessionUpdated?: (row: SessionRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onSessionKilled?: (payload: { id: number }) => void;
  /** @deprecated See `onSessionCreated`. */
  onHostAdded?: (row: HostRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onHostProbed?: (row: HostRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onHostRemoved?: (payload: { alias: string }) => void;
  /** @deprecated See `onSessionCreated`. */
  onAccountUpserted?: (row: AccountRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onProjectUpdated?: (row: ProjectRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onWorktreeUpdated?: (row: WorktreeRow) => void;
  /** @deprecated See `onSessionCreated`. */
  onWorktreeRemoved?: (payload: { id: number }) => void;
  // ── asset catalog (per-event: a scan writes tens of rows, not the
  //    hundreds a reconcile tick produces, and the assets store patches in
  //    place, so these need no batched twin) ──
  onAssetInventoryUpdated?: (row: AssetInventoryRow) => void;
  onAssetInventoryCleared?: (payload: { host_alias: string; harness: string }) => void;
  onCatalogLoaded?: (summary: CatalogSummary) => void;
  onSyncProgress?: (p: SyncProgress) => void;
  onMoveProgress?: (p: MoveProgress) => void;
  /** `start:progress` (step 5.13): one step of a `new_session` in flight. */
  onStartProgress?: (p: StartProgressFrame) => void;
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
  onTaskEvents?: (events: TaskEvent[]) => void;
  /** One call per flush with every `account_usage:updated` row. */
  onAccountUsageEvents?: (rows: AccountUsageSnapshot[]) => void;
  /** One call per flush with every `session:event` (timeline push). */
  onTimelineEvents?: (events: TimelineEvent[]) => void;
  /** One call per flush with the ids from every `session:conversations`. */
  onConversationsChanged?: (sessionIds: number[]) => void;
  /** One call per flush with every `work:*` frame (work graph M3), in order. */
  onWorkEvents?: (events: WorkEvent[]) => void;
  /** One call per flush with every well-formed `work:changed` (work graph
   *  M14.1d: ids only), in order. Each `what` reaches the Work view's
   *  readers as a change kind (`noteWorkChanged`): a `resync` reloads the
   *  whole view, anything else re-reads what shows. */
  onWorkChanged?: (changes: WorkChanged[]) => void;
  /** One call per flush with the key of every `settings:changed`
   *  (declarative pages P3: the key only), in order, duplicates kept. */
  onSettingsChanged?: (keys: string[]) => void;
  /** One call per flush with every `update:changed` (update design §11: ids
   *  only), in order: re-read `update_status`. */
  onUpdateChanged?: (changes: UpdateChanged[]) => void;
  /** One call per flush with every `update:decision` (update design §6.4):
   *  what the hub would tell a target moved; the target checks again. */
  onUpdateDecision?: (decisions: UpdateDecision[]) => void;
  /** One call per flush with the id of every `download:changed` (file
   *  downloads: ids only), in order: re-read `list_downloads`. */
  onDownloadsChanged?: (ids: number[]) => void;
  /** One call per flush with the id of every `local_workspace:changed`
   *  (local workspace sync: ids only), in order: re-read
   *  `list_local_workspaces`. */
  onLocalWorkspacesChanged?: (ids: number[]) => void;
  /**
   * One call per flush with every well-formed `grant:changed` (multi-user M1:
   * ids only), in order. A grant mutates no `sessions` column, so sharing and
   * revoking emit NOTHING a row event could carry — this frame is how a
   * client keeps its own grant set current (`access.ts::applyGrantChanges`)
   * without re-fetching, and it is what makes a revoke close an attached
   * terminal rather than waiting for a re-list.
   */
  onGrantChanged?: (changes: GrantChanged[]) => void;
};

/**
 * `confirm:changed` (redesign step 9.2): the queue of control-API calls
 * waiting for a person moved. An empty frame; `confirms.ts` listens to it on
 * its own (not through `subscribeRowEvents`' batching) and re-reads the
 * queue, which on a hub-backed desktop is the hub's.
 */
export const CONFIRM_CHANGED_EVENT = 'confirm:changed';

/**
 * `handoff:changed` (redesign step 9.3): Control's agent handed work on and
 * a receipt was written. An empty frame; `handoffs.ts` listens to it on its
 * own and re-reads `control_handoffs`, which on a hub-backed desktop is the
 * hub's.
 */
export const HANDOFF_CHANGED_EVENT = 'handoff:changed';

/** The payload of `update:changed`: what moved, never the row itself. */
/** `what` is observed | pin | channel today; a newer hub may add others. */
export type UpdateChanged = { what: string; target?: string };
/** The payload of `update:decision`: a target, its new status and version. */
export type UpdateDecision = { target: string; status: string; version?: string };

type Queued =
  | { name: 'session:created' | 'session:updated'; payload: SessionRow }
  | { name: 'session:killed'; payload: { id: number } }
  | { name: 'session:event'; payload: TimelineEvent }
  | { name: 'session:conversations'; payload: { session_id: number } }
  | { name: 'host:added' | 'host:probed'; payload: HostRow }
  | {
      name: 'host:pinged';
      payload: {
        alias: string;
        last_pinged_at: number;
        reachable: boolean;
        claude_version_at?: number | null;
        health?: HostHealth | null;
      };
    }
  | { name: 'host:removed'; payload: { alias: string } }
  | { name: 'account:upserted'; payload: AccountRow }
  | { name: 'project:updated'; payload: ProjectRow }
  | { name: 'worktree:updated'; payload: WorktreeRow }
  | { name: 'worktree:removed'; payload: { id: number } }
  | { name: 'task:updated'; payload: TaskRow }
  | { name: 'account_usage:updated'; payload: AccountUsageSnapshot }
  | { name: 'asset_inventory:updated'; payload: AssetInventoryRow }
  | { name: 'asset_inventory:cleared'; payload: { host_alias: string; harness: string } }
  | { name: 'catalog:loaded'; payload: CatalogSummary }
  | { name: 'sync:progress'; payload: SyncProgress }
  | { name: 'move:progress'; payload: MoveProgress }
  | { name: 'start:progress'; payload: StartProgressFrame }
  | { name: 'work:item'; payload: WorkItemRow }
  | { name: 'work:tracker'; payload: TrackerRow }
  | { name: 'work:tracker_removed'; payload: { id: number } }
  | { name: 'work:changed'; payload: unknown }
  | { name: 'settings:changed'; payload: { key: string } }
  | { name: 'update:changed'; payload: UpdateChanged }
  | { name: 'update:decision'; payload: UpdateDecision }
  | { name: 'download:changed'; payload: { id: number } }
  | { name: 'local_workspace:changed'; payload: { id: number } }
  | { name: 'grant:changed'; payload: unknown };

/**
 * Subscribe to every row-change event from the backend. Returns a single
 * unsubscribe function that tears them all down.
 *
 * Each handler is optional — if you only care about session events, just pass
 * the session handlers. The listeners not declared are simply never created.
 *
 * Delivery is batched on a short timer (`ROW_EVENT_FLUSH_MS`): the first
 * event arms it, everything that lands before it fires is flushed together.
 * Per-event handlers still see every event, in order; the batched `on*Events`
 * handlers get one call per flush with that kind's events in order. A
 * `killed` after an `updated` of the same id inside one batch therefore
 * still removes the row.
 */
export async function subscribeToRowEvents(handlers: RowEventHandlers): Promise<UnlistenFn> {
  let queue: Queued[] = [];
  let timer: ReturnType<typeof setTimeout> | null = null;
  let disposed = false;

  // One throwing subscriber must not drop the rest of the batch: every
  // handler call is its own unit, so a timeline bug cannot cost the work and
  // grant frames that share its flush.
  const deliver = (call: () => void) => {
    try {
      call();
    } catch (e) {
      console.error('[events] row-event handler failed', e);
    }
  };

  const flush = () => {
    timer = null;
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
    const taskEvents: TaskEvent[] = [];
    const accountUsageEvents: AccountUsageSnapshot[] = [];
    const timelineEvents: TimelineEvent[] = [];
    const conversationsChangedIds: number[] = [];
    const workEvents: WorkEvent[] = [];
    const workChanges: WorkChanged[] = [];
    const settingsKeys: string[] = [];
    const updateChanges: UpdateChanged[] = [];
    const updateDecisions: UpdateDecision[] = [];
    const downloadIds: number[] = [];
    const localWorkspaceIds: number[] = [];
    const grantChanges: GrantChanged[] = [];
    for (const ev of batch) {
      switch (ev.name) {
        case 'session:created':
          deliver(() => handlers.onSessionCreated?.(ev.payload));
          sessionEvents.push({ type: 'created', row: ev.payload });
          break;
        case 'session:updated':
          deliver(() => handlers.onSessionUpdated?.(ev.payload));
          sessionEvents.push({ type: 'updated', row: ev.payload });
          break;
        case 'session:killed':
          deliver(() => handlers.onSessionKilled?.(ev.payload));
          sessionEvents.push({ type: 'killed', id: ev.payload.id });
          break;
        case 'session:event':
          timelineEvents.push(ev.payload);
          break;
        case 'session:conversations':
          conversationsChangedIds.push(ev.payload.session_id);
          break;
        case 'host:added':
          deliver(() => handlers.onHostAdded?.(ev.payload));
          hostEvents.push({ type: 'added', row: ev.payload });
          break;
        case 'host:probed':
          deliver(() => handlers.onHostProbed?.(ev.payload));
          hostEvents.push({ type: 'probed', row: ev.payload });
          break;
        // A probe that found the host exactly as it was. Carries the two
        // fields that can move without being a change — the stamp and, for
        // the moment it flips, reachability — instead of the whole row.
        case 'host:pinged':
          hostEvents.push({ type: 'pinged', ...ev.payload });
          break;
        case 'host:removed':
          deliver(() => handlers.onHostRemoved?.(ev.payload));
          hostEvents.push({ type: 'removed', alias: ev.payload.alias });
          break;
        case 'account:upserted':
          deliver(() => handlers.onAccountUpserted?.(ev.payload));
          accountRows.push(ev.payload);
          break;
        case 'project:updated':
          deliver(() => handlers.onProjectUpdated?.(ev.payload));
          projectEvents.push({ type: 'project_updated', row: ev.payload });
          break;
        case 'worktree:updated':
          deliver(() => handlers.onWorktreeUpdated?.(ev.payload));
          projectEvents.push({ type: 'worktree_updated', row: ev.payload });
          break;
        case 'worktree:removed':
          deliver(() => handlers.onWorktreeRemoved?.(ev.payload));
          projectEvents.push({ type: 'worktree_removed', id: ev.payload.id });
          break;
        case 'task:updated':
          taskEvents.push({ type: 'updated', row: ev.payload });
          break;
        case 'account_usage:updated':
          accountUsageEvents.push(ev.payload);
          break;
        case 'asset_inventory:updated':
          deliver(() => handlers.onAssetInventoryUpdated?.(ev.payload));
          break;
        case 'asset_inventory:cleared':
          deliver(() => handlers.onAssetInventoryCleared?.(ev.payload));
          break;
        case 'catalog:loaded':
          deliver(() => handlers.onCatalogLoaded?.(ev.payload));
          break;
        case 'sync:progress':
          deliver(() => handlers.onSyncProgress?.(ev.payload));
          break;
        case 'move:progress':
          deliver(() => handlers.onMoveProgress?.(ev.payload));
          break;
        case 'start:progress':
          deliver(() => handlers.onStartProgress?.(ev.payload));
          break;
        case 'work:item':
          workEvents.push({ type: 'item', row: ev.payload });
          break;
        case 'work:tracker':
          workEvents.push({ type: 'tracker', row: ev.payload });
          break;
        case 'work:tracker_removed':
          workEvents.push({ type: 'tracker_removed', id: ev.payload.id });
          break;
        case 'work:changed': {
          const c = parseWorkChanged(ev.payload);
          if (c) workChanges.push(c);
          break;
        }
        case 'settings:changed':
          if (typeof ev.payload?.key === 'string') settingsKeys.push(ev.payload.key);
          break;
        case 'update:changed': {
          // Ids only, so anything readable is enough: a newer hub's `what`
          // still means "re-read".
          const p = ev.payload as Partial<UpdateChanged> | null;
          if (p && typeof p.what === 'string') {
            updateChanges.push(
              typeof p.target === 'string' ? { what: p.what, target: p.target } : { what: p.what },
            );
          }
          break;
        }
        case 'update:decision': {
          const p = ev.payload as Partial<UpdateDecision> | null;
          if (p && typeof p.target === 'string' && typeof p.status === 'string') {
            updateDecisions.push(
              typeof p.version === 'string'
                ? { target: p.target, status: p.status, version: p.version }
                : { target: p.target, status: p.status },
            );
          }
          break;
        }
        case 'download:changed':
          if (typeof ev.payload?.id === 'number') downloadIds.push(ev.payload.id);
          break;
        case 'local_workspace:changed':
          if (typeof ev.payload?.id === 'number') localWorkspaceIds.push(ev.payload.id);
          break;
        case 'grant:changed': {
          const g = parseGrantChanged(ev.payload);
          if (g) grantChanges.push(g);
          break;
        }
      }
    }
    if (sessionEvents.length > 0) deliver(() => handlers.onSessionEvents?.(sessionEvents));
    if (hostEvents.length > 0) deliver(() => handlers.onHostEvents?.(hostEvents));
    if (accountRows.length > 0) deliver(() => handlers.onAccountEvents?.(accountRows));
    if (projectEvents.length > 0) deliver(() => handlers.onProjectEvents?.(projectEvents));
    if (taskEvents.length > 0) deliver(() => handlers.onTaskEvents?.(taskEvents));
    if (accountUsageEvents.length > 0) deliver(() => handlers.onAccountUsageEvents?.(accountUsageEvents));
    if (timelineEvents.length > 0) deliver(() => handlers.onTimelineEvents?.(timelineEvents));
    if (conversationsChangedIds.length > 0) deliver(() => handlers.onConversationsChanged?.(conversationsChangedIds));
    if (workEvents.length > 0) deliver(() => handlers.onWorkEvents?.(workEvents));
    if (workChanges.length > 0) deliver(() => handlers.onWorkChanged?.(workChanges));
    if (settingsKeys.length > 0) deliver(() => handlers.onSettingsChanged?.(settingsKeys));
    if (updateChanges.length > 0) deliver(() => handlers.onUpdateChanged?.(updateChanges));
    if (updateDecisions.length > 0) deliver(() => handlers.onUpdateDecision?.(updateDecisions));
    if (downloadIds.length > 0) deliver(() => handlers.onDownloadsChanged?.(downloadIds));
    if (localWorkspaceIds.length > 0) deliver(() => handlers.onLocalWorkspacesChanged?.(localWorkspaceIds));
    if (grantChanges.length > 0) deliver(() => handlers.onGrantChanged?.(grantChanges));
  };

  const enqueue = (ev: Queued) => {
    queue.push(ev);
    if (timer === null) timer = setTimeout(flush, ROW_EVENT_FLUSH_MS);
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
    task: !!handlers.onTaskEvents,
    accountUsage: !!handlers.onAccountUsageEvents,
    timelineEvents: !!handlers.onTimelineEvents,
    conversationsChanged: !!handlers.onConversationsChanged,
    assetInventoryUpdated: !!handlers.onAssetInventoryUpdated,
    assetInventoryCleared: !!handlers.onAssetInventoryCleared,
    catalogLoaded: !!handlers.onCatalogLoaded,
    syncProgress: !!handlers.onSyncProgress,
    moveProgress: !!handlers.onMoveProgress,
    startProgress: !!handlers.onStartProgress,
    work: !!handlers.onWorkEvents,
    workChanged: !!handlers.onWorkChanged,
    settingsChanged: !!handlers.onSettingsChanged,
    updateChanged: !!handlers.onUpdateChanged,
    updateDecision: !!handlers.onUpdateDecision,
    downloadsChanged: !!handlers.onDownloadsChanged,
    localWorkspacesChanged: !!handlers.onLocalWorkspacesChanged,
    grantChanged: !!handlers.onGrantChanged,
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
  // allSettled, so a `listen` that rejects does not strand the ones that
  // already registered: they are removed before the error propagates.
  const settled = await Promise.allSettled([
    sub('session:created', wanted.session),
    sub('session:updated', wanted.session),
    sub('session:killed', wanted.session),
    sub('session:event', wanted.timelineEvents),
    sub('session:conversations', wanted.conversationsChanged),
    sub('host:added', wanted.host),
    sub('host:probed', wanted.host),
    sub('host:pinged', wanted.host),
    sub('host:removed', wanted.host),
    sub('account:upserted', wanted.account),
    sub('project:updated', wanted.project),
    sub('worktree:updated', wanted.project),
    sub('worktree:removed', wanted.project),
    sub('task:updated', wanted.task),
    sub('account_usage:updated', wanted.accountUsage),
    sub('asset_inventory:updated', wanted.assetInventoryUpdated),
    sub('asset_inventory:cleared', wanted.assetInventoryCleared),
    sub('catalog:loaded', wanted.catalogLoaded),
    sub('sync:progress', wanted.syncProgress),
    sub('move:progress', wanted.moveProgress),
    sub('start:progress', wanted.startProgress),
    sub('work:item', wanted.work),
    sub('work:tracker', wanted.work),
    sub('work:tracker_removed', wanted.work),
    sub('work:changed', wanted.workChanged),
    sub('settings:changed', wanted.settingsChanged),
    sub('update:changed', wanted.updateChanged),
    sub('update:decision', wanted.updateDecision),
    sub('download:changed', wanted.downloadsChanged),
    sub('local_workspace:changed', wanted.localWorkspacesChanged),
    sub('grant:changed', wanted.grantChanged),
  ]);
  const unlisteners = settled.map((r) => (r.status === 'fulfilled' ? r.value : null));
  const failed = settled.find((r): r is PromiseRejectedResult => r.status === 'rejected');
  if (failed) {
    for (const u of unlisteners) u?.();
    throw failed.reason;
  }
  return () => {
    disposed = true;
    queue = [];
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    for (const u of unlisteners) u?.();
  };
}
