import { invoke } from '@tauri-apps/api/core';

export interface IpcError {
  code: string;
  message: string;
  details?: unknown;
}

export type Result<T> = { ok: true; value: T } | { ok: false; error: IpcError };

export async function invokeCmd<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<Result<T>> {
  try {
    const value = await invoke<T>(cmd, args);
    return { ok: true, value };
  } catch (raw) {
    return { ok: false, error: toIpcError(raw) };
  }
}

let _callIdCounter = 1;
function nextCallId(): number {
  return _callIdCounter++;
}

/**
 * Like `invokeCmd`, but takes an optional `AbortSignal` for cancellation. The
 * helper mints a monotonic frontend `call_id`, injects it into the command's
 * `args`, and on abort fires a `cancel_command(call_id)` IPC fire-and-forget.
 * The backend's command handler is expected to register the token under the
 * same `call_id` in the `CancellationRegistry`.
 *
 * The function returns the underlying invoke `Result<T>`. If the signal was
 * already aborted before the call, returns `Err({ code: 'E_CANCELLED' })`
 * synchronously without invoking.
 */
export async function invokeCmdAbortable<T>(
  cmd: string,
  args: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<Result<T>> {
  if (signal?.aborted) {
    return { ok: false, error: { code: 'E_CANCELLED', message: 'aborted before invoke' } };
  }
  const call_id = nextCallId();
  // Inject call_id into the args wrapper. The IPC convention in this codebase
  // is { args: { ...fields } } — we merge call_id into that inner object.
  const baseArgs = (args.args as Record<string, unknown> | undefined) ?? {};
  const fullArgs = { args: { ...baseArgs, call_id } };

  let onAbort: (() => void) | undefined;
  if (signal) {
    onAbort = () => {
      // Fire-and-forget: cancel_command takes call_id as a top-level Tauri param.
      void invokeCmd('cancel_command', { call_id }).catch(() => undefined);
    };
    signal.addEventListener('abort', onAbort, { once: true });
  }

  try {
    return await invokeCmd<T>(cmd, fullArgs);
  } finally {
    if (signal && onAbort) {
      signal.removeEventListener('abort', onAbort);
    }
  }
}

function toIpcError(raw: unknown): IpcError {
  if (raw && typeof raw === 'object' && 'code' in raw && 'message' in raw) {
    const r = raw as { code: unknown; message: unknown; details?: unknown };
    if (typeof r.code === 'string' && typeof r.message === 'string') {
      return { code: r.code, message: r.message, details: r.details };
    }
  }
  if (raw instanceof Error) {
    return { code: 'E_UNKNOWN', message: raw.message };
  }
  return { code: 'E_UNKNOWN', message: String(raw) };
}
