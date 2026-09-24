// work.ts loaders (roadmap M2.5): past work merged per key, the resume
// plan's older-hub detection, and the argument shapes of the commands.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  loadPastWork,
  pastWork,
  pastWorkSummary,
  workPurgeImpact,
  workResumePlan,
  RESUME_UNSUPPORTED,
  type WorkLink,
} from './work';

const link = (id: number, key: string, ended: number | null, state = 'confirmed'): WorkLink => ({
  id,
  ref_key: key,
  state,
  source: 'manual',
  created_at: 1,
  ended_at: ended,
});

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  pastWork.set(new Map());
});

describe('loadPastWork', () => {
  it('merges the recent read with each live key, deduped and newest first', async () => {
    vi.mocked(invoke).mockImplementation(async (_cmd: string, a?: unknown) => {
      const key = (a as { args: { key?: string } }).args.key;
      if (key === undefined) return [link(1, 'ABC-1', 100), link(2, 'DEF-2', 300)];
      if (key === 'ABC-1') return [link(1, 'ABC-1', 100), link(3, 'ABC-1', 200), link(4, 'ABC-1', null)];
      return [];
    });
    const out = await loadPastWork(['ABC-1']);
    expect(out.get('ABC-1')!.map((l) => l.id)).toEqual([3, 1]);
    expect(out.get('DEF-2')!.map((l) => l.id)).toEqual([2]);
    expect(get(pastWork)).toBe(out);
  });

  it('an older hub refusing the empty read leaves only the per-key past', async () => {
    vi.mocked(invoke).mockImplementation(async (_cmd: string, a?: unknown) => {
      const key = (a as { args: { key?: string } }).args.key;
      if (key === undefined) throw { code: 'E_INVALID', message: 'pass exactly one' };
      return [link(7, 'ABC-1', 50)];
    });
    const out = await loadPastWork(['ABC-1']);
    expect([...out.keys()]).toEqual(['ABC-1']);
  });
});

describe('resume commands', () => {
  it('a plan without modes (an older hub) is reported as unsupported', async () => {
    vi.mocked(invoke).mockResolvedValue([link(1, 'ABC-1', 1)]);
    const r = await workResumePlan('ABC-1');
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.message).toBe(RESUME_UNSUPPORTED);
  });

  it('sends the plan and purge arguments the commands expect', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ key: 'ABC-1', modes: [] });
    await workResumePlan('ABC-1', { linkId: 4, withBrief: true });
    expect(vi.mocked(invoke)).toHaveBeenLastCalledWith('work_resume_plan', {
      args: { key: 'ABC-1', link_id: 4, host_alias: null, with_brief: true },
    });
    vi.mocked(invoke).mockResolvedValueOnce({ keys: ['ABC-1'] });
    const r = await workPurgeImpact(3, ['h']);
    expect(vi.mocked(invoke)).toHaveBeenLastCalledWith('work_purge_impact', {
      args: { project_id: 3, host_aliases: ['h'] },
    });
    expect(r).toEqual({ ok: true, value: ['ABC-1'] });
  });

  it('summarises past work', () => {
    const now = 10 * 86400 * 1000;
    expect(pastWorkSummary([link(1, 'A-1', 8 * 86400), link(2, 'A-1', 5 * 86400)], now)).toBe(
      '2 sessions, last 2d ago',
    );
    expect(pastWorkSummary([link(1, 'A-1', null)], now)).toBe('1 session');
  });
});
