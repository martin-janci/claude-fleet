import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { invokeCmd, type IpcError } from './result';

describe('invokeCmd', () => {
  it('returns Ok on resolved invoke', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ a: 1 });
    const r = await invokeCmd<{ a: number }>('ok_cmd');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual({ a: 1 });
  });

  it('returns Err on rejected invoke carrying a structured IpcError', async () => {
    const ipcErr: IpcError = { code: 'E_TEST', message: 'boom' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(ipcErr);
    const r = await invokeCmd<unknown>('fail_cmd');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_TEST');
      expect(r.error.message).toBe('boom');
    }
  });

  it('wraps plain Error rejections into IpcError with E_UNKNOWN', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('explode'));
    const r = await invokeCmd<unknown>('throwy_cmd');
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_UNKNOWN');
      expect(r.error.message).toContain('explode');
    }
  });
});
