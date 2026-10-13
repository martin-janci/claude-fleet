import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';

vi.mock('./conversation', async () => {
  const actual = await vi.importActual<typeof import('./conversation')>('./conversation');
  return { ...actual, toolDetail: vi.fn() };
});
import { toolDetail, type ToolLine as Line, type ToolDetail } from './conversation';
import ToolLine from './ToolLine.svelte';
import { expectAccessible } from './a11y_check';

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

describe('ToolLine state mark (redesign 5.14, New layout)', () => {
  const mark = () => screen.getByTestId('conv-tool-state');

  it('only the running call carries a Comet; finished calls go still', () => {
    const { unmount } = render(ToolLine, { line: line({ done: false, ended_at: null }), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: true });
    expect(mark().getAttribute('data-state')).toBe('running');
    expect(mark().querySelector('[data-testid^="loader"]')).toBeTruthy();
    unmount();
    const done = render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: true });
    expect(mark().getAttribute('data-state')).toBe('done');
    expect(mark().querySelector('[data-testid^="loader"]')).toBeNull();
    expect(mark().textContent).toContain('✓');
    done.unmount();
    // An unfinished call outside the running turn never got its result:
    // still, not spinning.
    render(ToolLine, { line: line({ done: false, ended_at: null }), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: false });
    expect(mark().getAttribute('data-state')).toBe('still');
    expect(mark().querySelector('[data-testid^="loader"]')).toBeNull();
  });

  it('is accessible', async () => {
    mockedDetail.mockResolvedValue({ ok: true, value: detail() });
    const { container } = render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: 'c1', nowMs: 0, live: true });
    await expectAccessible(container);
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    await expectAccessible(container);
  });
});

describe('ToolLine: a debug_devices call', () => {
  const DEVICE = 'mcp__claude-fleet__debug_devices';
  const pixel = {
    id: 3,
    title: 'bench pixel',
    host: 'mac',
    platform: 'android',
    kind: 'physical',
    key: 'usb:R5CT',
    name: 'Pixel 9',
    os_version: '16',
    state: 'online',
    ready: true,
    shared: false,
    claimed_by: 'host:mercury#42',
    claimed_until: 1_000 + 12 * 60,
    claim_note: 'login flow',
    waiting: 1,
    first_seen_at: 1,
    last_seen_at: 2,
  };
  const deviceLine = line({ name: DEVICE, target: null, summary: DEVICE });

  it('leads with Device and shows the device it answered, not raw JSON', async () => {
    mockedDetail.mockResolvedValue({
      ok: true,
      value: detail({ name: DEVICE, command: null, input: '{\n  "action": "claim",\n  "device": "bench pixel"\n}', result: JSON.stringify(pixel) }),
    });
    render(ToolLine, { line: deviceLine, sessionId: 1, claudeSessionId: null, nowMs: 1_000_000, live: false });
    expect(screen.getByTestId('conv-tool').textContent).toContain('Device');
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool').textContent).toContain('claim · bench pixel');
    const card = screen.getByTestId('conv-device-result');
    expect(card.textContent).toContain('bench pixel');
    expect(card.textContent).toContain('on mac');
    expect(screen.getByTestId('device-state').textContent).toBe('Online');
    expect(screen.getByTestId('device-holder').textContent).toContain('session #42 on mercury');
    expect(screen.getByTestId('device-holder').textContent).toBe('Held by session #42 on mercury · 12 min left · “login flow”');
    expect(screen.getByTestId('device-queue').textContent).toContain('1 waiting');
    // The raw JSON is only behind the Input disclosure.
    expect(screen.queryByTestId('conv-tool-result')).toBeNull();
    expect(screen.getByText('Input').closest('details')?.open).toBe(false);
  });

  it('a list result is a compact list, a run result its output', async () => {
    mockedDetail.mockResolvedValueOnce({
      ok: true,
      value: detail({ name: DEVICE, command: null, input: '{"action":"list"}', result: JSON.stringify({ devices: [pixel, { ...pixel, id: 4, title: 'iPhone 16' }] }) }),
    });
    const { unmount } = render(ToolLine, { line: deviceLine, sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-device-list').querySelectorAll('li')).toHaveLength(2);
    unmount();

    mockedDetail.mockResolvedValueOnce({
      ok: true,
      value: detail({ name: DEVICE, command: null, input: '{"action":"run","device":"pixel","args":["shell","ls"]}', result: '{"exit_code":0,"output":"sdcard","truncated":true}' }),
    });
    render(ToolLine, { line: deviceLine, sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-device-output').textContent).toBe('sdcard');
    expect(screen.getByTestId('conv-tool-detail').textContent).toContain('output truncated');
  });

  it('draws a screenshot result as an image that opens full size', async () => {
    mockedDetail.mockResolvedValue({
      ok: true,
      value: detail({
        name: DEVICE,
        command: null,
        input: '{"action":"screenshot","device":"pixel"}',
        result: 'bench pixel on mac: 3 bytes',
        images: [{ mime: 'image/png', data: 'AAEC' }],
        images_omitted: 2,
      }),
    });
    render(ToolLine, { line: deviceLine, sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool-result').textContent).toContain('3 bytes');
    const img = screen.getByTestId('conv-tool-image').querySelector('img')!;
    expect(img.getAttribute('src')).toBe('data:image/png;base64,AAEC');
    expect(screen.getByTestId('conv-tool-images-omitted').textContent).toContain('2 more images not shown');
    await fireEvent.click(screen.getByTestId('conv-tool-image'));
    const full = screen.getByTestId('conv-tool-image-full');
    expect(full.getAttribute('role')).toBe('dialog');
    await fireEvent.keyDown(full, { key: 'Escape' });
    expect(screen.queryByTestId('conv-tool-image-full')).toBeNull();
  });

  it('any tool’s images are drawn, and a non-image type is not', async () => {
    mockedDetail.mockResolvedValue({
      ok: true,
      value: detail({ images: [{ mime: 'image/jpeg', data: '/9j/' }, { mime: 'text/html', data: 'PGI+' }] }),
    });
    render(ToolLine, { line: line(), sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    const imgs = screen.getAllByTestId('conv-tool-image');
    expect(imgs).toHaveLength(1);
    expect(imgs[0].querySelector('img')?.getAttribute('src')).toBe('data:image/jpeg;base64,/9j/');
  });

  it('an error result keeps the error styling', async () => {
    mockedDetail.mockResolvedValue({
      ok: true,
      value: detail({ name: DEVICE, command: null, input: '{"action":"claim","device":"pixel"}', result: 'E_CONFLICT: held by host:mac#7', is_error: true }),
    });
    render(ToolLine, { line: { ...deviceLine, error: true }, sessionId: 1, claudeSessionId: null, nowMs: 0, live: false });
    await fireEvent.click(screen.getByTestId('conv-tool'));
    await settle();
    expect(screen.getByTestId('conv-tool-result').getAttribute('data-error')).toBe('true');
    expect(screen.queryByTestId('conv-device-result')).toBeNull();
  });
});
