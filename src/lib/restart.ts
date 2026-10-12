import type { IpcError } from './result';
import { push, pushError } from './toasts';

/**
 * Surface a failed `restart_session` from a Restart button.
 *
 * Every failure is the usual sticky "Restart failed" toast, except
 * `E_SELF_TARGET`: the backend refuses to restart the registered fleet
 * controller without `force`, and no button could pass it, so the controller
 * had no Restart at all ("fleet-operator on mefistos is the registered fleet
 * controller; pass force=true", 2026-10-11). That refusal becomes a question
 * instead, with a "Restart anyway" a person has to click: the guard still
 * stops every restart nobody asked for, and the person who did ask gets a
 * way through. A restart keeps the row and the conversation.
 *
 * `restartForced` is the caller's own restart again with `force: true`, so
 * it passes the caller's access gate a second time at click time.
 */
export function pushRestartError(
  error: IpcError,
  hostAlias: string,
  name: string,
  restartForced: () => void,
): number {
  if (error.code !== 'E_SELF_TARGET') return pushError(error, 'Restart failed');
  return push({
    kind: 'warning',
    code: error.code,
    message: `${name} on ${hostAlias} is the registered fleet controller. Restart it anyway?`,
    sticky: true,
    action: { label: 'Restart anyway', run: restartForced },
  });
}
