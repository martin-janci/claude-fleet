// The link decisions' Work view guards (work.ts, work graph M14.1d) and the
// `work:changed` parsing (work_view.ts): each command's argument object, and
// what a frame may carry.
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./sessions', async (orig) => ({ ...(await orig<typeof import('./sessions')>()), acceptCommandRow: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { parseWorkChanged } from './work_view';
import { confirmSessionWork, linkSessionWork, rejectWorkLink, unlinkSessionWork } from './work';

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue({});
});

const lastCall = () => vi.mocked(invoke).mock.calls.at(-1);

describe('link decisions', () => {
  it('link / confirm / reject / unlink carry primary and expected_version only when given', async () => {
    vi.mocked(invoke).mockResolvedValue({ id: 7 });
    await linkSessionWork(7, { key: 'ABC-1' });
    expect(lastCall()).toEqual(['link_session_work', { args: { session_id: 7, key: 'ABC-1' } }]);
    await linkSessionWork(7, { key: 'ABC-1' }, { primary: false, expectedVersion: 2 });
    expect(lastCall()).toEqual([
      'link_session_work',
      { args: { session_id: 7, key: 'ABC-1', primary: false, expected_version: 2 } },
    ]);
    await confirmSessionWork(7, 5, { primary: false, expectedVersion: 3 });
    expect(lastCall()).toEqual([
      'confirm_session_work',
      { args: { session_id: 7, link_id: 5, primary: false, expected_version: 3 } },
    ]);
    await rejectWorkLink(7, 5, { expectedVersion: 3 });
    expect(lastCall()).toEqual([
      'reject_session_work',
      { args: { session_id: 7, link_id: 5, expected_version: 3 } },
    ]);
    await unlinkSessionWork(7, 5);
    expect(lastCall()).toEqual(['unlink_session_work', { args: { session_id: 7, link_id: 5 } }]);
  });
});

describe('work:changed', () => {
  it('parses the five whats, ids only', () => {
    expect(parseWorkChanged({ what: 'placement', task_id: 'item:3' })).toEqual({ what: 'placement', task_id: 'item:3' });
    expect(parseWorkChanged({ what: 'org', task_id: 'item:3' })).toEqual({ what: 'org', task_id: 'item:3' });
    expect(parseWorkChanged({ what: 'rule', rule_id: 4 })).toEqual({ what: 'rule', rule_id: 4 });
    expect(parseWorkChanged({ what: 'view', view_id: 9 })).toEqual({ what: 'view', view_id: 9 });
    expect(parseWorkChanged({ what: 'resync' })).toEqual({ what: 'resync' });
    // Extra fields are not carried through.
    expect(parseWorkChanged({ what: 'rule', rule_id: 4, name: 'x' })).toEqual({ what: 'rule', rule_id: 4 });
  });

  it('drops a malformed or unknown frame', () => {
    for (const p of [
      null,
      'placement',
      {},
      { what: 3 },
      { what: 'someday' },
      { what: 'placement', task_id: 3 },
      { what: 'rule', rule_id: '4' },
      { what: 'view', view_id: null },
    ]) {
      expect(parseWorkChanged(p)).toBeNull();
    }
  });
});
