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

  it('loads requests that were pending before mount', async () => {
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'mcp_pending_confirms' ? [{ nonce: 'p', tool: 'delete_worktree' }] : null,
    );
    render(McpConfirmDialog);
    await tick(); await tick();
    expect((await screen.findByTestId('mcp-confirm')).textContent).toContain('delete_worktree');
  });
});
