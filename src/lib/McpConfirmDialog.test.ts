import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  return {
    listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
      handlers.set(name, cb);
      return () => handlers.delete(name);
    }),
    emit: vi.fn(async (name: string, payload: unknown) => {
      handlers.get(name)?.({ payload });
    }),
  };
});

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import McpConfirmDialog from './McpConfirmDialog.svelte';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'mcp_pending_confirms':
        return [];
      case 'mcp_confirm':
        return true;
      default:
        return null;
    }
  });
});

describe('McpConfirmDialog', () => {
  it('renders nothing until a confirmation request arrives', async () => {
    render(McpConfirmDialog);
    await tick();
    expect(screen.queryByTestId('mcp-confirm')).toBeNull();
  });

  it('shows the request and answers approve via mcp_confirm with the nonce', async () => {
    render(McpConfirmDialog);
    await tick();
    await emit('mcp:confirm-required', {
      nonce: 'n-1',
      tool: 'kill_session',
      summary: 'host=local name=dev-x',
      caller: 'host:mefistos',
    });
    await tick();
    const dialog = await screen.findByTestId('mcp-confirm');
    expect(dialog.textContent).toContain('kill_session');
    expect(dialog.textContent).toContain('host=local name=dev-x');
    expect(dialog.textContent).toContain('host:mefistos');
    await fireEvent.click(screen.getByTestId('mcp-confirm-approve'));
    await tick(); await tick();
    const call = inv.mock.calls.find((c) => c[0] === 'mcp_confirm');
    expect(call).toBeDefined();
    expect(call![1]).toEqual({ nonce: 'n-1', approved: true });
    expect(screen.queryByTestId('mcp-confirm')).toBeNull();
  });

  it('deny sends approved=false and the queue advances to the next request', async () => {
    render(McpConfirmDialog);
    await tick();
    await emit('mcp:confirm-required', { nonce: 'a', tool: 'broadcast_prompt', summary: '', caller: 'master' });
    await emit('mcp:confirm-required', { nonce: 'b', tool: 'set_clipboard', summary: '', caller: 'master' });
    // Duplicate nonce is ignored.
    await emit('mcp:confirm-required', { nonce: 'a', tool: 'broadcast_prompt', summary: '', caller: 'master' });
    await tick();
    expect(screen.getByTestId('mcp-confirm').textContent).toContain('broadcast_prompt');
    expect(screen.getByTestId('mcp-confirm').textContent).toContain('1 more waiting');
    await fireEvent.click(screen.getByTestId('mcp-confirm-deny'));
    await tick(); await tick();
    const call = inv.mock.calls.find((c) => c[0] === 'mcp_confirm');
    expect(call![1]).toEqual({ nonce: 'a', approved: false });
    expect(screen.getByTestId('mcp-confirm').textContent).toContain('set_clipboard');
  });

  // The dialog appears unannounced over whatever the user is doing, and
  // showModal() steals focus — so a Space/Enter already in flight lands on
  // the focused button. That button must be the safe one.
  it('gives initial focus to Deny, not Approve', async () => {
    render(McpConfirmDialog);
    await tick();
    await emit('mcp:confirm-required', { nonce: 'n-1', tool: 'kill_session', summary: '', caller: 'master' });
    await tick(); await tick();
    // Real focus, not just the attribute that asks for it: asserting the
    // attribute alone passes even if Modal.svelte stops focusing anything at
    // all, which is the half that actually protects the user.
    expect(document.activeElement).toBe(screen.getByTestId('mcp-confirm-deny'));
    expect(document.activeElement).not.toBe(screen.getByTestId('mcp-confirm-approve'));
    // Deny also happens to be first in DOM order here, so focus alone cannot
    // prove the attribute is what put it there — keep asserting the marker so
    // a reordering of the two buttons is still caught.
    expect(screen.getByTestId('mcp-confirm-deny')).toHaveAttribute('data-autofocus');
    expect(screen.getByTestId('mcp-confirm-approve')).not.toHaveAttribute('data-autofocus');
  });

  it('keeps the request queued and reports when the verdict fails to land', async () => {
    const { toasts, clearToasts } = await import('./toasts');
    const { get } = await import('svelte/store');
    clearToasts();
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'mcp_pending_confirms') return [];
      if (cmd === 'mcp_confirm') throw new Error('ipc down');
      return null;
    });
    render(McpConfirmDialog);
    await tick();
    await emit('mcp:confirm-required', { nonce: 'n-1', tool: 'kill_session', summary: '', caller: 'master' });
    await tick();
    await fireEvent.click(screen.getByTestId('mcp-confirm-deny'));
    await tick(); await tick();
    // Still on screen: the user has NOT denied anything yet.
    expect(screen.getByTestId('mcp-confirm').textContent).toContain('kill_session');
    // …and it says so. Silently re-queueing would leave the user believing
    // they had refused the agent. (Without this the `pushError` line could be
    // deleted and the test would still pass.)
    const shown = get(toasts);
    expect(shown).toHaveLength(1);
    expect(shown[0].kind).toBe('error');
    expect(shown[0].message).toContain('Denying kill_session failed');
    clearToasts();
  });

  it('loads requests that were pending before mount', async () => {
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'mcp_pending_confirms' ? [{ nonce: 'p', tool: 'delete_worktree' }] : null,
    );
    render(McpConfirmDialog);
    await tick(); await tick();
    expect((await screen.findByTestId('mcp-confirm')).textContent).toContain('delete_worktree');
  });
});
