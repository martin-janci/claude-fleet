// The UX agent's state, as the panel needs it. Follows `app_views.ts`: the
// FAB and the panel talk through these stores instead of prop-drilling
// through App.svelte.
import { derived, get, writable, type Readable, type Writable } from 'svelte/store';
import { invokeCmd } from './result';
import { restartSession, sessions, type SessionRow } from './sessions';

export type OperatorBlocked = 'absent' | 'lost' | 'no_mcp' | 'token_revoked' | 'no_host';

export interface OperatorStatus {
  ready: boolean;
  session: SessionRow | null;
  blocked: OperatorBlocked | null;
  /**
   * The host the operator runs (or would run) on. Absent from a hub older
   * than the field, which only ever homed the operator on `local`.
   */
  host?: string;
}

export const agentPanelOpen: Writable<boolean> = writable(false);
export const operatorState: Writable<'unknown' | 'waking' | 'ready' | OperatorBlocked> =
  writable('unknown');
/**
 * The operator's row as the last `operator_status` / `ensure_operator` call
 * returned it. This is the IDENTITY anchor — which session the panel is
 * about — not the live status; read `operatorRow` for that.
 */
export const operatorSession: Writable<SessionRow | null> = writable(null);
/**
 * Where the operator is homed, as the last status call said. Only the
 * `no_host` copy reads it: which host is missing decides what the fix is.
 */
export const operatorHost: Writable<string> = writable('local');

/**
 * The operator's row as the app currently knows it.
 *
 * Every other panel in the app reads its row out of the `sessions` store,
 * which the row-event bus patches in place (`events.ts` →
 * `subscribeToRowEvents` → `applySessionEvents`). `operatorSession` alone was
 * a snapshot taken when the panel opened, so the composer's "refuse to send
 * while working" gate and the `stuck_kind` line — the spec's two stated
 * mitigations — read a status that could not change: after you sent, the row
 * still said whatever it said at open, and if the gate ever did fire it never
 * cleared.
 *
 * Matched by `(host_alias, tmux_name)` and not by id, for the same reason
 * `OperatorRef` is: ids churn on re-discovery. The anchor is the fallback for
 * the window between a birth and the `session:created` event that follows it,
 * so the panel is never blank for a session that demonstrably exists.
 */
export const operatorRow: Readable<SessionRow | null> = derived(
  [operatorSession, sessions],
  ([$anchor, $rows]) => {
    if (!$anchor) return null;
    return (
      $rows.find(
        (r) => r.host_alias === $anchor.host_alias && r.tmux_name === $anchor.tmux_name,
      ) ?? $anchor
    );
  },
);

/**
 * PURE: what the panel says for a blocked state, and whether there is a
 * button under it.
 *
 * Only `absent` and `lost` get a button. The brief's other two candidates
 * were dropped:
 *  - `no_mcp`'s "Enable the control API" would call `mcp_configure`, a
 *    LocalOnly command — nothing behind this button may be LocalOnly, so a
 *    phone client can reuse it. (It also cannot occur in hub mode, where the
 *    control API is always on.) The fix lives in Settings instead.
 *  - `token_revoked`'s "Mint a new token" would call `ensure_operator`,
 *    which returns early when the session is alive — and for a revoked
 *    token the session IS alive, so the button would do nothing. The honest
 *    recovery is killing that session from the sidebar and pressing the
 *    open-agent button again, which the title says outright.
 */
export function blockedCopy(
  b: OperatorBlocked,
  host = 'local',
): { title: string; action: string | null } {
  switch (b) {
    case 'absent':
      return { title: 'The agent is not running.', action: 'Wake the agent' };
    case 'lost':
      return {
        title:
          "The agent's session was lost. It is not brought back silently, because it may have stopped mid-sentence.",
        action: 'Restart the agent',
      };
    case 'no_mcp':
      return {
        title:
          'The control API is off, so the agent would have no tools. Turn it on in Settings → Control API.',
        action: null,
      };
    case 'token_revoked':
      return {
        title:
          "The agent's token was revoked, so it can no longer reach the fleet. Kill its session from the sidebar and press the button again to mint a new one.",
        action: null,
      };
    case 'no_host':
      // `absent` here would offer "Wake the agent" for a press that cannot
      // work: the agent's home is a host this fleet does not have. Which
      // host decides the fix — `local` means a hub without a local host,
      // and the flag that homes the agent elsewhere is the way out; any
      // other alias was configured and is simply not in the fleet.
      return {
        title:
          host === 'local'
            ? 'The agent runs on the local host, and this fleet has none (a hub started with hub.local_host=false). Start the hub with --operator-host <alias> to run it on a fleet host.'
            : `The agent is set to run on ${host}, which is not in this fleet. Add that host, or point the agent elsewhere with --operator-host <alias>.`,
        action: null,
      };
  }
}

