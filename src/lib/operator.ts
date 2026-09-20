// The UX agent's state, as the panel needs it. Follows `app_views.ts`: the
// FAB and the panel talk through these stores instead of prop-drilling
// through App.svelte.
import { get, writable, type Writable } from 'svelte/store';
import { invokeCmd } from './result';
import { restartSession, type SessionRow } from './sessions';

export type OperatorBlocked = 'absent' | 'lost' | 'no_mcp' | 'token_revoked';

export interface OperatorStatus {
  ready: boolean;
  session: SessionRow | null;
  blocked: OperatorBlocked | null;
}

export const agentPanelOpen: Writable<boolean> = writable(false);
export const operatorState: Writable<'unknown' | 'waking' | 'ready' | OperatorBlocked> =
  writable('unknown');
export const operatorSession: Writable<SessionRow | null> = writable(null);

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
export function blockedCopy(b: OperatorBlocked): { title: string; action: string | null } {
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
  operatorState.set(r.value.ready ? 'ready' : (r.value.blocked ?? 'absent'));
}

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
export async function openAgent(): Promise<void> {
  agentPanelOpen.set(true);
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
