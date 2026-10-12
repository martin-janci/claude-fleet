import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

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
import { get } from 'svelte/store';
import ConfirmCards from './ConfirmCards.svelte';
import McpConfirmDialog from './McpConfirmDialog.svelte';
import { confirmQueue, resetConfirmsForTests } from './confirms';
import { clearToasts, toasts } from './toasts';
import { expectAccessible } from './a11y_check';

// Redesign step 9.2: the operator's confirms are cards in Control's
// transcript; everything else, and the operator's requests whenever no
// transcript shows, stays in the dialog.

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
let confirmOk = true;
let confirmAnswered = true;

const KILL = {
  nonce: 'n-op',
  tool: 'kill_session',
  summary: 'host=local name=dev-x',
  caller: 'client:ux-agent',
  operator: true,
  asked_at: Math.floor(Date.now() / 1000) - 120,
};
const OTHER = { nonce: 'n-host', tool: 'set_clipboard', summary: '', caller: 'host:mefistos', operator: false, asked_at: 0 };

beforeEach(() => {
  resetConfirmsForTests();
  confirmOk = true;
  confirmAnswered = true;
  clearToasts();
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd === 'mcp_pending_confirms') return [];
    if (cmd === 'mcp_confirm') {
      if (!confirmOk) throw { code: 'E_HUB', message: 'hub unreachable' };
      return confirmAnswered;
    }
    return null;
  });
});

async function mountBoth() {
  render(McpConfirmDialog);
  render(ConfirmCards);
  await tick();
}

describe('confirms as transcript cards', () => {
  it("the operator's kill is a card, not the dialog, while the transcript shows", async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await tick();
    const card = screen.getByTestId('confirm-card');
    expect(card.textContent).toContain('Kill a session?');
    expect(card.textContent).toContain('asked 2m ago');
    expect(screen.getByTestId('question-detail').textContent).toBe('kill_session host=local name=dev-x');
    expect(screen.queryByTestId('mcp-confirm')).toBeNull();
  });

  it('a request from anyone else stays in the dialog', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', OTHER);
    await tick();
    expect(screen.queryByTestId('confirm-card')).toBeNull();
    expect(screen.getByTestId('mcp-confirm')).toBeTruthy();
  });

  it('with no transcript on screen the operator request falls back to the dialog', async () => {
    render(McpConfirmDialog);
    const cards = render(ConfirmCards);
    await tick();
    await emit('mcp:confirm-required', KILL);
    await tick();
    expect(screen.queryByTestId('mcp-confirm')).toBeNull();
    cards.unmount();
    await tick();
    expect(screen.getByTestId('mcp-confirm')).toBeTruthy();
  });

  it('Approve answers the nonce once and removes the card', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-card-approve'));
    await tick();
    expect(inv).toHaveBeenCalledWith('mcp_confirm', { nonce: 'n-op', approved: true });
    expect(screen.queryByTestId('confirm-card')).toBeNull();
  });

  it('a Deny that never reached the backend keeps the card', async () => {
    confirmOk = false;
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-card-deny'));
    await tick();
    expect(inv).toHaveBeenCalledWith('mcp_confirm', { nonce: 'n-op', approved: false });
    expect(get(confirmQueue).map((r) => r.nonce)).toEqual(['n-op']);
    expect(screen.getByTestId('confirm-card')).toBeTruthy();
  });

  it('an answer the backend refused (expired, or answered elsewhere first) is said, not shown as done', async () => {
    confirmAnswered = false;
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-card-approve'));
    await tick();
    expect(get(confirmQueue)).toEqual([]);
    expect(get(toasts).map((t) => t.message).join('\n')).toContain('already been answered or had expired');
  });

  it('requests raised before the window mounted are listed again, operator flag and all', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'mcp_pending_confirms' ? [KILL, { nonce: 'old', tool: 'new_session' }] : true));
    await mountBoth();
    await tick();
    const q = get(confirmQueue);
    expect(q.map((r) => [r.nonce, r.operator])).toEqual([
      ['n-op', true],
      ['old', false],
    ]);
    expect(screen.getAllByTestId('confirm-card')).toHaveLength(1);
  });

  it("on a hub, confirm:changed brings the hub's request in and takes an answered one out", async () => {
    let listed: unknown[] = [];
    inv.mockImplementation(async (cmd: string) => (cmd === 'mcp_pending_confirms' ? listed : true));
    await mountBoth();
    listed = [KILL];
    await emit('confirm:changed', {});
    await tick();
    await tick();
    expect(screen.getAllByTestId('confirm-card')).toHaveLength(1);
    listed = [];
    await emit('confirm:changed', {});
    await tick();
    await tick();
    expect(screen.queryByTestId('confirm-card')).toBeNull();
  });
});

describe('ConfirmCards: accessibility', () => {
  it("the operator's pending kill, as a card, is accessible", async () => {
    render(McpConfirmDialog);
    const { container } = render(ConfirmCards);
    await tick();
    await emit('mcp:confirm-required', KILL);
    await tick();
    expect(screen.getByTestId('confirm-card')).toBeTruthy();
    await expectAccessible(container);
  });
});

// G4.9 (the fleet agent board): a plan of two or more steps is one card,
// "Confirm 2 · Cancel", answered step by step in the order asked.
describe('a multi-step plan is one card', () => {
  const MOVE = { ...KILL, nonce: 'n-move', tool: 'new_session', summary: 'host=mercury name=review', asked_at: KILL.asked_at + 5 };

  it('lists each step under one Confirm 2 / Cancel', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await emit('mcp:confirm-required', MOVE);
    await tick();
    expect(screen.queryByTestId('confirm-card')).toBeNull();
    const card = screen.getByTestId('confirm-plan');
    expect(card.textContent).toContain('2 steps need your OK');
    expect(screen.getAllByTestId('confirm-plan-step').map((li) => li.textContent)).toEqual([
      'Kill a session kill_session host=local name=dev-x',
      'Start a session new_session host=mercury name=review',
    ]);
    expect(screen.getByTestId('confirm-plan-approve').textContent).toContain('Confirm 2');
  });

  it('Confirm approves every step, oldest first', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await emit('mcp:confirm-required', MOVE);
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-plan-approve'));
    await vi.waitFor(() => expect(get(confirmQueue)).toEqual([]));
    const answers = inv.mock.calls.filter((c) => c[0] === 'mcp_confirm').map((c) => c[1]);
    expect(answers).toEqual([
      { nonce: 'n-op', approved: true },
      { nonce: 'n-move', approved: true },
    ]);
    expect(screen.queryByTestId('confirm-plan')).toBeNull();
  });

  it('Cancel denies every step', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await emit('mcp:confirm-required', MOVE);
    await tick();
    await fireEvent.click(screen.getByTestId('confirm-plan-cancel'));
    await vi.waitFor(() => expect(get(confirmQueue)).toEqual([]));
    const answers = inv.mock.calls.filter((c) => c[0] === 'mcp_confirm').map((c) => c[1]);
    expect(answers.every((a) => a.approved === false)).toBe(true);
  });

  it('the card that arrives takes focus on its first answer, unless the person is typing', async () => {
    await mountBoth();
    await emit('mcp:confirm-required', KILL);
    await tick();
    expect(document.activeElement).toBe(screen.getByTestId('confirm-card-approve'));
  });
});
