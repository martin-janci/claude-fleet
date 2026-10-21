vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { resetErrorReportingForTests } from './error_report';
import { toasts, push, dismiss, clearToasts, pushError, pushResultError, runToastAction, INFO_TIMEOUT_MS, ACTION_TIMEOUT_MS } from './toasts';

const inv = () => invoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  clearToasts();
  inv().mockReset();
  inv().mockResolvedValue(undefined);
  resetErrorReportingForTests();
});
afterEach(() => {
  vi.useRealTimers();
});

describe('toasts store', () => {
  it('push adds a toast with kind, code and message', () => {
    push({ kind: 'error', code: 'E_TMUX', message: 'tmux exited' });
    const all = get(toasts);
    expect(all).toHaveLength(1);
    expect(all[0]).toMatchObject({ kind: 'error', code: 'E_TMUX', message: 'tmux exited', sticky: true, count: 1 });
  });

  it('errors are sticky; info auto-dismisses', () => {
    push({ kind: 'error', message: 'boom' });
    push({ kind: 'info', message: 'saved' });
    expect(get(toasts)).toHaveLength(2);
    vi.advanceTimersByTime(INFO_TIMEOUT_MS + 1);
    const left = get(toasts);
    expect(left).toHaveLength(1);
    expect(left[0].message).toBe('boom');
  });

  it('dedupes by code+message and bumps the counter instead of stacking', () => {
    const a = push({ kind: 'error', code: 'E_PTY', message: 'write failed' });
    const b = push({ kind: 'error', code: 'E_PTY', message: 'write failed' });
    const c = push({ kind: 'error', code: 'E_PTY', message: 'write failed' });
    expect(a).toBe(b);
    expect(b).toBe(c);
    const all = get(toasts);
    expect(all).toHaveLength(1);
    expect(all[0].count).toBe(3);
  });

  it('a different code with the same message is a separate toast', () => {
    push({ kind: 'error', code: 'E_A', message: 'same' });
    push({ kind: 'error', code: 'E_B', message: 'same' });
    expect(get(toasts)).toHaveLength(2);
  });

  it('re-pushing a visible info toast restarts its timer', () => {
    push({ kind: 'info', message: 'hi' });
    vi.advanceTimersByTime(INFO_TIMEOUT_MS - 100);
    push({ kind: 'info', message: 'hi' });
    vi.advanceTimersByTime(200);
    expect(get(toasts)).toHaveLength(1); // would have expired without the restart
    vi.advanceTimersByTime(INFO_TIMEOUT_MS);
    expect(get(toasts)).toHaveLength(0);
  });

  it('dismiss removes by id and cancels its timer', () => {
    const id = push({ kind: 'info', message: 'bye' });
    dismiss(id);
    expect(get(toasts)).toHaveLength(0);
    vi.advanceTimersByTime(INFO_TIMEOUT_MS + 1); // no throw, nothing to remove
    expect(get(toasts)).toHaveLength(0);
  });

  it('sticky override makes an info toast persist', () => {
    push({ kind: 'info', message: 'pinned', sticky: true });
    vi.advanceTimersByTime(INFO_TIMEOUT_MS * 2);
    expect(get(toasts)).toHaveLength(1);
  });

  it('pushError keeps the IpcError code and prefixes the context', () => {
    pushError({ code: 'E_NOTFOUND', message: 'no such session' }, 'Kill failed');
    expect(get(toasts)[0]).toMatchObject({ kind: 'error', code: 'E_NOTFOUND', message: 'Kill failed: no such session' });
  });

  // Review r13: transport noise is a sentence in the line and the backend's
  // words under Details; Retry re-runs what failed.
  it('pushError says a transport failure in a sentence and offers Retry', () => {
    const retry = vi.fn();
    pushError({ code: 'E_SSH', message: 'ssh: connect to host mercury port 22: Connection refused' }, 'Kill failed', retry);
    const t = get(toasts)[0];
    expect(t.message).toBe("Kill failed: Couldn't reach the host over SSH");
    expect(t.detail).toBe('E_SSH: ssh: connect to host mercury port 22: Connection refused');
    expect(t.action?.label).toBe('Retry');
    runToastAction(t.id);
    expect(retry).toHaveBeenCalledOnce();
  });

  it('pushError also reports to the hub error channel', () => {
    pushError({ code: 'E_SSH', message: 'ssh refused' }, 'Opening terminal');
    const reported = inv().mock.calls.filter((c) => c[0] === 'report_client_error');
    expect(reported).toHaveLength(1);
    expect((reported[0][1] as { args: { code: string; component: string } }).args).toMatchObject({ code: 'E_SSH', component: 'frontend' });
  });

  it('pushResultError is a no-op on Ok and surfaces Err', () => {
    expect(pushResultError({ ok: true, value: 1 })).toBeNull();
    expect(get(toasts)).toHaveLength(0);
    const id = pushResultError({ ok: false, error: { code: 'E_DB', message: 'locked' } });
    expect(id).not.toBeNull();
    expect(get(toasts)[0].code).toBe('E_DB');
  });

  it('an action toast stays up longer, and running its action dismisses it once', () => {
    const run = vi.fn();
    const id = push({ message: 'mefistos hidden', action: { label: 'Undo', run } });
    vi.advanceTimersByTime(INFO_TIMEOUT_MS + 1);
    expect(get(toasts)).toHaveLength(1);
    runToastAction(id);
    expect(run).toHaveBeenCalledTimes(1);
    expect(get(toasts)).toHaveLength(0);
    runToastAction(id);
    expect(run).toHaveBeenCalledTimes(1);
    push({ message: 'later', action: { label: 'Undo', run } });
    vi.advanceTimersByTime(ACTION_TIMEOUT_MS + 1);
    expect(get(toasts)).toHaveLength(0);
  });
});

