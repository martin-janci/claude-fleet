import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { toasts, push, dismiss, clearToasts, pushError, pushResultError, INFO_TIMEOUT_MS } from './toasts';

beforeEach(() => {
  vi.useFakeTimers();
  clearToasts();
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
    pushError({ code: 'E_SSH', message: 'connection refused' }, 'Kill failed');
    expect(get(toasts)[0]).toMatchObject({ kind: 'error', code: 'E_SSH', message: 'Kill failed: connection refused' });
  });

  it('pushResultError is a no-op on Ok and surfaces Err', () => {
    expect(pushResultError({ ok: true, value: 1 })).toBeNull();
    expect(get(toasts)).toHaveLength(0);
    const id = pushResultError({ ok: false, error: { code: 'E_DB', message: 'locked' } });
    expect(id).not.toBeNull();
    expect(get(toasts)[0].code).toBe('E_DB');
  });
});
