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
    expect(calls('pty_close').at(-1)![1]).toEqual({ args: { id: calls('pty_open')[0][1].args.id } });
    // Nothing drains a terminal nobody shows.
    await new Promise((r) => setTimeout(r, 50));
    expect(calls('pty_drain')).toHaveLength(0);
  });

  it("a view remounted while the last one's open was in flight keeps its terminal when that open ends", async () => {
    const finishes: ((gen: number) => void)[] = [];
    inv.mockImplementation((cmd: string) => {
      if (cmd === 'pty_open') return new Promise((r) => finishes.push(r));
      return Promise.resolve(null);
    });
    const first = render(FleetTerminal, { props: { fleet: 'company', session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(1));
    first.unmount();
    render(FleetTerminal, { props: { fleet: 'company', session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(2));
    const [oldId, newId] = calls('pty_open').map(([, a]) => a.args.id as string);
    expect(oldId).not.toBe(newId);
    expect(newId).toMatch(/^fleet-company-3\.\d+$/);
    expect(newId).toMatch(/^[A-Za-z0-9_.:-]{1,64}$/);
    finishes[1](8);
    finishes[0](7);
    await vi.waitFor(() => expect(calls('pty_close').some(([, a]) => a.args.id === oldId)).toBe(true));
    // The old view's late close ends its own attach, never the new one's.
    expect(calls('pty_close').some(([, a]) => a.args.id === newId)).toBe(false);
  });

  it("never opens this machine's tmux for a session on that hub's own machine", async () => {
    inv.mockResolvedValue(null);
    const { getByTestId } = render(FleetTerminal, { props: { fleet: 'company', session: { ...session, host_alias: 'local' } } });
    await tick();
    expect(calls('pty_open')).toHaveLength(0);
    expect(getByTestId('fleet-terminal-refused')).toBeTruthy();
  });
});
