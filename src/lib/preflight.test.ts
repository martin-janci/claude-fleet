import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('./moveSession', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./moveSession')>();
  return { ...actual, previewMove: vi.fn() };
});
import { previewMove, type MovePreview } from './moveSession';
import type { Result } from './result';
import {
  preflights,
  requestPreflight,
  preflightFor,
  preflightAge,
  resetPreflightsForTest,
  PREFLIGHT_DEBOUNCE_MS,
  type PreflightEntry,
} from './preflight';

const previewFixture: MovePreview = {
  session_id: 7,
  from_host: 'mefistos',
  to_host: 'turanga',
  branch: 'feat',
  source_cwd: '/r/feat',
  unpushed_commits: 0,
  commits_ahead: 0,
  dirty: [],
  ignored_carried: [],
  ignored_left_behind: [],
  transcript_bytes: 100,
  session_state_files: 0,
  session_state_bytes: 0,
  memory_files: 0,
  memory_bytes: 0,
  target_path: '/r/feat',
  target: { state: 'absent' },
  unknowns: [],
};

beforeEach(() => {
  vi.useFakeTimers();
  resetPreflightsForTest();
  vi.mocked(previewMove).mockReset();
  vi.mocked(previewMove).mockResolvedValue({ ok: true, value: previewFixture });
});

describe('requestPreflight', () => {
  it('fires once for a burst of target changes, for the last target', async () => {
    requestPreflight(7, 'a');
    requestPreflight(7, 'b');
    requestPreflight(7, 'c');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(previewMove).toHaveBeenCalledTimes(1);
    expect(previewMove).toHaveBeenCalledWith(7, 'c');
  });

  it('debounces per session, not globally', async () => {
    requestPreflight(7, 'a');
    requestPreflight(8, 'a');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(previewMove).toHaveBeenCalledTimes(2);
  });

  it('shows loading, then the answer with the time it arrived', async () => {
    let resolve!: (v: Result<MovePreview>) => void;
    vi.mocked(previewMove).mockReturnValueOnce(new Promise((r) => (resolve = r)));
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    expect(preflightFor(get(preflights), 7, 'beta')?.status).toBe('loading');
    resolve({ ok: true, value: previewFixture });
    await Promise.resolve();
    const e = preflightFor(get(preflights), 7, 'beta')!;
    expect(e.status).toBe('ready');
    expect(e.at).not.toBeNull();
  });

  it('records a refusal as refused, keeping the error the move would return', async () => {
    vi.mocked(previewMove).mockResolvedValueOnce({
      ok: false,
      error: { code: 'E_MOVE_MIDOP', message: 'mid-merge', details: null },
    });
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    const e = preflightFor(get(preflights), 7, 'beta')!;
    expect(e.status).toBe('refused');
    expect(e.error?.code).toBe('E_MOVE_MIDOP');
  });

  it('never lets an older answer overwrite a newer one for the same key', async () => {
    let first!: (v: Result<MovePreview>) => void;
    vi.mocked(previewMove)
      .mockReturnValueOnce(new Promise((r) => (first = r)))
      .mockResolvedValueOnce({ ok: true, value: { ...previewFixture, transcript_bytes: 2 } });
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    requestPreflight(7, 'beta');
    await vi.advanceTimersByTimeAsync(PREFLIGHT_DEBOUNCE_MS);
    // The second answer is in; now the first, older one arrives late.
    first({ ok: true, value: { ...previewFixture, transcript_bytes: 1 } });
    await Promise.resolve();
    expect(preflightFor(get(preflights), 7, 'beta')?.preview?.transcript_bytes).toBe(2);
  });

  it('reports age from the moment the answer arrived', () => {
    const e = { at: 1_000 } as PreflightEntry;
    expect(preflightAge(e, 31_000)).toBe(30_000);
    expect(preflightAge({ at: null } as PreflightEntry, 5)).toBeNull();
  });
});