/** Read status without changing anything else. */
export async function refreshOperator(): Promise<void> {
  const r = await invokeCmd<OperatorStatus>('operator_status');
  if (!r.ok) {
    operatorState.set('absent');
    return;
  }
  operatorSession.set(r.value.session);
  operatorHost.set(r.value.host ?? 'local');
  operatorState.set(r.value.ready ? 'ready' : (r.value.blocked ?? 'absent'));
}

/** The in-flight `openAgent`, or null. See [`openAgent`]. */
let opening: Promise<void> | null = null;

/**
 * Open the panel and make sure there is an agent behind it.
 *
 * `absent` is the only reason worth acting on here: every other block is
 * either deliberate (the control API turned off, a token revoked) or a
 * death worth seeing, and creating a session under any of them would
 * produce an agent that cannot work. The backend depends on this ordering:
 * `ensure_operator` fails with `E_PROVISION` when the control API has never
 * been enabled, and `operator_status` returns `no_mcp` first precisely so
 * that path is unreachable.
 */
export function openAgent(): Promise<void> {
  // Re-entrancy guard. Without it a double-click (or ⌘E while the first
  // press is still on the wire) issues two `ensure_operator` calls, and
  // Tauri runs commands concurrently. The backend now serialises them too —
  // `operator_birth_lock` in `service/operator.rs` — but this is the layer
  // that should not have asked twice in the first place: the second caller
  // wants the same answer as the first, and joining the in-flight promise IS
  // that answer.
  //
  // Not a boolean flag: a flag would let the second caller return before the
  // agent exists, and the panel would render `unknown` over a session that
  // is halfway born.
  //
  // Opening the panel happens HERE and not in `openAgentOnce`, because it is
  // what every caller wants whether it starts the birth or joins one already
  // running. Inside the work function it ran once, before the first `await`,
  // so closing the sheet mid-birth and pressing again returned a promise
  // already long past that line: the button did nothing, visibly, until the
  // birth resolved.
  agentPanelOpen.set(true);
  if (opening) return opening;
  const p = openAgentOnce().finally(() => {
    if (opening === p) opening = null;
  });
  opening = p;
  return p;
}

async function openAgentOnce(): Promise<void> {
  await refreshOperator();
  if (get(operatorState) !== 'absent') return;
  operatorState.set('waking');
  const r = await invokeCmd<SessionRow>('ensure_operator');
  if (!r.ok) {
    operatorState.set('absent');
    return;
  }
  operatorSession.set(r.value);
  operatorState.set('ready');
}

/**
 * Close the panel. The agent keeps running — the panel is a window onto a
 * session, not the session itself.
 */
export function closeAgent(): void {
  agentPanelOpen.set(false);
}

/**
 * What the FAB and ⌘E do: open the panel, or close it if it is already open.
 *
 * The design says the chord TOGGLES, and for a fixed sheet pinned over the
 * bottom-right corner of every view that is not a nicety — before this,
 * `agentPanelOpen` was written `true` in one place and `false` nowhere in
 * production code, so the first press covered the corner of the terminal,
 * Hosts and Files for the life of the process with no way back.
 *
 * Closing while the agent is still waking is deliberate and safe: the
 * in-flight `openAgent` runs to completion (nobody cancels a birth halfway
 * through), and reopening finds the session it created.
 */
export function toggleAgent(): Promise<void> {
  if (get(agentPanelOpen)) {
    closeAgent();
    return Promise.resolve();
  }
  return openAgent();
}

/**
 * Restart the operator's session (the `lost` recovery) and refresh status
 * afterward. If the session row is not in the store — the panel was never
 * opened, or status has not resolved yet — this does nothing: there is
 * nothing to restart.
 */
export async function restartOperator(): Promise<void> {
  const session = get(operatorSession);
  if (!session) return;
  const r = await restartSession(session.host_alias, session.tmux_name);
  if (!r.ok) return;
  await refreshOperator();
}
