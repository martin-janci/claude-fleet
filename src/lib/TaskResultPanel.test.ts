// The result panel (design 2026-10-13, D6): the verdict first, the detail
// folded, and a follow-up that becomes a task from its own row.
import { render, fireEvent } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock('./task_result', async (orig) => ({
  ...(await orig<typeof import('./task_result')>()),
  createFollowupTask: vi.fn(),
}));
vi.mock('./work', async (orig) => ({
  ...(await orig<typeof import('./work')>()),
  setWorkStatus: vi.fn(),
}));

import { openUrl } from '@tauri-apps/plugin-opener';
import TaskResultPanel from './TaskResultPanel.svelte';
import { createFollowupTask as realCreate, type TaskResultView } from './task_result';
import { setWorkStatus as realSetStatus } from './work';

const createFollowupTask = realCreate as unknown as ReturnType<typeof vi.fn>;
const setWorkStatus = realSetStatus as unknown as ReturnType<typeof vi.fn>;

const base: TaskResultView = {
  task_id: 1,
  item_id: 2,
  session_id: 3,
  role: 'wrapup',
  at: 0,
  marker: 'FLEET_TASK_DONE_x',
  report: {
    summary: 'Added the gate.',
    outcome: 'done',
    done: ['gate.rs with completion_gate'],
    decisions: ['the gate never moves an item itself'],
    remaining: [],
    context: ['verify.rs computes on read'],
    tests_run: ['cargo fleet-test'],
    warnings: [],
    blockers: [],
    followups: ['A', 'B'],
    confidence: null,
  },
  evidence: {
    at: 0,
    commits: [{ sha: '0123456789abcdef', subject: 'feat: the gate' }],
    commits_total: 3,
    files: [{ path: 'gate.rs', added: 40, removed: 2 }],
    files_total: 1,
    uncommitted: 2,
  },
  prs: [],
  gate: { state: 'passed', checks: [], open_children: [] },
  followups: [],
};

const withTwoFollowups: TaskResultView = {
  ...base,
  followups: [
    { text: 'A', item_id: null },
    { text: 'B', item_id: null },
  ],
};

const blockedByChild: TaskResultView = {
  ...base,
  gate: {
    state: 'blocked',
    checks: [
      { line: 'CI is green', kind: 'ci', state: 'fail', detail: '2 jobs red' },
      { line: 'Docs updated', kind: 'manual', state: 'pass', detail: '' },
    ],
    open_children: [{ id: 9, title: 'Child open' }],
  },
};

beforeEach(() => {
  createFollowupTask.mockReset();
  setWorkStatus.mockReset();
});

