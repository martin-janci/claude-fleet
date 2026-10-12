// Redesign 11.7b: presence — who else has the open session on screen.
import { describe, expect, it, vi } from 'vitest';
import { get, writable } from 'svelte/store';
import { render, screen } from '@testing-library/svelte';
import { othersLooking, presence, trackPresence, viewerTitle, type PresenceView } from './presence';
import PresenceStrip from './PresenceStrip.svelte';
import type { SessionRow } from './sessions';

const row = (id: number, over: Partial<SessionRow> = {}) => ({ id, kind: 'work', ...over }) as SessionRow;

function view(id: number, viewers: PresenceView['viewers'] = []): PresenceView {
  return { session_id: id, viewers, heartbeat_secs: 20 };
}

function fakeDoc() {
  const listeners = new Set<() => void>();
  return {
    visibilityState: 'visible' as DocumentVisibilityState,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
    fire() {
      for (const fn of listeners) fn();
    },
  };
}

function harness(enabled = true) {
  const timers: Array<() => void> = [];
  const report = vi.fn(async (id: number, leaving = false) =>
    leaving ? { ok: true as const, value: view(id) } : { ok: true as const, value: view(id, [{ person_id: 2, name: 'bea', since: 1 }]) },
  );
  const doc = fakeDoc();
  const selected = writable<SessionRow | null>(null);
  const stop = trackPresence(selected, {
    report,
    enabled: () => enabled,
    doc: doc as never,
    setTimer: (fn) => timers.push(fn),
    clearTimer: () => {},
  });
  return { report, doc, selected, stop, timers };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe('presence (11.7b)', () => {
  it('reports the open session, keeps the answer, and leaves the one it closes', async () => {
    const h = harness();
    h.selected.set(row(7));
    await flush();
    expect(h.report).toHaveBeenCalledWith(7);
    expect(get(presence)?.session_id).toBe(7);
    expect(h.timers).toHaveLength(1);
    h.selected.set(row(8));
    await flush();
    expect(h.report).toHaveBeenCalledWith(7, true);
    expect(h.report).toHaveBeenCalledWith(8);
    expect(get(presence)?.session_id).toBe(8);
    h.stop();
    expect(h.report).toHaveBeenCalledWith(8, true);
    expect(get(presence)).toBeNull();
  });

  it('a heartbeat reports again; a hidden window does not', async () => {
    const h = harness();
    h.selected.set(row(7));
    await flush();
    h.timers[0]();
    await flush();
    expect(h.report.mock.calls.filter((c) => c[0] === 7 && c.length === 1)).toHaveLength(2);
    h.doc.visibilityState = 'hidden';
    h.doc.fire();
    h.timers[1]?.();
    await flush();
    expect(h.report.mock.calls.filter((c) => c[0] === 7 && c.length === 1)).toHaveLength(2);
    h.stop();
  });

  it('standalone (no hub) sends nothing', async () => {
    const h = harness(false);
    h.selected.set(row(7));
    await flush();
    expect(h.report).not.toHaveBeenCalled();
    h.stop();
  });

  it('a refused report (an older hub) clears and stops asking', async () => {
    const h = harness();
    h.report.mockResolvedValueOnce({ ok: false, error: { code: 'E_UNKNOWN', message: 'unknown tool' } } as never);
    h.selected.set(row(7));
    await flush();
    expect(get(presence)).toBeNull();
    expect(h.timers).toHaveLength(0);
    h.stop();
  });

  it('others are everyone but this device, one per person', () => {
    const v = view(7, [
      { person_id: 1, name: 'me', since: 1, you: true },
      { person_id: 2, name: 'bea', device: 'bea-phone', since: 2 },
      { person_id: 2, name: 'bea', device: 'bea-mac', since: 3 },
      { person_id: 3, name: 'cy', since: 4 },
    ]);
    expect(othersLooking(v).map((x) => x.name)).toEqual(['bea', 'cy']);
    expect(viewerTitle(othersLooking(v)[0])).toBe('bea is looking at this session, from bea-phone');
    expect(othersLooking(null)).toEqual([]);
  });

  it('the strip names who else is looking, and nothing when nobody is', async () => {
    presence.set(view(7, [{ person_id: 1, name: 'me', since: 1, you: true }]));
    const { unmount } = render(PresenceStrip, { props: { sessionId: 7 } });
    expect(screen.queryByTestId('presence-strip')).toBeNull();
    presence.set(view(7, [{ person_id: 1, name: 'me', since: 1, you: true }, { person_id: 2, name: 'Peter', device: 'iPhone', since: 2 }]));
    await flush();
    expect(screen.getByTestId('presence-strip').textContent).toContain('Peter watching');
    expect(screen.getByTestId('presence-face').getAttribute('title')).toContain('from iPhone');
    // Another session's answer is not this one's.
    presence.set(view(8, [{ person_id: 2, name: 'Peter', since: 2 }]));
    await flush();
    expect(screen.queryByTestId('presence-strip')).toBeNull();
    unmount();
    presence.set(null);
  });
});
