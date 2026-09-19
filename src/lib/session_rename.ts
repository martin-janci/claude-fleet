import { get } from 'svelte/store';
import { renameSession, setFriendlyName, type SessionRow } from './sessions';
import { migrateSessionUi } from './session_ui';
import { pushError } from './toasts';
import type { IpcError } from './result';
import { hubStatus, hubActionBlocked } from './hub';
import { hubConnection } from './hub_connection';

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
export async function applySessionRename(
  target: Pick<SessionRow, 'host_alias' | 'tmux_name'> & { friendly_name?: string | null },
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
    const blocked = hubActionBlocked('set_friendly_name', get(hubStatus), get(hubConnection));
    if (blocked) {
      const error: IpcError = { code: 'E_LOCAL_ONLY', message: blocked };
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
  const blocked = hubActionBlocked('rename_session', get(hubStatus), get(hubConnection));
  if (blocked) {
    const error: IpcError = { code: 'E_LOCAL_ONLY', message: blocked };
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
