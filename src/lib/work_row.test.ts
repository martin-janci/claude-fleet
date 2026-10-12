// The Work list row's words and chips (board "Work · tasks with filters
// open").
import { describe, it, expect } from 'vitest';
import { link, task } from './work_view_fixture';
import { costChip, prChip, prNumber, stageOf, taskLine, taskTone } from './work_row';

describe('work_row', () => {
  it('reads the stage the hub sent, else one from the tracker for an older hub', () => {
    expect(stageOf(task({ stage: 'in_review' }))).toBe('in_review');
    expect(stageOf(task({ stage: undefined, status_category: 'done' }))).toBe('done');
    expect(stageOf(task({ stage: undefined, status_category: 'todo', blocked: true }))).toBe('blocked');
    expect(stageOf(task({ stage: undefined, status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } }))).toBe('backlog');
  });

  it('says what needs a person first: needs you, then a failed session, then why', () => {
    const waiting = task({ needs_you: true, sessions: [link({ needs_you: true })] });
    expect(taskTone(waiting)).toBe('waiting');
    expect(taskLine(waiting)).toEqual({ lead: 'In progress', failed: false, why: 'session needs you' });

    const broken = task({ sessions: [link({ claude_status: 'failed', name: 'tests' })] });
    expect(taskTone(broken)).toBe('failed');
    expect(taskLine(broken)).toEqual({ lead: 'Session failed', failed: true, why: 'tests' });

    const working = task({ sessions: [link({ claude_status: 'working', name: 'test suite', host: 'mercury' })] });
    expect(taskTone(working)).toBe('working');
    expect(taskLine(working).why).toBe('test suite on mercury');

    const quiet = task({ sessions: [], tracker_name: 'Jira PD', status_category: 'todo', counts: { active: 0, ended: 0, suggested: 0 } });
    expect(taskTone(quiet)).toBe('idle');
    expect(taskLine(quiet)).toEqual({ lead: 'Backlog', failed: false, why: 'Jira PD' });
  });

  it('a live session’s pull request is a chip, with its checks when the row has them', () => {
    const t = task({ sessions: [link({ session_id: 7, pr_url: 'https://github.com/o/r/pull/476' })] });
    expect(prNumber('https://github.com/o/r/pull/476')).toBe('#476');
    expect(prChip(t, new Map())).toMatchObject({ label: 'PR #476', checks: null });
    const failing = new Map([[7, { pr_evidence: { draft: false, checks: { total: 9, pending: 0, skipped: 0, failing_total: 1 } } }]]);
    expect(prChip(t, failing)).toMatchObject({ checks: 'failing', failing: 1 });
    const green = new Map([[7, { pr_evidence: { draft: false, checks: { total: 15, pending: 0, skipped: 0, failing_total: 0 } } }]]);
    expect(prChip(t, green)?.checks).toBe('passing');
    // An ended session's PR is not this task's live one.
    expect(prChip(task({ sessions: [link({ state: 'ended', session_id: null, pr_url: 'x/pull/1' })] }), new Map())).toBeNull();
  });

  it('the spend chip shows only what was spent', () => {
    expect(costChip({ cost_micros: 8_510_000 })).toBe('$8.51');
    expect(costChip({ cost_micros: 0 })).toBeNull();
    expect(costChip({})).toBeNull();
  });
});
