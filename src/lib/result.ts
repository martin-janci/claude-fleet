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
