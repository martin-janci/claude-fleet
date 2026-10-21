// Review w13-9: the desktop lists Send later's waiting prompts and can take
// one back (the phone always could).
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import ScheduledPrompts from './ScheduledPrompts.svelte';
import { scheduledWhen, waitingPrompts } from './send_later';

const settle = async () => {
  for (let i = 0; i < 4; i++) await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
};

describe('ScheduledPrompts', () => {
  const mocked = vi.mocked(invoke);
  let rows: Record<string, unknown>[];

  beforeEach(() => {
    rows = [
      { id: 1, session_id: 7, body: 'run the release checklist', created_at: 1, not_before: 4_000_000_000 },
      { id: 2, session_id: 7, body: 'already went', created_at: 1, delivered_at: 5 },
      { id: 3, session_id: 7, body: 'after the limit', created_at: 1, until_limit_reset: true },
    ];
    mocked.mockReset();
    mocked.mockImplementation(async (cmd, args) => {
      if (cmd === 'queued_prompts') return rows;
      if (cmd === 'cancel_queued_prompt') {
        const id = (args as { args: { id: number } }).args.id;
        rows = rows.map((r) => (r.id === id ? { ...r, cancelled_at: 9 } : r));
        return null;
      }
      return null;
    });
  });

  it('lists what is still waiting, with when it goes, and cancels one', async () => {
    render(ScheduledPrompts, { sessionId: 7 });
    await settle();
    const items = screen.getAllByTestId('conv-scheduled-item');
    expect(items).toHaveLength(2);
    expect(screen.getByTestId('conv-scheduled').textContent).toContain('Scheduled (2)');
    expect(items[0].textContent).toContain('run the release checklist');
    expect(items[1].textContent).toContain('when the usage limit resets');

    await fireEvent.click(screen.getAllByTestId('conv-scheduled-cancel')[0]);
    await settle();
    expect(mocked).toHaveBeenCalledWith('cancel_queued_prompt', { args: { session_id: 7, id: 1 } });
    expect(screen.getAllByTestId('conv-scheduled-item')).toHaveLength(1);
  });

  it('names each cancel button after its prompt', async () => {
    render(ScheduledPrompts, { sessionId: 7 });
    await settle();
    const names = screen.getAllByTestId('conv-scheduled-cancel').map((b) => b.getAttribute('aria-label'));
    expect(names[0]).toContain('run the release checklist');
    expect(names[1]).toContain('after the limit');
    expect(new Set(names).size).toBe(2);
  });

  it("drops the previous session's list at once on a switch, and cancels for the session it was read for", async () => {
    const { rerender } = render(ScheduledPrompts, { sessionId: 7 });
    await settle();
    expect(screen.getAllByTestId('conv-scheduled-item')).toHaveLength(2);
    let answer: (v: unknown) => void = () => {};
    mocked.mockImplementation((cmd) => (cmd === 'queued_prompts' ? new Promise((r) => (answer = r)) : Promise.resolve(null)));
    await rerender({ sessionId: 8 });
    await settle();
    expect(screen.queryByTestId('conv-scheduled')).toBeNull();
    answer([{ id: 40, session_id: 8, body: 'for eight', created_at: 1 }]);
    await settle();
    expect(screen.getAllByTestId('conv-scheduled-item')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('conv-scheduled-cancel'));
    await settle();
    expect(mocked).toHaveBeenCalledWith('cancel_queued_prompt', { args: { session_id: 8, id: 40 } });
  });

  it('a late read for the previous session does not land on the new one', async () => {
    let answer: (v: unknown) => void = () => {};
    mocked.mockImplementation((cmd) => (cmd === 'queued_prompts' ? new Promise((r) => (answer = r)) : Promise.resolve(null)));
    const { rerender } = render(ScheduledPrompts, { sessionId: 7 });
    await settle();
    const first = answer;
    await rerender({ sessionId: 8 });
    await settle();
    first(rows);
    await settle();
    expect(screen.queryByTestId('conv-scheduled')).toBeNull();
  });

  it('a transient failed re-read keeps the list with a note; an older hub clears it', async () => {
    const { rerender } = render(ScheduledPrompts, { sessionId: 7, refreshKey: 0 });
    await settle();
    mocked.mockImplementation(async (cmd) => {
      if (cmd === 'queued_prompts') throw { code: 'E_HUB_TIMEOUT', message: 'slow' };
      return null;
    });
    await rerender({ sessionId: 7, refreshKey: 1 });
    await settle();
    expect(screen.getAllByTestId('conv-scheduled-item')).toHaveLength(2);
    expect(screen.getByTestId('conv-scheduled-stale')).toBeTruthy();
    mocked.mockImplementation(async (cmd) => {
      if (cmd === 'queued_prompts') throw { code: 'E_HUB_PROTOCOL', message: 'unknown command' };
      return null;
    });
    await rerender({ sessionId: 7, refreshKey: 2 });
    await settle();
    expect(screen.queryByTestId('conv-scheduled')).toBeNull();
  });

  it('shows nothing when nothing waits', async () => {
    rows = [];
    render(ScheduledPrompts, { sessionId: 7 });
    await settle();
    expect(screen.queryByTestId('conv-scheduled')).toBeNull();
  });
});

describe('send_later waiting helpers', () => {
  it('keeps only prompts that have not gone, failed, been cancelled or skipped', () => {
    const ids = waitingPrompts([
      { id: 1 },
      { id: 2, delivered_at: 1 },
      { id: 3, failed_at: 1 },
      { id: 4, cancelled_at: 1 },
      { id: 5, skipped_at: 1 },
    ]).map((q) => q.id);
    expect(ids).toEqual([1]);
  });

  it('says when a prompt goes', () => {
    const now = new Date(2026, 9, 10, 14, 20).getTime();
    const at = Math.floor(new Date(2026, 9, 11, 9, 0).getTime() / 1000);
    expect(scheduledWhen({ not_before: at }, now, 'en-US')).toBe('at Sun 09:00');
    expect(scheduledWhen({ until_limit_reset: true }, now)).toBe('when the usage limit resets');
    expect(scheduledWhen({}, now)).toBe('when it is idle');
  });
});