describe('TaskResultPanel', () => {
  it('shows the result first and the detail folded', async () => {
    const { getByTestId, queryByTestId } = render(TaskResultPanel, { result: withTwoFollowups, title: 'Add the gate' });
    expect(getByTestId('task-result-headline').textContent).toContain('Task completed');
    expect(getByTestId('task-result-summary')).toBeTruthy();
    expect(getByTestId('task-result-done').textContent).toContain('gate.rs with completion_gate');
    expect((getByTestId('task-result-changes') as HTMLDetailsElement).open).toBe(false);
    expect((getByTestId('task-result-decisions') as HTMLDetailsElement).open).toBe(false);
    expect(queryByTestId('task-result-gate')).toBeNull();
    expect(queryByTestId('task-result-mark-done')).toBeNull(); // gate passed + already done handled by parent
    expect(queryByTestId('task-result-ask')).toBeNull();
  });

  it('keeps the context for the next session off the panel', () => {
    const { getByTestId } = render(TaskResultPanel, { result: base, title: 'T' });
    expect(getByTestId('task-result').textContent).not.toContain('verify.rs computes on read');
  });

  it('marks a test the worker ran as reported, not passed', () => {
    const { getByTestId } = render(TaskResultPanel, { result: blockedByChild, title: 'T' });
    const rows = Array.from(getByTestId('task-result-validation').querySelectorAll('li')).map((li) => li.textContent);
    expect(rows).toEqual(['passedDocs updated', 'reportedcargo fleet-test']);
  });

  it('counts the changes from the totals and names what is not committed', () => {
    const { getByTestId } = render(TaskResultPanel, { result: base, title: 'T' });
    const text = getByTestId('task-result-changes').textContent ?? '';
    expect(text).toContain('3 commits');
    expect(text).toContain('1 file');
    expect(text).toContain('0123456');
    expect(text).not.toContain('01234567');
    expect(text).toContain('2 more');
    expect(text).toContain('+40 −2');
    expect(text).toContain('2 uncommitted files');
  });

  it('creates a task from a follow-up and then shows it as created', async () => {
    createFollowupTask.mockResolvedValue({ ok: true, value: { id: 77, title: 'A' } });
    const onChanged = vi.fn();
    const onOpenTask = vi.fn();
    const { getAllByTestId, findByTestId } = render(TaskResultPanel, {
      result: withTwoFollowups,
      title: 'T',
      onChanged,
      onOpenTask,
    });
    await fireEvent.click(getAllByTestId('task-result-followup-create')[0]);
    expect(createFollowupTask).toHaveBeenCalledWith(1, { index: 0 });
    const open = await findByTestId('task-result-followup-open-77');
    expect(open.textContent).toContain('Open task');
    expect(getAllByTestId('task-result-followup-create')).toHaveLength(1);
    expect(onChanged).toHaveBeenCalledTimes(1);
    await fireEvent.click(open);
    expect(onOpenTask).toHaveBeenCalledWith(77);
  });

  it('shows a follow-up that already is a task as one', () => {
    const result = { ...base, followups: [{ text: 'A', item_id: 5 }] };
    const { getByTestId, queryByTestId } = render(TaskResultPanel, { result, title: 'T' });
    expect(getByTestId('task-result-followup-open-5')).toBeTruthy();
    expect(queryByTestId('task-result-followup-create')).toBeNull();
  });

  it('says in the row why a follow-up was not created', async () => {
    createFollowupTask.mockResolvedValue({ ok: false, error: { code: 'E_NOTFOUND', message: 'no task 1' } });
    const { getAllByTestId, findByTestId } = render(TaskResultPanel, { result: withTwoFollowups, title: 'T' });
    await fireEvent.click(getAllByTestId('task-result-followup-create')[1]);
    expect((await findByTestId('task-result-followup-error')).textContent).toBe('no task 1');
    expect(getAllByTestId('task-result-followup-create')).toHaveLength(2);
  });

  it('creates a follow-up of the person’s own by title', async () => {
    createFollowupTask.mockResolvedValue({ ok: true, value: { id: 80, title: 'Windows path' } });
    const { getByTestId, findByTestId } = render(TaskResultPanel, { result: base, title: 'T' });
    const input = getByTestId('task-result-followup-title') as HTMLInputElement;
    expect((getByTestId('task-result-followup-own') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(input, { target: { value: '  Windows path ' } });
    await fireEvent.click(getByTestId('task-result-followup-own'));
    expect(createFollowupTask).toHaveBeenCalledWith(1, { title: 'Windows path' });
    expect((await findByTestId('task-result-followup-open-80')).textContent).toContain('Open task');
    expect(input.value).toBe('');
  });

  it('a blocked gate names what blocks', () => {
    const { getByTestId } = render(TaskResultPanel, { result: blockedByChild, title: 'T' });
    const gate = getByTestId('task-result-gate').textContent ?? '';
    expect(getByTestId('task-result-headline').textContent).toContain('Completion blocked');
    expect(gate).toContain('Child open');
    expect(gate).toContain('CI is green');
    expect(gate).toContain('2 jobs red');
    expect(gate).not.toContain('Docs updated');
  });

  it('lets a person mark the task done past a gate that is not passed', async () => {
    setWorkStatus.mockResolvedValue({ ok: true, value: { id: 2 } });
    const onChanged = vi.fn();
    const { getByTestId } = render(TaskResultPanel, { result: blockedByChild, title: 'T', canMarkDone: true, onChanged });
    await fireEvent.click(getByTestId('task-result-mark-done'));
    expect(setWorkStatus).toHaveBeenCalledWith(2, 'done');
    await vi.waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));
  });

  it('shows why marking done failed', async () => {
    setWorkStatus.mockResolvedValue({ ok: false, error: { code: 'E_INVALID_STATE', message: 'not a local task' } });
    const { getByTestId, findByTestId } = render(TaskResultPanel, { result: base, title: 'T', canMarkDone: true });
    await fireEvent.click(getByTestId('task-result-mark-done'));
    expect((await findByTestId('task-result-mark-done-error')).textContent).toBe('not a local task');
  });

  it('opens a PR in the browser and says its state, CI and review', async () => {
    const prs = [{ url: 'https://github.com/o/r/pull/7', number: 7, state: 'open', ci_status: 'passing', review_decision: 'CHANGES_REQUESTED' }];
    const { getByTestId } = render(TaskResultPanel, { result: { ...base, prs }, title: 'T' });
    const chip = getByTestId('task-result-prs').querySelector('button')!;
    expect(chip.textContent).toBe('#7 · open · CI passing · review changes requested');
    await fireEvent.click(chip);
    await vi.waitFor(() => expect(openUrl).toHaveBeenCalledWith('https://github.com/o/r/pull/7'));
  });

  it('offers a question about the result only with a session to ask', () => {
    const { getByTestId } = render(TaskResultPanel, { result: base, title: 'T', sessionId: 3 });
    expect(getByTestId('task-result-ask').textContent).toContain('Ask about result');
  });

  it('hands Ask about result to the parent when it opens the session first (review w09-9)', async () => {
    const onAsk = vi.fn();
    const { getByTestId } = render(TaskResultPanel, { result: base, title: 'T', sessionId: 3, onAsk });
    await fireEvent.click(getByTestId('task-result-ask'));
    expect(onAsk).toHaveBeenCalledTimes(1);
  });

  it('says which role produced the result and when (review w09-10)', () => {
    const at = Math.floor(Date.now() / 1000) - 3 * 86_400;
    const { getByTestId } = render(TaskResultPanel, { result: { ...base, at }, title: 'T' });
    expect(getByTestId('task-result-when').textContent).toBe('Wrap-up · finished 3d ago');
  });

  it('lists an unmet check once, in the gate group (review w09-11)', () => {
    const { getByTestId } = render(TaskResultPanel, { result: blockedByChild, title: 'T' });
    expect(getByTestId('task-result-validation').textContent).not.toContain('CI is green');
    expect(getByTestId('task-result-validation').textContent).toContain('Docs updated');
    expect(getByTestId('task-result-gate').textContent).toContain('CI is green');
  });

  it('caps a typed follow-up at the title limit the store accepts (review w09-12)', () => {
    const { getByTestId } = render(TaskResultPanel, { result: base, title: 'T' });
    expect((getByTestId('task-result-followup-title') as HTMLInputElement).maxLength).toBe(120);
  });
});
