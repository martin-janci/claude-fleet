import { describe, it, expect } from 'vitest';
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
    render(SubagentBlock, { item: item(), nowMs: 0 });
    const head = screen.getByTestId('conv-subagent-head');
    expect(head.textContent).toContain('Explore');
    expect(head.textContent).toContain('Map the store');
    expect(head.textContent).toContain('2m 00s');
  });

  it('running shows running…', () => {
    render(SubagentBlock, { item: item({ done: false, ended_at: null, result: null, agent_type: null }), nowMs: 0 });
    const head = screen.getByTestId('conv-subagent-head');
    expect(head.textContent).toContain('subagent');
    expect(head.textContent).toContain('running…');
  });

  it('error is marked', () => {
    render(SubagentBlock, { item: item({ error: true }), nowMs: 0 });
    const block = screen.getByTestId('conv-subagent');
    expect(block.getAttribute('data-error')).toBe('true');
    expect(block.querySelector('.sub-err')).toBeTruthy();
  });

  it('the result renders as markdown, clamped with Show more', async () => {
    const long = ['Found **three** modules.', ...Array.from({ length: 10 }, (_, i) => `- item ${i}`)].join('\n');
    render(SubagentBlock, { item: item({ result: long }), nowMs: 0 });
    const body = screen.getByTestId('conv-subagent-result');
    expect(body.querySelector('strong')?.textContent).toBe('three');
    expect(body.classList.contains('clamped')).toBe(true);
    await fireEvent.click(screen.getByRole('button', { name: 'Show more' }));
    expect(screen.getByTestId('conv-subagent-result').classList.contains('clamped')).toBe(false);
    expect(screen.getByRole('button', { name: 'Show less' })).toBeTruthy();
  });
});
