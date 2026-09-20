import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import ConversationHeader from './ConversationHeader.svelte';
import type { SessionRow } from './sessions';
import type { ConversationSummary } from './conversation';

const session = (o: Partial<SessionRow> = {}) =>
  ({ id: 1, claude_session_id: 'bbb', claude_status: 'idle', current_activity: null, model: 'claude-opus-5',
     context_pct: 21, context_tokens: 42_000, context_window: 200_000, context_stale: false, ...o }) as SessionRow;
const conv = (o: Partial<ConversationSummary>): ConversationSummary => ({
  id: 1, session_id: 1, claude_session_id: 'aaa', transcript_path: null, started_at: 1_789_000_000,
  ended_at: 1_789_000_500, start_source: 'fleet', end_reason: 'clear', model: null, first_prompt: 'fix the bug',
  turns: 3, compactions: 0, current: false, ...o,
});
const list = [conv({ id: 2, claude_session_id: 'bbb', start_source: 'clear', current: true, ended_at: null, turns: 1, first_prompt: 'next' }), conv({})];

describe('ConversationHeader', () => {
  it('shows the current conversation, context, model and status', () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: '/compact 3m ago', newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-switcher').textContent).toContain('Current · /clear · 1 turn');
    expect(screen.getByTestId('conv-ctx').textContent).toContain('42k / 200k · 21%');
    expect(screen.getByTestId('conv-model').textContent).toContain('opus-5');
    expect(screen.getByTestId('conv-status').textContent).toContain('idle');
    expect(screen.getByTestId('conv-last-event').textContent).toContain('/compact 3m ago');
  });

  it('lists earlier conversations and selects one', async () => {
    const onSelect = vi.fn();
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    const items = screen.getAllByTestId('conv-switcher-item');
    expect(items).toHaveLength(2);
    expect(items[1].textContent).toContain('fix the bug');
    await fireEvent.click(items[1]);
    expect(onSelect).toHaveBeenCalledWith('aaa');
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });

  it('selecting the current entry passes null; Escape closes the menu', async () => {
    const onSelect = vi.fn();
    render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: true, onSelect });
    expect(screen.getByTestId('conv-switcher-dot')).toBeTruthy();
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.click(screen.getAllByTestId('conv-switcher-item')[0]);
    expect(onSelect).toHaveBeenCalledWith(null);
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'Escape' });
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });

  it('hides the meter while viewing an earlier conversation and dims a stale one', () => {
    const { unmount } = render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.queryByTestId('conv-ctx')).toBeNull();
    unmount();
    render(ConversationHeader, { session: session({ context_stale: true }), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-ctx').getAttribute('data-stale')).toBe('true');
  });

  it('shows 0 after a clear', () => {
    render(ConversationHeader, { session: session({ context_pct: 0, context_tokens: 0 }), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-ctx').textContent).toContain('0 / 200k · 0%');
  });

  it('works with an empty conversation list (phase-1 hub not yet reached)', () => {
    render(ConversationHeader, { session: session(), conversations: [], viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-switcher').textContent).toContain('Current');
  });

  it('shows the viewed conversation\'s model while viewing an earlier one', () => {
    const l = [list[0], conv({ model: 'claude-sonnet-4' })];
    render(ConversationHeader, { session: session(), conversations: l, viewing: 'aaa', lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-model').textContent).toBe('sonnet-4');
  });

  it('highlights and aria-selects the viewed entry, the current one otherwise', async () => {
    const { unmount } = render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    let items = screen.getAllByTestId('conv-switcher-item');
    expect(items.map((i) => i.getAttribute('aria-selected'))).toEqual(['false', 'true']);
    expect(items.map((i) => i.classList.contains('selected'))).toEqual([false, true]);
    unmount();
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    items = screen.getAllByTestId('conv-switcher-item');
    expect(items.map((i) => i.getAttribute('aria-selected'))).toEqual(['true', 'false']);
    expect(items.map((i) => i.classList.contains('selected'))).toEqual([true, false]);
  });

  it('gives the newer dot an accessible label', () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: true, onSelect: vi.fn() });
    expect(screen.getByTestId('conv-switcher-dot').getAttribute('aria-label')).toBe('newer conversation available');
    expect(screen.getByRole('button', { name: /newer conversation available/ })).toBeTruthy();
  });

  it('Escape returns focus to the switcher button', async () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'Escape' });
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('conv-switcher'));
  });

  it('Space selects an option like Enter', async () => {
    const onSelect = vi.fn();
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.keyDown(screen.getAllByTestId('conv-switcher-item')[1], { key: ' ' });
    expect(onSelect).toHaveBeenCalledWith('aaa');
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });

  it('opens on the viewed conversation and walks the list with the arrow keys', async () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: 'aaa', lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    const items = screen.getAllByTestId('conv-switcher-item');
    // Focus starts on the entry being viewed, not blindly on the first row.
    expect(document.activeElement).toBe(items[1]);
    expect(items[1].getAttribute('tabindex')).toBe('0');
    expect(items[0].getAttribute('tabindex')).toBe('-1');

    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[0]);
    // The ends do not wrap past themselves.
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'ArrowUp' });
    expect(document.activeElement).toBe(items[0]);
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'End' });
    expect(document.activeElement).toBe(items[1]);
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'Home' });
    expect(document.activeElement).toBe(items[0]);
    await fireEvent.keyDown(screen.getByTestId('conv-switcher-menu'), { key: 'ArrowDown' });
    expect(document.activeElement).toBe(items[1]);
  });

  it('an outside pointerdown closes the switcher, an inside one does not', async () => {
    render(ConversationHeader, { session: session(), conversations: list, viewing: null, lastEvent: null, newerAvailable: false, onSelect: vi.fn() });
    await fireEvent.click(screen.getByTestId('conv-switcher'));
    await fireEvent.pointerDown(screen.getAllByTestId('conv-switcher-item')[0]);
    expect(screen.getByTestId('conv-switcher-menu')).toBeTruthy();
    await fireEvent.pointerDown(document.body);
    expect(screen.queryByTestId('conv-switcher-menu')).toBeNull();
  });
});