// Requirement (c): with `mcp.confirm_destructive` on, a hub holds
// kill_session / delete_worktree / move_session / cancel_task until someone
// approves them. The request opens in this window's dialog (its queue is the
// hub's since redesign 9.2) and on the phone; the toast says so (review w16-3).
// Review w16-12: E_REPAIR_REQUIRED names one next step, Repair workspace,
// and a caller that knows the session offers it as the button.
describe('pushError with a repair', () => {
  it('offers Repair workspace for E_REPAIR_REQUIRED', () => {
    const repair = vi.fn();
    pushError({ code: 'E_REPAIR_REQUIRED', message: 'the workspace needs an explicit repair' }, 'Restart failed', undefined, { repair });
    const t = get(toasts).at(-1)!;
    expect(t.action?.label).toBe('Repair workspace…');
    t.action!.run();
    expect(repair).toHaveBeenCalled();
  });

  it('leaves any other error with its Retry', () => {
    const repair = vi.fn();
    pushError({ code: 'E_IO', message: 'disk full' }, 'Restart failed', () => {}, { repair });
    expect(get(toasts).at(-1)!.action?.label).toBe('Retry');
  });
});

describe('pushError on a hub client', () => {
  it('adds the next step to a confirmation the hub is waiting on', async () => {
    const { hubStatus, STANDALONE } = await import('./hub');
    hubStatus.set({
      ...STANDALONE,
      remote: true,
      url: 'https://fleet.example.com',
      client_name: 'laptop',
    });
    try {
      pushError({ code: 'E_CONFIRM_REQUIRED', message: 'confirmation required' }, 'Kill failed');
      const t = get(toasts)[0];
      expect(t.code).toBe('E_CONFIRM_REQUIRED');
      expect(t.message).toContain('Kill failed');
      expect(t.message).toContain('approving the request won’t run it from here');
      expect(t.message).not.toContain('fleet.example.com');
    } finally {
      hubStatus.set({ ...STANDALONE });
    }
  });

  it('changes nothing in standalone mode, where the dialog works', () => {
    pushError({ code: 'E_CONFIRM_REQUIRED', message: 'confirmation required' }, 'Kill failed');
    expect(get(toasts)[0].message).toBe('Kill failed: confirmation required');
  });

  it('leaves an error that already explains itself alone', async () => {
    const { hubStatus, STANDALONE } = await import('./hub');
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    try {
      pushError({ code: 'E_LOCAL_ONLY', message: 'not from here; do it on the hub' });
      expect(get(toasts)[0].message).toBe('not from here; do it on the hub');
    } finally {
      hubStatus.set({ ...STANDALONE });
    }
  });
});
