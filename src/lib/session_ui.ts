/**
 * Per-session UI state persisted to localStorage.
 *
 * Things that vary per tmux session:
 *   - centerPx: width (px) of the middle "Details" pane.
 *   - centerCollapsed: whether the user has hidden the middle pane to
 *     reclaim space for the terminal.
 *
 * Identity is the tuple `host_alias + tmux_name`. tmux_name alone is not
 * unique across hosts; the DB row's numeric `id` is, but it churns whenever
 * sessions are re-discovered and rewritten, which would lose user prefs.
 *
 * Storage shape (a single JSON object under one key so we can iterate it
 * for rename-migration without scanning every localStorage key):
 *
 *     cf:session-ui = {
 *       "local:dev-foo":     { centerPx: 360, centerCollapsed: false },
 *       "local:dev-bar":     { centerPx: 240, centerCollapsed: true  },
 *       ...
 *     }
 */

const STORAGE_KEY = 'cf:session-ui';

export interface SessionUiState {
  centerPx: number;
  centerCollapsed: boolean;
}

export const DEFAULT_UI: SessionUiState = {
  centerPx: 360,
  centerCollapsed: false,
};

/** Key used both as map entry name and rename migration target. */
export function sessionKey(hostAlias: string, tmuxName: string): string {
  return `${hostAlias}:${tmuxName}`;
}

function readAll(): Record<string, SessionUiState> {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, SessionUiState>;
    }
  } catch {
    /* corrupt — start over */
  }
  return {};
}

function writeAll(map: Record<string, SessionUiState>): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    /* quota / safari private mode — silently degrade */
  }
}

export function loadSessionUi(hostAlias: string, tmuxName: string): SessionUiState {
  const all = readAll();
  const found = all[sessionKey(hostAlias, tmuxName)];
  if (!found) return { ...DEFAULT_UI };
  return {
    centerPx: typeof found.centerPx === 'number' ? found.centerPx : DEFAULT_UI.centerPx,
    centerCollapsed: typeof found.centerCollapsed === 'boolean' ? found.centerCollapsed : DEFAULT_UI.centerCollapsed,
  };
}

export function saveSessionUi(
  hostAlias: string,
  tmuxName: string,
  state: Partial<SessionUiState>,
): void {
  const all = readAll();
  const key = sessionKey(hostAlias, tmuxName);
  const prev = all[key] ?? DEFAULT_UI;
  all[key] = { ...prev, ...state };
  writeAll(all);
}

/** Move a session's UI state from one tmux_name to another. Called by
 *  Sidebar's rename flow so a renamed session keeps its layout. */
export function migrateSessionUi(
  hostAlias: string,
  oldTmuxName: string,
  newTmuxName: string,
): void {
  const all = readAll();
  const oldKey = sessionKey(hostAlias, oldTmuxName);
  const newKey = sessionKey(hostAlias, newTmuxName);
  if (oldKey === newKey) return;
  if (!all[oldKey]) return;
  all[newKey] = all[oldKey];
  delete all[oldKey];
  writeAll(all);
}

/** Drop persisted state when a session is killed. Keeps localStorage
 *  from growing unbounded over months of fleet use. */
export function forgetSessionUi(hostAlias: string, tmuxName: string): void {
  const all = readAll();
  const key = sessionKey(hostAlias, tmuxName);
  if (!(key in all)) return;
  delete all[key];
  writeAll(all);
}
