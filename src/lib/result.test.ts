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

  // The `fleet:outcome-unknown` broadcast makes every listener re-fetch the
  // whole fleet. A hub timeout on a READ changes nothing on the hub, and a
  // refresh that itself times out would broadcast again — the amplification
  // the final review found. Only a timed-out MUTATION (which the backend
  // marks `outcome_unknown: true`) may fire it.
  describe('fleet:outcome-unknown', () => {
    async function dispatchesFor(error: unknown): Promise<number> {
      let fired = 0;
      const onEvent = () => {
        fired += 1;
      };
      window.addEventListener('fleet:outcome-unknown', onEvent);
      try {
        (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce(error);
        await invokeCmd<unknown>('some_cmd');
      } finally {
        window.removeEventListener('fleet:outcome-unknown', onEvent);
      }
      return fired;
    }

    it('fires for a timed-out mutation (outcome_unknown: true)', async () => {
      expect(
        await dispatchesFor({
          code: 'E_HUB_TIMEOUT',
          message: 'no answer within 40s',
          details: { outcome_unknown: true },
        }),
      ).toBe(1);
    });

    it('stays silent for a timed-out read (outcome_unknown: false)', async () => {
      expect(
        await dispatchesFor({
          code: 'E_HUB_TIMEOUT',
          message: 'no answer within 40s',
          details: { outcome_unknown: false },
        }),
      ).toBe(0);
    });

    it('stays silent for an E_HUB_TIMEOUT carrying no details at all', async () => {
      expect(await dispatchesFor({ code: 'E_HUB_TIMEOUT', message: 'no answer within 40s' })).toBe(0);
    });

    it('stays silent for E_HUB_UNREACHABLE', async () => {
      expect(
        await dispatchesFor({
          code: 'E_HUB_UNREACHABLE',
          message: 'refused',
          details: { outcome_unknown: true },
        }),
      ).toBe(0);
    });
  });
});
