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
});
