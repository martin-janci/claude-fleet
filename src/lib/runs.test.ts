import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { listRuns, outcomeLabel } from './runs';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

describe('listRuns', () => {
  beforeEach(() => invoke.mockReset());

  it('sends only the filters that are set, under args', async () => {
    invoke.mockResolvedValue({ runs: [], total: 0 });
    await listRuns({ since: 10, kind: 'jev', session_id: undefined, limit: 20 });
    expect(invoke).toHaveBeenCalledWith('list_runs', {
      args: { since: 10, kind: 'jev', limit: 20 },
    });
  });

  it('gives a null-stripped row its empty session list back', async () => {
    invoke.mockResolvedValue({
      runs: [{ id: 'jev:1', source: 'jev', kind: 'jev', owner: 'x', started_at: 1, outcome: 'ok' }],
      total: 1,
    });
    const r = await listRuns();
    expect(r.ok && r.value.runs[0].session_ids).toEqual([]);
    expect(r.ok && r.value.total).toBe(1);
  });

  it('passes a refusal through', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_INVALID', message: 'bad kind' });
    const r = await listRuns({ outcome: 'ok' });
    expect(r.ok).toBe(false);
  });
});

describe('outcomeLabel', () => {
  it('says each outcome in plain words', () => {
    expect(outcomeLabel({ outcome: 'ok' })).toBe('OK');
    expect(outcomeLabel({ outcome: 'needs_person' })).toBe('Needs person');
    expect(outcomeLabel({ outcome: 'nothing_to_do' })).toBe('Nothing to do');
    expect(outcomeLabel({ outcome: 'running' })).toBe('Running');
  });

  it('carries a failure its error', () => {
    expect(outcomeLabel({ outcome: 'failed', error: 'timeout' })).toBe('Failed: timeout');
    expect(outcomeLabel({ outcome: 'failed' })).toBe('Failed');
  });

  it('shows an outcome it does not know as sent', () => {
    expect(outcomeLabel({ outcome: 'later' })).toBe('later');
  });
});
