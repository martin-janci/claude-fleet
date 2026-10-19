// An extra fleet's terminal: an attach still in flight at unmount must not
// leave this machine's ssh attach running behind a closed view.
import { render } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FleetTerminal from './FleetTerminal.svelte';
import type { SessionRow } from './sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const session = { id: 3, tmux_name: 'pos-3', host_alias: 'mac' } as SessionRow;

function calls(cmd: string) {
  return inv.mock.calls.filter(([c]) => c === cmd);
}

beforeEach(() => {
  inv.mockReset();
});

describe('FleetTerminal', () => {
  it('closes a terminal whose open was still in flight when the view went away', async () => {
    let finishOpen: (gen: number) => void = () => {};
    inv.mockImplementation((cmd: string) => {
      if (cmd === 'pty_open') return new Promise((r) => (finishOpen = r));
      return Promise.resolve(null);
    });
    const { unmount } = render(FleetTerminal, { props: { fleet: 'company', session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(1));
    unmount();
    const closesBefore = calls('pty_close').length;
    finishOpen(7);
    await vi.waitFor(() => expect(calls('pty_close').length).toBeGreaterThan(closesBefore));
    expect(calls('pty_close').at(-1)![1]).toEqual({ args: { id: 'fleet-company-3' } });
    // Nothing drains a terminal nobody shows.
    await new Promise((r) => setTimeout(r, 50));
    expect(calls('pty_drain')).toHaveLength(0);
  });

  it("never opens this machine's tmux for a session on that hub's own machine", async () => {
    inv.mockResolvedValue(null);
    const { getByTestId } = render(FleetTerminal, { props: { fleet: 'company', session: { ...session, host_alias: 'local' } } });
    await tick();
    expect(calls('pty_open')).toHaveLength(0);
    expect(getByTestId('fleet-terminal-refused')).toBeTruthy();
  });
});
