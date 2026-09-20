import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, toolDetail: vi.fn() };
});
import { toolDetail, type ToolLine as Line, type ToolDetail } from './conversation';
import ToolLine from './ToolLine.svelte';

vi.mock('./clipboard', async () => {
  const actual = await vi.importActual<typeof import('./clipboard')>('./clipboard');
  return { ...actual, copyText: vi.fn() };
});
import { copyText } from './clipboard';
const mockedCopy = copyText as unknown as ReturnType<typeof vi.fn>;

const mockedDetail = toolDetail as unknown as ReturnType<typeof vi.fn>;

const line = (o: Partial<Line> = {}): Line => ({
  summary: 'Bash(cargo test)',
  error: false,
  id: 'toolu_1',
  name: 'Bash',
  target: 'cargo test',
  at: '2026-09-18T09:00:00Z',
  ended_at: '2026-09-18T09:00:12Z',
  done: true,
  ...o,
});
const detail = (o: Partial<ToolDetail> = {}): ToolDetail => ({
  id: 'toolu_1',
  name: 'Bash',
  input: '{\n  "command": "cargo test"\n}',
  edit: null,
  command: 'cargo test',
  result: 'test result: ok',
  is_error: false,
  ...o,
});

async function settle() {
  await tick();
  await Promise.resolve();
  await tick();
}

beforeEach(() => {
  mockedDetail.mockReset();
  mockedCopy.mockReset();
});

