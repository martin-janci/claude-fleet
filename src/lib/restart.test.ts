import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { get } from 'svelte/store';
import { pushRestartError } from './restart';
import { restartSession, type SessionRow } from './sessions';
import { toasts, clearToasts, runToastAction } from './toasts';

const row = { id: 7, tmux_name: 'fleet-operator', host_alias: 'mefistos' } as unknown as SessionRow;
const selfTarget = {
  code: 'E_SELF_TARGET',
  message: 'fleet-operator on mefistos is the registered fleet controller; pass force=true to target it anyway',
};

beforeEach(() => {
  invoke.mockReset();
  clearToasts();
});

describe('restartSession: the controller guard', () => {
  it('a plain restart never overrides the guard', async () => {
    invoke.mockResolvedValueOnce(row);
    await restartSession('mefistos', 'fleet-operator');
    expect(invoke.mock.calls[0][1].args).not.toHaveProperty('force');
  });

  it('force=true reaches the command only when asked for', async () => {
    invoke.mockResolvedValueOnce(row);
    await restartSession('mefistos', 'fleet-operator', undefined, {}, { force: true });
    expect(invoke.mock.calls[0]).toEqual([
      'restart_session',
      { args: { host_alias: 'mefistos', name: 'fleet-operator', force: true } },
    ]);
  });
});

// 2026-10-11: every Restart of the registered fleet controller answered
// E_SELF_TARGET, and no button could pass `force`.
describe('pushRestartError', () => {
  it('turns E_SELF_TARGET into a question whose button is the forced restart', () => {
    const forced = vi.fn();
    const id = pushRestartError(selfTarget, 'mefistos', 'fleet-operator', forced);
    const t = get(toasts).find((x) => x.id === id)!;
    expect(t.kind).toBe('warning');
    expect(t.message).toBe('fleet-operator on mefistos is the registered fleet controller. Restart it anyway?');
    expect(t.action?.label).toBe('Restart anyway');
    // Nothing is forced until the person clicks.
    expect(forced).not.toHaveBeenCalled();
    runToastAction(id);
    expect(forced).toHaveBeenCalledTimes(1);
  });

  it('every other failure is the plain error toast, with no override offered', () => {
    const forced = vi.fn();
    const id = pushRestartError({ code: 'E_NOTFOUND', message: 'no such session' }, 'local', 'dev-a', forced);
    const t = get(toasts).find((x) => x.id === id)!;
    expect(t.kind).toBe('error');
    expect(t.message.startsWith('Restart failed')).toBe(true);
    expect(t.action).toBeNull();
  });
});
