import { describe, expect, it, vi } from 'vitest';
import { writable } from 'svelte/store';
import { trackViewedSession, wantsViewedStamp } from './session_viewed';
import type { SessionRow } from './sessions';

function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1,
    kind: 'work',
    started_at: 100,
    last_stop_at: null,
    last_viewed_at: 50,
    ...over,
  } as SessionRow;
}

function fakeDoc(state: DocumentVisibilityState = 'visible') {
  const listeners = new Set<() => void>();
  return {
    visibilityState: state,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    fire() {
      for (const fn of listeners) fn();
    },
    listeners,
  };
}

describe('session_viewed', () => {
  it('wants a stamp only when the open session is unread or never viewed', () => {
    expect(wantsViewedStamp(null)).toBe(false);
    expect(wantsViewedStamp(row())).toBe(false);
    expect(wantsViewedStamp(row({ last_stop_at: 200 }))).toBe(true);
    expect(wantsViewedStamp(row({ last_viewed_at: null }))).toBe(true);
    expect(wantsViewedStamp(row({ kind: 'external', last_viewed_at: null }))).toBe(false);
  });

  it('stamps the open session once per finished turn', () => {
    const selected = writable<SessionRow | null>(null);
    const touch = vi.fn();
    const doc = fakeDoc();
    const stop = trackViewedSession(selected, touch, doc as unknown as Document);
    selected.set(row());
    expect(touch).not.toHaveBeenCalled();
    selected.set(row({ last_stop_at: 200 }));
    expect(touch).toHaveBeenCalledTimes(1);
    expect(touch).toHaveBeenCalledWith(1);
    // The same turn again (a refused call, or another row event): not again.
    selected.set(row({ last_stop_at: 200, context_pct: 3 }));
    expect(touch).toHaveBeenCalledTimes(1);
    // The next turn ends while it is open.
    selected.set(row({ last_stop_at: 300 }));
    expect(touch).toHaveBeenCalledTimes(2);
    stop();
    expect(doc.listeners.size).toBe(0);
    selected.set(row({ last_stop_at: 400 }));
    expect(touch).toHaveBeenCalledTimes(2);
  });

  it('waits until the window is visible again', () => {
    const selected = writable<SessionRow | null>(null);
    const touch = vi.fn();
    const doc = fakeDoc('hidden');
    trackViewedSession(selected, touch, doc as unknown as Document);
    selected.set(row({ last_stop_at: 200 }));
    expect(touch).not.toHaveBeenCalled();
    doc.visibilityState = 'visible';
    doc.fire();
    expect(touch).toHaveBeenCalledTimes(1);
  });

  it('waits while another page covers the session', () => {
    const selected = writable<SessionRow | null>(null);
    const shown = writable(false);
    const touch = vi.fn();
    trackViewedSession(selected, touch, fakeDoc() as unknown as Document, shown);
    selected.set(row({ last_stop_at: 200 }));
    expect(touch).not.toHaveBeenCalled();
    shown.set(true);
    expect(touch).toHaveBeenCalledTimes(1);
  });
});
