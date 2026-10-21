// Multi-hub Phase 3: another fleet's session in the same TerminalView. The
// attach is this machine's own ssh under that fleet's pty id, and nothing that
// speaks to this window's own hub by id runs for it (its ids are another
// hub's).
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }),
}));
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ readText: vi.fn(), writeText: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TerminalView from './TerminalView.svelte';
import type { SessionRow } from './sessions';
import { terminalPane } from './terminals';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const calls = (cmd: string) => inv.mock.calls.filter(([c]) => c === cmd);
const session = {
  id: 3,
  tmux_name: 'pos-3',
  host_alias: 'mac',
  project_id: 9,
  kind: 'work',
  status: 'running',
  fleet: 'company',
} as SessionRow;

class FakeResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => (cmd === 'pty_drain' ? { data: '', bytes: 0 } : null));
  vi.stubGlobal('ResizeObserver', FakeResizeObserver);
});
afterEach(() => vi.unstubAllGlobals());

describe("TerminalView: another fleet's session", () => {
  it("attaches under that fleet's own pty id and touches nothing on this window's hub", async () => {
    render(TerminalView, { props: { session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(1));
    const open = calls('pty_open')[0][1].args;
    expect(open).toMatchObject({ session_name: 'pos-3', host_alias: 'mac' });
    expect(open.id).toMatch(/^fleet-company-3\.\d+$/);
    expect(open.id).toMatch(/^[A-Za-z0-9_.:-]{1,64}$/);
    await tick();
    for (const cmd of ['repair_session', 'shell_terminals', 'unarchive_session', 'upload_to_session']) {
      expect(calls(cmd)).toHaveLength(0);
    }
    // No strip, and the main window's bar is left alone.
    expect(screen.queryByTestId('terminal-strip')).toBeNull();
    expect(get(terminalPane).sessionId).toBeNull();
  });

  it('closes a terminal whose open was still in flight when the view went away', async () => {
    let finishOpen: (gen: number) => void = () => {};
    inv.mockImplementation((cmd: string) => {
      if (cmd === 'pty_open') return new Promise((r) => (finishOpen = r));
      return Promise.resolve(null);
    });
    const { unmount } = render(TerminalView, { props: { session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(1));
    const id = calls('pty_open')[0][1].args.id;
    unmount();
    finishOpen(7);
    await vi.waitFor(() => expect(calls('pty_close').some(([, a]) => a.args.id === id)).toBe(true));
    await new Promise((r) => setTimeout(r, 50));
    expect(calls('pty_drain')).toHaveLength(0);
  });

  it('two views of one session never share a pty id', async () => {
    render(TerminalView, { props: { session } });
    render(TerminalView, { props: { session } });
    await vi.waitFor(() => expect(calls('pty_open')).toHaveLength(2));
    const [a, b] = calls('pty_open').map(([, p]) => p.args.id as string);
    expect(a).not.toBe(b);
  });

  it("never opens this machine's tmux for a session on that hub's own machine", async () => {
    render(TerminalView, { props: { session: { ...session, host_alias: 'local' } } });
    await tick();
    await tick();
    expect(calls('pty_open')).toHaveLength(0);
    expect(screen.getByTestId('terminal-no-attach').textContent).toContain("that hub's own machine");
  });
});
