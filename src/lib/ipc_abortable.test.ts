import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { invokeCmdAbortable } from './result';

describe('invokeCmdAbortable', () => {
  beforeEach(() => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  });

  it('injects call_id into args', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    const ac = new AbortController();
    await invokeCmdAbortable('foo', { args: { x: 1 } }, ac.signal);
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find(
      (c) => c[0] === 'foo',
    );
    expect(call).toBeDefined();
    expect((call![1] as { args: { x: number; call_id: number } }).args.x).toBe(1);
    expect((call![1] as { args: { call_id: number } }).args.call_id).toEqual(expect.any(Number));
  });

  it('fires cancel_command on abort', async () => {
    let resolveInvoke: (v: unknown) => void = () => {};
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation((cmd: string) => {
      if (cmd === 'cancel_command') return Promise.resolve(null);
      return new Promise((res) => {
        resolveInvoke = res;
      });
    });
    const ac = new AbortController();
    const p = invokeCmdAbortable('long_op', { args: {} }, ac.signal);
    ac.abort();
    await new Promise((r) => setTimeout(r, 0));
    const sawCancel = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
      (c) => c[0] === 'cancel_command',
    );
    expect(sawCancel).toBe(true);
    resolveInvoke(null);
    await p;
  });

  it('returns E_CANCELLED synchronously if signal already aborted', async () => {
    const ac = new AbortController();
    ac.abort();
    const r = await invokeCmdAbortable('foo', { args: {} }, ac.signal);
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.code).toBe('E_CANCELLED');
    }
  });
});
