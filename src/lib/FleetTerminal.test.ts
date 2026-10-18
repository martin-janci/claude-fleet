// An extra fleet's terminal: what it attaches, and what it leaves behind.
import { render } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FleetTerminal from './FleetTerminal.svelte';
import type { SessionRow } from './sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const session = { id: 3, tmux_name: 'pos-3', host_alias: 'mac', status: 'running', last_activity_at: 1 } as SessionRow;

beforeEach(() => {
  inv.mockReset();
  inv.mockResolvedValue(null);
});

describe('FleetTerminal', () => {
  it('closes an attach that finishes opening after the view is gone, and starts no drain', async () => {
    let opened!: (v: unknown) => void;
    inv.mockImplementation((cmd: string) =>
      cmd === 'pty_open' ? new Promise((r) => (opened = r)) : Promise.resolve({ data: '', bytes: 0, eof: false, overflowed: false }),
    );
    const view = render(FleetTerminal, { props: { fleet: 'company', session } });
    await vi.waitFor(() => expect(inv.mock.calls.some(([cmd]) => cmd === 'pty_open')).toBe(true));
    view.unmount();
    inv.mockClear();
    vi.useFakeTimers();
    try {
      opened(1);
      await vi.advanceTimersByTimeAsync(1000);
    } finally {
      vi.useRealTimers();
    }
    expect(inv.mock.calls).toEqual([['pty_close', { args: { id: 'fleet-company-3' } }]]);
  });

  it("never opens this machine's tmux for a session on that hub's own machine", async () => {
    render(FleetTerminal, { props: { fleet: 'company', session: { ...session, host_alias: 'local' } } });
    await tick();
    expect(inv.mock.calls.some(([cmd]) => cmd === 'pty_open')).toBe(false);
  });
});
