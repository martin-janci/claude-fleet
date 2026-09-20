import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import SubagentBlock from './SubagentBlock.svelte';
import type { ConvGroup } from './conversation';

type Sub = Extract<ConvGroup, { kind: 'subagent' }>;

const item = (o: Partial<Sub> = {}): Sub => ({
  kind: 'subagent',
  id: 'toolu_9',
  name: 'Task',
  agent_type: 'Explore',
  description: 'Map the store',
  result: 'Found **three** modules.',
  error: false,
  at: '2026-09-18T09:00:00Z',
  ended_at: '2026-09-18T09:02:00Z',
  done: true,
  ...o,
});

describe('SubagentBlock', () => {
  it('shows type, description and duration', () => {
    render(SubagentBlock, { item: item(), nowMs: 0, live: false });
    const head = screen.getByTestId('conv-subagent-head');
    expect(head.textContent).toContain('Explore');
    expect(head.textContent).toContain('Map the store');
    expect(head.textContent).toContain('2m 00s');
  });

  it('a running subagent in the live turn shows how long it has been running', () => {
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null, agent_type: null }),
      nowMs: Date.parse('2026-09-18T09:01:05Z'),
      live: true,
    });
    const head = screen.getByTestId('conv-subagent-head');
    expect(head.textContent).toContain('subagent');
    expect(head.textContent).toContain('running 1m 05s');
  });

  it('an unfinished subagent outside the live turn shows "no result"', () => {
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null }),
      nowMs: Date.parse('2026-09-18T09:01:05Z'),
      live: false,
    });
    const head = screen.getByTestId('conv-subagent-head');
    expect(head.textContent).toContain('no result');
    expect(head.textContent).not.toContain('running');
    // It is neither running (nothing is driving it) nor done, so the header
    // names no status rather than guessing one.
    expect(screen.queryByTestId('conv-subagent-status')).toBeNull();
  });

  it('error is marked', () => {
    render(SubagentBlock, { item: item({ error: true }), nowMs: 0, live: false });
    const block = screen.getByTestId('conv-subagent');
    expect(block.getAttribute('data-error')).toBe('true');
    expect(block.querySelector('.sub-err')).toBeTruthy();
  });

  it('shows a status word in the header', () => {
    const { unmount } = render(SubagentBlock, { item: item(), nowMs: 0, live: false });
    expect(screen.getByTestId('conv-subagent-status').textContent).toBe('done');
    unmount();
    render(SubagentBlock, { item: item({ error: true }), nowMs: 0, live: false });
    expect(screen.getByTestId('conv-subagent-status').textContent).toBe('failed');
  });

  it('a subagent that has not reported reads as running', () => {
    render(SubagentBlock, { item: item({ done: false, ended_at: null, result: null }), nowMs: 0, live: true });
    expect(screen.getByTestId('conv-subagent-status').textContent).toBe('running');
  });

  it('an unfinished background agent reads as running outside the live turn', () => {
    // The switcher lists this same call as `running`. `onOpen` is passed
    // only when it does, so its presence is what lets the block agree
    // instead of falling back to a wordless "no result".
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null }),
      nowMs: 0,
      live: false,
      onOpen: () => {},
    });
    expect(screen.getByTestId('conv-subagent-status').textContent).toBe('running');
  });

  it('an unfinished foreground call outside the live turn still gets no word', () => {
    render(SubagentBlock, {
      item: item({ done: false, ended_at: null, result: null }),
      nowMs: 0,
      live: false,
    });
    expect(screen.queryByTestId('conv-subagent-status')).toBeNull();
  });

  it('offers an open affordance only when one is wired, and it is a real button', async () => {
    const { unmount } = render(SubagentBlock, { item: item(), nowMs: 0, live: false });
    expect(screen.queryByTestId('conv-subagent-open')).toBeNull();
    unmount();
    const onOpen = vi.fn();
    render(SubagentBlock, { item: item(), nowMs: 0, live: false, onOpen });
    const btn = screen.getByTestId('conv-subagent-open');
    expect(btn.tagName).toBe('BUTTON');
    await fireEvent.click(btn);
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it('the result renders as markdown, clamped with Show more', async () => {
    const long = ['Found **three** modules.', ...Array.from({ length: 10 }, (_, i) => `- item ${i}`)].join('\n');
    render(SubagentBlock, { item: item({ result: long }), nowMs: 0, live: false });
    const body = screen.getByTestId('conv-subagent-result');
    expect(body.querySelector('strong')?.textContent).toBe('three');
    expect(body.classList.contains('clamped')).toBe(true);
    await fireEvent.click(screen.getByRole('button', { name: 'Show more' }));
    expect(screen.getByTestId('conv-subagent-result').classList.contains('clamped')).toBe(false);
    expect(screen.getByRole('button', { name: 'Show less' })).toBeTruthy();
  });
});
