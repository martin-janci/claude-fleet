import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import BackgroundDetail from './BackgroundDetail.svelte';
import type { BackgroundEntry } from './conversation';

const entry = (over: Partial<BackgroundEntry> = {}): BackgroundEntry => ({
  key: 'task:a6',
  source: 'transcript',
  kind: 'Agent',
  label: 'Posúdiť stratégiu testov',
  status: 'done',
  at: '2026-09-18T10:00:01Z',
  result: '# Posudok\n\nVšetko overené.',
  error: null,
  outputFile: '/private/tmp/x/tasks/a6.output',
  sessionId: null,
  taskId: null,
  history: [],
  ...over,
});

describe('BackgroundDetail', () => {
  it('shows what the agent was asked to do and what it reported', () => {
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack: () => {} });
    expect(getByTestId('bg-detail-label').textContent).toContain('Posúdiť stratégiu testov');
    expect(getByTestId('bg-detail-kind').textContent).toContain('Agent');
    expect(getByTestId('bg-detail-result').textContent).toContain('Všetko overené.');
  });

  it('offers the output file path to copy', () => {
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack: () => {} });
    expect(getByTestId('bg-detail-output').textContent).toContain('/private/tmp/x/tasks/a6.output');
  });

  it('says so plainly when nothing has been reported yet', () => {
    const { getByTestId, queryByTestId } = render(BackgroundDetail, {
      entry: entry({ status: 'running', result: null, outputFile: null }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-empty').textContent).toContain('has not reported back yet');
    expect(queryByTestId('bg-detail-output')).toBeNull();
  });

  it('shows a failure reason instead of a report', () => {
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({ source: 'fleet_task', status: 'failed', result: null, error: 'worker died' }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-error').textContent).toContain('worker died');
  });

  it('lists every report when an agent was resumed', () => {
    const { getAllByTestId } = render(BackgroundDetail, {
      entry: entry({
        history: [
          { at: '2026-09-18T10:05:00Z', status: 'completed', summary: 's', result: 'first pass' },
          { at: '2026-09-18T10:20:00Z', status: 'completed', summary: 's', result: 'second pass' },
        ],
      }),
      onBack: () => {},
    });
    expect(getAllByTestId('bg-detail-report')).toHaveLength(2);
  });

  it('does not list a single report twice', () => {
    const { queryAllByTestId } = render(BackgroundDetail, {
      entry: entry({
        history: [{ at: '2026-09-18T10:05:00Z', status: 'completed', summary: 's', result: 'only' }],
      }),
      onBack: () => {},
    });
    expect(queryAllByTestId('bg-detail-report')).toHaveLength(0);
  });

  it('goes back', async () => {
    const onBack = vi.fn();
    const { getByTestId } = render(BackgroundDetail, { entry: entry(), onBack });
    await fireEvent.click(getByTestId('bg-detail-back'));
    expect(onBack).toHaveBeenCalledOnce();
  });
});
