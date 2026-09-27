// work_view.ts (work graph M14.1d): each wrapper's command and argument
// object (`{ args: { … } }`, the fields of the hub action), the session-row
// patch after a decision, and the `work:changed` parsing.
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./sessions', () => ({ acceptCommandRow: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { acceptCommandRow } from './sessions';
import {
  ackWorkLink,
  assignWorkOrg,
  conflictOf,
  decideWorkBatch,
  deleteWorkRule,
  deleteWorkView,
  needsFullReload,
  needsNewerHub,
  parseWorkChanged,
  placeWork,
  reconsiderWorkLink,
  saveWorkRule,
  saveWorkView,
  setPrimaryWork,
  workOrgImpact,
  workReview,
  workRulePreview,
  workRules,
  workSessionTasks,
  workTask,
  workTree,
  workViews,
} from './work_view';
import { confirmSessionWork, linkSessionWork, rejectWorkLink, unlinkSessionWork } from './work';

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue({});
  vi.mocked(acceptCommandRow).mockReset();
});

const lastCall = () => vi.mocked(invoke).mock.calls.at(-1);

describe('the reads', () => {
  it('send each read under its command with its fields, and nothing unset', async () => {
    await workTree();
    expect(lastCall()).toEqual(['work_tree', { args: {} }]);
    await workTree({ filters: { org: 'none', status: 'open' }, cursor: 'c1', limit: 25, perTask: 3 });
    expect(lastCall()).toEqual([
      'work_tree',
      { args: { filters: { org: 'none', status: 'open' }, cursor: 'c1', limit: 25, per_task: 3 } },
    ]);
    await workTask('item:3');
    expect(lastCall()).toEqual(['work_task', { args: { task_id: 'item:3' } }]);
    await workSessionTasks(7);
    expect(lastCall()).toEqual(['work_session_tasks', { args: { session_id: 7 } }]);
    await workReview({ limit: 10 });
    expect(lastCall()).toEqual(['work_review', { args: { limit: 10 } }]);
    await workRules();
    expect(lastCall()).toEqual(['work_rules', { args: {} }]);
    const rule = { name: 'Pay', conditions: { key_prefix: 'PAY' }, group: 'Payments' };
    await workRulePreview(rule);
    expect(lastCall()).toEqual(['work_rule_preview', { args: { rule } }]);
    await workViews();
    expect(lastCall()).toEqual(['work_views', { args: {} }]);
    await workOrgImpact('item:3', 0);
    expect(lastCall()).toEqual(['work_org_impact', { args: { task_id: 'item:3', org_id: 0 } }]);
  });

  it('answers an IpcError as a failed Result', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({ code: 'E_INVALID', message: 'unknown work action "tree"' });
    const r = await workTree();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(needsNewerHub(r.error)).toBe(true);
  });
});

describe('the writes', () => {
  it('a link decision patches the returned session row in place', async () => {
    const row = { id: 7 };
    vi.mocked(invoke).mockResolvedValue(row);
    await setPrimaryWork(7, 5, 4);
    expect(lastCall()).toEqual([
      'set_primary_work',
      { args: { session_id: 7, link_id: 5, expected_primary: 4 } },
    ]);
    await reconsiderWorkLink(7, 5, 3);
    expect(lastCall()).toEqual([
      'reconsider_work_link',
      { args: { session_id: 7, link_id: 5, expected_version: 3 } },
    ]);
    await ackWorkLink(7, 5);
    expect(lastCall()).toEqual(['ack_work_link', { args: { session_id: 7, link_id: 5 } }]);
    expect(vi.mocked(acceptCommandRow).mock.calls).toEqual([[row], [row], [row]]);
  });

  it('a refused decision patches nothing', async () => {
    vi.mocked(invoke).mockRejectedValueOnce({
      code: 'E_CONFLICT',
      message: 'the primary changed',
      details: { primary_link_id: 9 },
    });
    const r = await setPrimaryWork(7, 5, 4);
    expect(acceptCommandRow).not.toHaveBeenCalled();
    expect(r.ok).toBe(false);
    if (!r.ok) expect(conflictOf(r.error)).toEqual({ primary_link_id: 9 });
  });

  it('the structure writes send their action fields', async () => {
    const decisions = [{ session_id: 7, link_id: 5, decision: 'confirm' as const, expected_version: 2, primary: false }];
    await decideWorkBatch(decisions);
    expect(lastCall()).toEqual(['decide_work_batch', { args: { decisions } }]);
    await placeWork('item:3', 'Payments', 0);
    expect(lastCall()).toEqual([
      'place_work',
      { args: { task_id: 'item:3', group: 'Payments', expected_version: 0 } },
    ]);
    await placeWork('item:3', '', 2, 'why');
    expect(lastCall()).toEqual([
      'place_work',
      { args: { task_id: 'item:3', group: '', expected_version: 2, note: 'why' } },
    ]);
    await assignWorkOrg('item:3', 2, 'tok');
    expect(lastCall()).toEqual([
      'assign_work_org',
      { args: { task_id: 'item:3', org_id: 2, impact_token: 'tok' } },
    ]);
    const rule = { id: 4, name: 'Pay', conditions: { key_prefix: 'PAY' }, group: 'Payments', expected_version: 1 };
    await saveWorkRule(rule);
    expect(lastCall()).toEqual(['save_work_rule', { args: { rule } }]);
    await deleteWorkRule(4, 2);
    expect(lastCall()).toEqual(['delete_work_rule', { args: { rule_id: 4, expected_version: 2 } }]);
    const view = { name: 'Mine', filters: { mine: true } };
    await saveWorkView(view);
    expect(lastCall()).toEqual(['save_work_view', { args: { view } }]);
    await deleteWorkView(9);
    expect(lastCall()).toEqual(['delete_work_view', { args: { view_id: 9 } }]);
    expect(acceptCommandRow).not.toHaveBeenCalled();
  });

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

describe('errors', () => {
  it('conflictOf reads only E_CONFLICT', () => {
    expect(conflictOf({ code: 'E_NOTFOUND', message: 'x' })).toBeNull();
    expect(conflictOf({ code: 'E_CONFLICT', message: 'x' })).toEqual({});
    expect(conflictOf({ code: 'E_CONFLICT', message: 'x', details: { version: 3 } })).toEqual({ version: 3 });
  });

  it('needsNewerHub knows both tools and nothing else', () => {
    expect(needsNewerHub({ code: 'E_INVALID', message: 'unknown work_link action "place"; one of …' })).toBe(true);
    expect(needsNewerHub({ code: 'E_INVALID', message: 'place needs expected_version' })).toBe(false);
    expect(needsNewerHub({ code: 'E_NOTFOUND', message: 'unknown work action' })).toBe(false);
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

  it('a resync in the batch means a whole reload', () => {
    expect(needsFullReload([{ what: 'rule', rule_id: 1 }])).toBe(false);
    expect(needsFullReload([{ what: 'rule', rule_id: 1 }, { what: 'resync' }])).toBe(true);
    expect(needsFullReload([])).toBe(false);
  });
});
