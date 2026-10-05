import { get } from 'svelte/store';
import { renameSession, setFriendlyName, type SessionRow } from './sessions';
import { migrateSessionUi } from './session_ui';
import { pushError } from './toasts';
import type { IpcError } from './result';
import { hubStatus, hubActionBlocked } from './hub';
import { hubConnection } from './hub_connection';
import { sessionIdActionBlocked, type SessionAction } from './share';

export type RenameMode = 'label' | 'tmux';

export type RenameOutcome =
  | { kind: 'noop' }
  | { kind: 'ok'; row: SessionRow | null }
  | { kind: 'error'; error: IpcError };

/**
 * The backend half of an inline rename, shared by the sidebar row and the
 * details header. `label` sets (or, when empty, clears) the display name;
 * `tmux` renames the tmux session, which gives the row a new identity on
 * the backend — persisted UI state keyed by the old name is migrated along.
 * The value is compared against `target` so an unchanged submit is a no-op,
 * and a failure is already toasted when this returns.
 */
/**
 * Both halves of the refusal for one rename (multi-user M1, F2a; fixed F2e):
 * the hub's own, then this client's access to the row.
 *
 * `set_friendly_name` is `drive` in `share.ts::SESSION_TIER` and
 * `rename_session` is `own`. Both buttons that open the editor are disabled on
 * exactly these two predicates — but the editor ALSO opens on a double-click,
 * which consults no button, so this is the one place both routes funnel
 * through and the one place the gate holds for both. `share_sweep.test.ts`'s
 * `funnelGated` trusts this file for both actions, so nothing else in the sweep
 * is asked to hold that line.
 *
 * It asks by SESSION ID, through `share.ts::sessionIdActionBlocked`. F2a resolved
 * the row here by hand on `(host_alias, tmux_name)` and that was wrong twice:
 *
 *  1. a row the store does not hold answered `null` — allowed. On a fleet this
 *     client does not own, the hub fences rows this person may not see off the
 *     stream, so "not in `$sessions`" reads as *someone else's, or gone*: the
 *     fail-open F2d deleted from four other surfaces. `sessionIdActionBlocked`
 *     refuses with `UNKNOWN_SESSION_REASON` there and still answers `null` on a
 *     standalone desktop, where the master owns every row.
 *  2. a tmux name is not a session identity. This repo's own reconcile logic
 *     treats a lost row's name as reusable (the argument spelled out on
 *     `work.ts::linkSessionId`), so `(host, tmux)` can name a row that merely
 *     INHERITED the pane name — typically the one this person just started, so
 *     the wrong row was answered for in the `own` direction. `find` also took
 *     whichever of a lost row and its live namesake came first in the list.
 *
 * Both go away by using the id: the pinned identity Sidebar double-clicks with
 * is `{ id, host_alias, tmux_name, mode, original }` and SessionDetails passes
 * the row itself, so the id is already in hand at both call sites — the name is
 * what the IPC renames, not what says whose session this is.
 */
function accessBlocked(target: Pick<SessionRow, 'id'>, action: SessionAction): string | null {
  return sessionIdActionBlocked(target.id, action);
}

export async function applySessionRename(
  target: Pick<SessionRow, 'id' | 'host_alias' | 'tmux_name'> & { friendly_name?: string | null },
  mode: RenameMode,
  value: string,
): Promise<RenameOutcome> {
  const next = value.trim();
  if (mode === 'label') {
    // Empty is meaningful here: it clears the label.
    if (next === (target.friendly_name ?? '').trim()) return { kind: 'noop' };
    // The trigger buttons (label-from-details, the row's 🏷 icon) are
    // disabled up front, but the label editor also opens on a double-click
    // (SessionRowItem, SessionDetails' title) — a path that doesn't consult
    // a button's `disabled`. Gate the one place the IPC call actually
    // happens instead of every way to reach it.
    const hubWhy = hubActionBlocked('set_friendly_name', get(hubStatus), get(hubConnection));
    const blocked = hubWhy ?? accessBlocked(target, 'set_friendly_name');
    if (blocked) {
      // The code says WHICH half refused, because the two are different
      // problems: `E_LOCAL_ONLY` is "the hub never accepts this from a client",
      // `E_FORBIDDEN` is "this session is not yours" — which is also the code
      // the hub itself would have answered with.
      const error: IpcError = { code: hubWhy ? 'E_LOCAL_ONLY' : 'E_FORBIDDEN', message: blocked };
      pushError(error, 'Label update failed');
      return { kind: 'error', error };
    }
    const r = await setFriendlyName(target.host_alias, target.tmux_name, next);
    if (!r.ok) {
      pushError(r.error, 'Label update failed');
      return { kind: 'error', error: r.error };
    }
    return { kind: 'ok', row: null };
  }
  if (!next || next === target.tmux_name) return { kind: 'noop' };
  const hubWhy = hubActionBlocked('rename_session', get(hubStatus), get(hubConnection));
  const blocked = hubWhy ?? accessBlocked(target, 'rename_session');
  if (blocked) {
    const error: IpcError = { code: hubWhy ? 'E_LOCAL_ONLY' : 'E_FORBIDDEN', message: blocked };
    pushError(error, 'Rename failed');
    return { kind: 'error', error };
  }
  const r = await renameSession(target.host_alias, target.tmux_name, next);
  if (!r.ok) {
    pushError(r.error, 'Rename failed');
    return { kind: 'error', error: r.error };
  }
  migrateSessionUi(r.value.host_alias, target.tmux_name, r.value.tmux_name);
  return { kind: 'ok', row: r.value };
}

/** Enter commits, Escape cancels; everything else is left to the input. */
export function renameKeyHandler(commit: () => void, cancel: () => void) {
  return (e: KeyboardEvent): void => {
    if (e.key === 'Enter') {
      e.preventDefault();
      commit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    }
  };
}