describe('ToolLine', () => {
  it('a finished call shows verb, short target and duration', () => {
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: false });
    const row = screen.getByTestId('conv-tool');
    expect(row.textContent).toContain('Run');
    expect(row.textContent).toContain('cargo test');
    expect(row.textContent).toContain('12s');
    expect(row.getAttribute('aria-expanded')).toBe('false');
    expect(row.querySelector('[title="Bash(cargo test)"]')).toBeTruthy();
  });

  it('a failed call is marked', () => {
    render(ToolLine, { line: line({ error: true }), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    const row = screen.getByTestId('conv-tool');
    expect(row.getAttribute('data-error')).toBe('true');
    expect(row.textContent).toContain('✕');
  });

  it('expanding fetches the detail once and renders an edit diff', async () => {
    mockedDetail.mockResolvedValue({
      ok: true,
      value: detail({ name: 'Edit', command: null, edit: { file_path: '/r/src/a.rs', old: 'keep\nold line', new: 'keep\nnew line' } }),
    });
    render(ToolLine, {
      line: line({ name: 'Edit', target: '/r/src/a.rs', summary: 'Edit(/r/src/a.rs)' }),
      sessionId: 7,
      claudeSessionId: 'c1',
      nowMs: 0,
      live: false,
    });
    const row = screen.getByTestId('conv-tool');
    await fireEvent.click(row);
    await settle();
    expect(row.getAttribute('aria-expanded')).toBe('true');
    const body = screen.getByTestId('conv-tool-detail');
    expect(body.textContent).toContain('/r/src/a.rs');
    expect(body.querySelector('.del')?.textContent).toContain('old line');
    expect(body.querySelector('.add')?.textContent).toContain('new line');
    await fireEvent.click(row); // collapse
    await fireEvent.click(row); // expand again: served from the cache
    await settle();
    expect(mockedDetail).toHaveBeenCalledTimes(1);
    expect(mockedDetail).toHaveBeenCalledWith(7, 'toolu_1', 'c1');
    expect(screen.getByTestId('conv-tool-detail')).toBeTruthy();
  });

  it('a bash detail shows the command and the result', async () => {
    mockedDetail.mockResolvedValue({ ok: true, value: detail() });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(mockedDetail).toHaveBeenCalledWith(1, 'toolu_1', undefined);
    expect(screen.getByTestId('conv-tool-detail').querySelector('pre.cmd')?.textContent).toBe('$ cargo test');
    expect(screen.getByTestId('conv-tool-result').textContent).toContain('test result: ok');
  });

  it('a fetch error shows retry, and retry refetches', async () => {
    mockedDetail.mockResolvedValueOnce({ ok: false, error: { code: 'E_NOT_FOUND', message: 'tool call not found' } });
    mockedDetail.mockResolvedValueOnce({ ok: true, value: detail() });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    const errBox = screen.getByTestId('conv-tool-detail-error');
    expect(errBox.textContent).toContain('tool call not found');
    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await settle();
    expect(mockedDetail).toHaveBeenCalledTimes(2);
    expect(screen.queryByTestId('conv-tool-detail-error')).toBeNull();
    expect(screen.getByTestId('conv-tool-result').textContent).toContain('test result: ok');
  });

  it('a hub-connected refusal shows the reason without a Retry', async () => {
    mockedDetail.mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_LOCAL_ONLY', message: 'session_tool_detail is not available through the hub' },
    });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool-detail-error').textContent).toContain('not available through the hub');
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  // Same reasoning as the refusal above: the hub's wire contract is outside
  // this build's range, so the read cannot succeed until someone upgrades a
  // binary. A Retry button there is a promise the app cannot keep.
  it('a contract-skewed hub shows the reason without a Retry', async () => {
    mockedDetail.mockResolvedValueOnce({
      ok: false,
      error: {
        code: 'E_HUB_CONTRACT',
        message:
          'session_tool_detail was not run: the hub’s wire contract is revision 1, ' +
          'older than the 3 this app requires. Update the hub.',
      },
    });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool-detail-error').textContent).toContain('Update the hub.');
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull();
  });

  it('a call without an id is not expandable', () => {
    render(ToolLine, { line: line({ id: null }), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    expect(screen.queryByRole('button')).toBeNull();
    const row = screen.getByTestId('conv-tool');
    expect(row.tagName).toBe('DIV');
    expect(row.textContent).toContain('Run');
  });

  it('a running call shows how long it has been running', () => {
    render(ToolLine, {
      line: line({ done: false, ended_at: null }),
      sessionId: 1,
      claudeSessionId: null,
      nowMs: Date.parse('2026-09-18T09:00:05Z'),
      live: true,
    });
    expect(screen.getByTestId('conv-tool').textContent).toContain('running 5s');
  });

  it('an unfinished call outside the live turn shows "no result", not a running timer', () => {
    render(ToolLine, {
      line: line({ done: false, ended_at: null }),
      sessionId: 1,
      claudeSessionId: null,
      nowMs: Date.parse('2026-09-18T09:00:05Z'),
      live: false,
    });
    const row = screen.getByTestId('conv-tool');
    expect(row.textContent).toContain('no result');
    expect(row.textContent).not.toContain('running');
    expect(row.querySelector('.dur.muted')).toBeTruthy();
  });

  it('the result has a copy button that copies the full result and flashes Copied', async () => {
    mockedCopy.mockResolvedValue(true);
    const long = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');
    mockedDetail.mockResolvedValue({ ok: true, value: detail({ result: long }) });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    const btn = screen.getByTestId('conv-copy');
    expect(btn.getAttribute('aria-label')).toBe('Copy result');
    await fireEvent.click(btn);
    await settle();
    expect(mockedCopy).toHaveBeenCalledWith(long);
    expect(screen.getByTestId('conv-copy').getAttribute('aria-label')).toBe('Copied');
    expect(screen.getByTestId('conv-copy').getAttribute('title')).toBe('Copied');
  });

  it('copying a result capped by the backend says it was truncated', async () => {
    mockedCopy.mockResolvedValue(true);
    const capped = 'ž'.repeat(8_000) + '…';
    mockedDetail.mockResolvedValue({ ok: true, value: detail({ result: capped }) });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    await fireEvent.click(screen.getByTestId('conv-copy'));
    await settle();
    expect(mockedCopy).toHaveBeenCalledWith(capped);
    expect(screen.getByTestId('conv-copy').getAttribute('title')).toBe('Copied (truncated at 8 000 chars)');
  });

  it('a row moved to another call drops the in-flight fetch and starts collapsed', async () => {
    let resolve!: (v: unknown) => void;
    mockedDetail.mockReturnValueOnce(new Promise((r) => (resolve = r)));
    const { rerender } = render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByText('Loading…')).toBeTruthy();
    await rerender({ line: line({ id: 'toolu_2' }), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: false });
    await settle();
    expect(screen.getByTestId('conv-tool').getAttribute('aria-expanded')).toBe('false');
    resolve({ ok: true, value: detail({ result: 'stale result' }) });
    await settle();
    expect(screen.queryByTestId('conv-tool-detail')).toBeNull();
    // Expanding again reads the new call, not a cached old one.
    mockedDetail.mockResolvedValueOnce({ ok: true, value: detail({ id: 'toolu_2', result: 'fresh result' }) });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(mockedDetail).toHaveBeenLastCalledWith(1, 'toolu_2', 'c1');
    expect(screen.getByTestId('conv-tool-result').textContent).toBe('fresh result');
  });

  it('a long result is clamped to 20 lines with Show all', async () => {
    const long = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');
    mockedDetail.mockResolvedValue({ ok: true, value: detail({ result: long, is_error: true }) });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    const result = screen.getByTestId('conv-tool-result');
    expect(result.getAttribute('data-error')).toBe('true');
    expect(result.textContent).toContain('line 19');
    expect(result.textContent).not.toContain('line 20');
    await fireEvent.click(screen.getByRole('button', { name: 'Show all' }));
    expect(screen.getByTestId('conv-tool-result').textContent).toContain('line 29');
  });
});
