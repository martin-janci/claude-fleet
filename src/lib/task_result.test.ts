import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  createFollowupTask,
  headline,
  requestWorkResult,
  normalizeResult,
  validationLines,
  workResult,
  type TaskResultView,
} from './task_result';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const base: TaskResultView = {
  task_id: 1, item_id: 2, session_id: 3, role: 'wrapup', at: 0, marker: 'FLEET_TASK_DONE_x',
  report: { summary: 's', outcome: 'done', done: [], decisions: [], remaining: [], context: [],
            tests_run: ['cargo fleet-test'], warnings: [], blockers: [], followups: [], confidence: null },
  evidence: null, prs: [], followups: [],
  gate: { state: 'passed', checks: [], open_children: [] },
};

describe('task result', () => {
  it('lists gate checks before reported tests and marks the latter as reported', () => {
    const v = { ...base, gate: { ...base.gate, checks: [{ line: 'ci', kind: 'ci', state: 'pass', detail: '' }] } };
    expect(validationLines(v)).toEqual([
      { label: 'ci', state: 'pass' },
      { label: 'cargo fleet-test', state: 'reported' },
    ]);
  });
  it('reads a check that is neither passed nor failed as pending', () => {
    const checks = [
      { line: 'a', kind: 'ci', state: 'fail', detail: '' },
      { line: 'b', kind: 'manual', state: 'unknown', detail: '' },
    ];
    expect(validationLines({ ...base, gate: { ...base.gate, checks } }).map((l) => l.state)).toEqual([
      'fail',
      'pending',
      'reported',
    ]);
  });
  it('puts the run evidence between the gate lines and the reported tests', () => {
    const missing = { state: 'missing', task_id: 9, detail: 'the implement run committed nothing and changed no file' };
    expect(validationLines({ ...base, gate: { ...base.gate, state: 'blocked', evidence: missing } })).toEqual([
      { label: 'Run evidence: the implement run committed nothing and changed no file', state: 'fail' },
      { label: 'cargo fleet-test', state: 'reported' },
    ]);
    const waived = { ...missing, state: 'overridden', detail: 'a config change on the host' };
    expect(validationLines({ ...base, gate: { ...base.gate, evidence: waived } })[0].state).toBe('pass');
    expect(normalizeResult({ ...base, gate: { state: 'passed', checks: [], open_children: [] } }).gate.evidence).toBeNull();
  });
  it('says completed only when the report is done and the gate passed', () => {
    expect(headline(base)).toEqual({ label: 'Task completed', tone: 'success' });
    expect(headline({ ...base, gate: { ...base.gate, state: 'blocked' } }))
      .toEqual({ label: 'Completion blocked', tone: 'danger' });
    expect(headline({ ...base, gate: { ...base.gate, state: 'unverified' } }))
      .toEqual({ label: 'Reported done, not verified', tone: 'warning' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'partial' } }))
      .toEqual({ label: 'Needs you · partly done', tone: 'warning' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'failed' } }))
      .toEqual({ label: 'Failed', tone: 'danger' });
    expect(headline({ ...base, report: { ...base.report, outcome: 'blocked' } }))
      .toEqual({ label: 'Needs you · blocked', tone: 'danger' });
  });
});

describe('task result commands', () => {
  beforeEach(() => {
    inv.mockReset();
    inv.mockResolvedValue(null);
  });

  it('asks for a result by item or by session', async () => {
    await workResult({ itemId: 7 });
    await workResult({ sessionId: 9 });
    expect(inv.mock.calls).toEqual([
      ['work_result', { args: { item_id: 7 } }],
      ['work_result', { args: { session_id: 9 } }],
    ]);
  });

  it('fills in the lists and options the backend leaves out', async () => {
    // What the Rust view serialises for a report with nothing but a summary:
    // empty lists and unset options are skipped, not sent as [] / null.
    inv.mockResolvedValue({
      task_id: 1, item_id: 2, role: 'wrapup', at: 0, marker: 'FLEET_TASK_DONE_x',
      report: { summary: 's', outcome: 'done' },
      prs: [{ url: 'https://github.com/o/r/pull/1' }],
      gate: { state: 'passed', checks: [], open_children: [] },
      followups: [{ text: 'next' }],
    });
    const r = await workResult({ itemId: 2 });
    if (!r.ok || !r.value) throw new Error('expected a result');
    const v = r.value;
    expect(v.report.blockers).toEqual([]);
    expect(v.report.tests_run).toEqual([]);
    expect(v.session_id).toBeNull();
    expect(v.evidence).toBeNull();
    expect(v.prs[0].number).toBeNull();
    expect(v.followups).toEqual([{ text: 'next', item_id: null }]);
    expect(validationLines(v)).toEqual([]);
  });

  it('asks a session for its result', async () => {
    await requestWorkResult(9);
    expect(inv).toHaveBeenCalledWith('request_work_result', { args: { session_id: 9 } });
  });

  it('creates a follow-up by index or by title, never both', async () => {
    await createFollowupTask(1, { index: 0 });
    await createFollowupTask(1, { title: 'Windows path' });
    expect(inv.mock.calls).toEqual([
      ['create_followup_task', { args: { task_id: 1, index: 0 } }],
      ['create_followup_task', { args: { task_id: 1, title: 'Windows path' } }],
    ]);
  });

  it('hands a refusal back as it came', async () => {
    inv.mockRejectedValue({ code: 'E_NOTFOUND', message: 'no task 1' });
    expect(await createFollowupTask(1, { index: 0 })).toEqual({
      ok: false,
      error: { code: 'E_NOTFOUND', message: 'no task 1' },
    });
  });
});
