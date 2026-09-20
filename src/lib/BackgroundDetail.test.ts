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
      entry: entry({ status: 'running', result: null, outputFile: null, history: [] }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-empty').textContent).toContain('has not reported back yet');
    expect(queryByTestId('bg-detail-output')).toBeNull();
  });

  it('still says something when the one report carried no text', () => {
    // A report with a null summary AND a null result satisfied none of the
    // arms, so the body rendered empty — one click after a row that said the
    // task had reported.
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({
        status: 'done',
        result: null,
        outputFile: null,
        history: [{ at: '2026-09-18T10:05:00Z', status: 'completed', summary: null, result: null }],
      }),
      onBack: () => {},
    });
    const body = getByTestId('bg-detail-empty').textContent ?? '';
    expect(body.length).toBeGreaterThan(0);
    expect(body).not.toContain('has not reported back yet');
  });

  it("shows a background command's summary when its report carries no result", () => {
    // Every background `Bash` / `Monitor` is this shape: a `<summary>` with
    // the exit code and no `<result>` at all. Falling through to the empty
    // state told the reader it had not reported, one click after a row that
    // said it had.
    const { getByTestId, queryByTestId } = render(BackgroundDetail, {
      entry: entry({
        kind: 'Bash',
        label: 'Watch Docker build CI',
        result: null,
        history: [
          { at: '2026-09-18T10:05:00Z', status: 'completed', summary: 'Background command finished (exit code 0)', result: null },
        ],
      }),
      onBack: () => {},
    });
    expect(queryByTestId('bg-detail-empty')).toBeNull();
    expect(getByTestId('bg-detail-summary').textContent).toContain('exit code 0');
  });

  it('lists a resumed task every report, summary and result alike', () => {
    const { getAllByTestId } = render(BackgroundDetail, {
      entry: entry({
        history: [
          { at: '2026-09-18T10:05:00Z', status: 'completed', summary: 'first summary', result: 'first pass' },
          { at: '2026-09-18T10:20:00Z', status: 'completed', summary: 'second summary', result: null },
        ],
      }),
      onBack: () => {},
    });
    const rows = getAllByTestId('bg-detail-report');
    expect(rows[0].textContent).toContain('first summary');
    expect(rows[0].textContent).toContain('first pass');
    expect(rows[1].textContent).toContain('second summary');
  });

  it("opens the fleet task's worker session", async () => {
    const onOpenSession = vi.fn();
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({ source: 'fleet_task', sessionId: 4, taskId: 11 }),
      onBack: () => {},
      onOpenSession,
    });
    await fireEvent.click(getByTestId('bg-detail-open-session'));
    expect(onOpenSession).toHaveBeenCalledWith(4);
  });

  it('offers no worker link when there is no worker session', () => {
    const { queryByTestId } = render(BackgroundDetail, {
      entry: entry({ source: 'fleet_task', sessionId: null }),
      onBack: () => {},
      onOpenSession: vi.fn(),
    });
    expect(queryByTestId('bg-detail-open-session')).toBeNull();
  });

  it('shows how long it took, from launch to the newest report', () => {
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({
        at: '2026-09-18T10:00:00Z',
        history: [{ at: '2026-09-18T10:12:00Z', status: 'completed', summary: 's', result: 'r' }],
      }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-duration').textContent).toContain('12m 00s');
  });

  it('invents no duration when the entry never reported', () => {
    const { queryByTestId } = render(BackgroundDetail, {
      entry: entry({ at: '2026-09-18T10:00:00Z', history: [], result: null }),
      onBack: () => {},
    });
    expect(queryByTestId('bg-detail-duration')).toBeNull();
  });

  it('names an idle fleet child idle', () => {
    const { getByTestId } = render(BackgroundDetail, {
      entry: entry({ source: 'fleet_session', status: 'idle', result: null, history: [] }),
      onBack: () => {},
    });
    expect(getByTestId('bg-detail-status').textContent).toContain('idle');
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
