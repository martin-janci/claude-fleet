import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  actionBody,
  actionDetails,
  boldRuns,
  fieldLabel,
  actionOutcome,
  actionTitle,
  agents,
  agentState,
  groupActions,
  answerAgentAction,
  jobLine,
  pendingAgentCount,
  recentAgentOutcomes,
  ruleLine,
  comparisonLine,
  trackAgents,
  type AgentAction,
  type AgentJob,
  type AgentRule,
  type AgentsStatus,
} from './agents';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

function rule(over: Partial<AgentRule> = {}): AgentRule {
  return {
    id: 1,
    agent: 'review',
    scope: 'gh:o/r',
    level: 'propose',
    host_alias: 'mercury',
    project_id: 3,
    config: '{}',
    granted_by: 'console',
    granted_at: 0,
    expires_at: null,
    bindings: '[]',
    active: true,
    ...over,
  };
}

function job(over: Partial<AgentJob> = {}): AgentJob {
  return {
    id: 1,
    rule_id: 1,
    agent: 'review',
    subject: 'gh:o/r#4',
    trigger: 'opened',
    dedup_key: 'gh:o/r#4@abc',
    state: 'done',
    attempts: 1,
    next_attempt_at: 0,
    lease_until: null,
    error_kind: null,
    error: null,
    result: '{"verdict":"REQUEST_CHANGES","inline":1}',
    cost_micros: 0,
    created_at: 0,
    updated_at: 0,
    ...over,
  };
}

function action(over: Partial<AgentAction> = {}): AgentAction {
  return {
    id: 7,
    job_id: 1,
    kind: 'post_review',
    payload: JSON.stringify({ repo: 'o/r', number: 4, event: 'REQUEST_CHANGES', body: '## 🔄 Changes Requested', comments: [{}, {}] }),
    state: 'pending',
    decided_by: null,
    decided_at: null,
    executed_at: null,
    error: null,
    created_at: 0,
    ...over,
  };
}

function status(over: Partial<AgentsStatus> = {}): AgentsStatus {
  return { rules: [rule()], jobs: [job()], pending: [action()], ...over };
}

beforeEach(() => {
  invoke.mockReset();
  agents.set(null);
});

describe('agents (design step 5)', () => {
  it('names the support agent\'s Jira writes', () => {
    expect(actionTitle({ kind: 'jira_comment', payload: '{"key":"L2S-7","body":"h3. x"}' })).toBe('Comment on L2S-7');
    expect(actionTitle({ kind: 'work_task', payload: '{"title":"Flaky reconnect","steps":["a","b"]}' })).toBe(
      'Create a task: Flaky reconnect (2 steps)',
    );
    expect(actionTitle({ kind: 'fix_session', payload: '{"project":"o/r","host":"htz","title":"Fix backoff"}' })).toBe(
      'Start a session on o/r (htz): Fix backoff',
    );
    expect(actionTitle({ kind: 'setting', payload: '{"key":"hub.probe_secs","value":"60","was":"30"}' })).toBe(
      'Change hub.probe_secs from 30 to 60',
    );
    expect(actionTitle({ kind: 'session_recover', payload: '{"session":"fed-v2","host":"htz"}' })).toBe('Recover fed-v2 on htz');
    expect(
      actionTitle({ kind: 'jira_create_bug', payload: '{"project":"PD","source_key":"L2S-7","summary":"[pay] NPE"}' }),
    ).toBe('File a bug in PD for L2S-7: [pay] NPE');
    expect(actionTitle({ kind: 'jira_link', payload: '{"bug_key":"PD-4","source_key":"L2S-7","link_type":"Causes"}' })).toBe(
      'Link PD-4 to L2S-7 (causes)',
    );
    expect(actionBody({ payload: '{"body":"h3. x"}' })).toBe('h3. x');
    expect(actionTitle({ kind: 'kb_learn', payload: '{"path":"errors/pay-NPE-3.yaml"}' })).toBe(
      'Teach the support KB: errors/pay-NPE-3.yaml',
    );
    expect(actionTitle({ kind: 'tracker_comment', payload: '{"key":"L2S-7","body":"x"}' })).toBe('Comment on L2S-7');
    expect(
      actionTitle({
        kind: 'tracker_issue',
        payload: '{"project":"BUG","issue_type":"Bug","summary":"Key expired","link":{"to":"L2S-7","link_type":"Causes"}}',
      }),
    ).toBe('File a bug in BUG for L2S-7: Key expired');
    expect(actionTitle({ kind: 'tracker_link', payload: '{"from":"BUG-9","to":"L2S-7","link_type":"Causes"}' })).toBe(
      'Link BUG-9 to L2S-7 (causes)',
    );
    expect(actionTitle({ kind: 'record', payload: '{"store":"memory","key":"NPE"}' })).toBe('Remember NPE in memory');
    expect(actionTitle({ kind: 'mcp_call', payload: '{"binding":"kb","tool":"add_known_error"}' })).toBe(
      'Call kb.add_known_error',
    );
    expect(actionTitle({ kind: 'pr_comment', payload: '{"repo":"o/r","number":4,"body":"x"}' })).toBe('Comment on o/r#4');
    expect(
      actionTitle({
        kind: 'pr_review',
        payload: '{"repo":"o/r","number":4,"event":"REQUEST_CHANGES","body":"x","findings":[{"path":"a","line":1,"body":"y"}]}',
      }),
    ).toBe('Review o/r#4: request changes, 1 line comment');
    expect(actionTitle({ kind: 'jira_create_bug', payload: '{"project":"PD","source_key":null,"summary":"x"}' })).toBe(
      'File a bug in PD for a logged error: x',
    );
    expect(actionTitle({ kind: 'pr_labels', payload: '{"repo":"o/r","number":4,"add":["core"],"remove":["lgtm"]}' })).toBe(
      'Label o/r#4: +core −lgtm',
    );
    expect(actionTitle({ kind: 'pr_description', payload: '{"repo":"o/r","number":4,"body":"x"}' })).toBe('Describe o/r#4');
    expect(actionTitle({ kind: 'pr_reply', payload: '{"repo":"o/r","number":4,"root_id":7,"resolve":true}' })).toBe(
      'Reply in a review thread on o/r#4 and resolve it',
    );
    expect(actionTitle({ kind: 'debate_reply', payload: '{"repo":"o/r","number":4,"decision":"hold"}' })).toBe(
      'Answer the test debate on o/r#4: hold',
    );
  });

  it('says what an action will do and what it will post', () => {
    expect(actionTitle(action())).toBe('Post a review on o/r#4: request changes, 2 comments');
    expect(actionTitle(action({ payload: JSON.stringify({ repo: 'o/r', number: 4, event: 'APPROVE', comments: [{}] }) }))).toBe(
      'Post a review on o/r#4: approve, 1 comment',
    );
    expect(actionTitle(action({ kind: 'jira_comment', payload: 'not json' }))).toBe('jira comment');
    expect(actionBody(action())).toBe('## 🔄 Changes Requested');
    expect(actionBody(action({ payload: '{}' }))).toBeNull();
  });

  it('sums up a comparison', () => {
    expect(comparisonLine({ pairs: [], agree: 3, differ: 1, builtin_only: 2, fleet_only: 0 })).toBe(
      '3 agree, 1 differs; 2 built-in only, 0 fleet only',
    );
    expect(comparisonLine({ pairs: [], agree: 0, differ: 2, builtin_only: 0, fleet_only: 1 })).toBe(
      '0 agree, 2 differ; 0 built-in only, 1 fleet only',
    );
    expect(comparisonLine({ pairs: [], agree: 1, differ: 0, builtin_only: 0, fleet_only: 0 })).toBe(
      '1 agrees, 0 differ; 0 built-in only, 0 fleet only',
    );
  });

  it('names rules, jobs and each agent’s state', () => {
    expect(ruleLine(rule())).toBe('gh:o/r at propose on mercury');
    expect(ruleLine(rule({ active: false }))).toBe('gh:o/r at propose on mercury (expired)');
    expect(ruleLine(rule({ budget_micros: 25_000_000, spent_today_micros: 3_200_000 }))).toBe(
      'gh:o/r at propose on mercury · $3.20 of $25.00 today',
    );
    expect(jobLine(job())).toBe('gh:o/r#4 · done · request changes');
    expect(jobLine(job({ state: 'skipped', error: 'draft', result: null }))).toBe('gh:o/r#4 · skipped: draft');
    expect(jobLine(job({ subject: 'jira:L2S-1', result: '{"severity":"critical","confidence":"high"}' }))).toBe(
      'jira:L2S-1 · done · critical, high confidence',
    );
    expect(agentState('review', null)).toBe('not reported');
    expect(agentState('support', status())).toBe('off · no rule');
    expect(agentState('review', status({ jobs: [job({ state: 'running' })] }))).toBe('1 rule · 1 working · 1 waiting for you');
  });

  it('says what an approved action came to (review w08-3)', () => {
    const task = { kind: 'work_task', payload: JSON.stringify({ action: 'task', title: 'Flaky reconnect' }) };
    const title = actionTitle(task);
    expect(actionOutcome({ ...task, state: 'executed', error: 'created task 42' })).toBe(`Done: ${title} — created task 42`);
    expect(actionOutcome({ ...task, state: 'executed', error: null })).toBe(`Done: ${title}`);
    expect(actionOutcome({ ...task, state: 'failed', error: 'task 17 created; the session did not start: x' })).toBe(
      `Failed: ${title} — task 17 created; the session did not start: x`,
    );
    agents.set(status({ recent: [action({ state: 'executed', error: 'posted' })] }));
    expect(get(recentAgentOutcomes)).toHaveLength(1);
    // An older hub sends no `recent`.
    agents.set(status());
    expect(get(recentAgentOutcomes)).toEqual([]);
  });

  it('counts the pending actions and reads again after an answer', async () => {
    agents.set(status());
    expect(get(pendingAgentCount)).toBe(1);
    invoke.mockImplementation(async (cmd: string) =>
      cmd === 'agents_decide' ? { ...action(), state: 'approved' } : status({ pending: [] }),
    );
    const r = await answerAgentAction(7, true);
    expect(r.ok && r.value.state).toBe('approved');
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['agents_decide', 'agents_status']);
    expect(invoke.mock.calls[0][1]).toEqual({ id: 7, approve: true });
    expect(get(pendingAgentCount)).toBe(0);
  });

  it('counts the whole backlog when the pending list is cut', () => {
    agents.set(status({ pending_total: 250 }));
    expect(get(pendingAgentCount)).toBe(250);
  });

  it('keeps the status fresh, and stops asking a hub that refuses this device', async () => {
    vi.useFakeTimers();
    const load = vi.fn().mockResolvedValue({ ok: true, value: status() });
    const stop = trackAgents({ load, doc: null, every: 1000 });
    await vi.advanceTimersByTimeAsync(0);
    expect(get(pendingAgentCount)).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(load).toHaveBeenCalledTimes(2);
    stop();
    const refused = vi.fn().mockResolvedValue({ ok: false, error: { code: 'E_FORBIDDEN', message: 'not your device' } });
    trackAgents({ load: refused, doc: null, every: 1000 });
    await vi.advanceTimersByTimeAsync(5000);
    expect(refused).toHaveBeenCalledTimes(1);
    expect(get(agents)).toBeNull();
    vi.useRealTimers();
  });

  it('skips its reads while the window is blurred, and reads once on focus', async () => {
    vi.useFakeTimers();
    const load = vi.fn().mockResolvedValue({ ok: true, value: { rules: [], jobs: [], pending: [] } });
    const win = new EventTarget() as Window;
    const stop = trackAgents({ load, doc: null, win, every: 1000 });
    await vi.advanceTimersByTimeAsync(0);
    win.dispatchEvent(new Event('blur'));
    await vi.advanceTimersByTimeAsync(5000);
    expect(load).toHaveBeenCalledTimes(1);
    win.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(load).toHaveBeenCalledTimes(2);
    stop();
    vi.useRealTimers();
  });
});

describe('groupActions', () => {
  const a = (id: number, kind: string, payload: object = {}) => ({
    id,
    job_id: 1,
    kind,
    payload: JSON.stringify(payload),
    state: 'pending',
    decided_by: null,
    decided_at: null,
    executed_at: null,
    error: null,
    created_at: id,
  });
  it('folds by the hub\'s group key, in order', () => {
    const ab = (id: number, group: string) => ({ id, from: 'x', what: 'w', group });
    const g = groupActions([a(3, 'mcp_call'), a(2, 'kb_learn'), a(1, 'mcp_call')], [ab(3, 'g1'), ab(2, 'g2'), ab(1, 'g1')]);
    expect(g.map((x) => [x.key, x.items.map((i) => i.action.id)])).toEqual([
      ['g1', [3, 1]],
      ['g2', [2]],
    ]);
  });
  it('folds only tool calls for an older hub', () => {
    const call = { binding: 'jira', tool: 't' };
    const g = groupActions([a(3, 'mcp_call', call), a(2, 'kb_learn'), a(1, 'kb_learn'), a(0, 'mcp_call', call)], undefined);
    expect(g.map((x) => x.items.length)).toEqual([2, 1, 1]);
  });
  it('prefers the hub\'s title', () => {
    const x = a(1, 'mcp_call', { binding: 'jira', tool: 'jira_update_issue' });
    expect(actionTitle(x)).toBe('Call jira.jira_update_issue');
    expect(actionTitle(x, { title: 'Jira: update issue PD-1 (status)' })).toBe('Jira: update issue PD-1 (status)');
  });
});

describe('actionDetails (readable Details)', () => {
  it('labels arguments in words', () => {
    expect(fieldLabel('issue_key')).toBe('Issue key');
    expect(fieldLabel('returnIssue')).toBe('Return issue');
  });

  it('keeps a string as written, joins lists, and indents only what is nested', () => {
    const d = actionDetails({
      kind: 'mcp_call',
      payload: JSON.stringify({ binding: 'jira', tool: 'x', arguments: { labels: ['a', 'b'], fields: { a: 1 }, text: 'one\ntwo' } }),
    });
    expect(d?.tool).toBe('jira · x');
    expect(d?.fields).toEqual([
      { label: 'Labels', value: 'a, b', code: false },
      { label: 'Fields', value: '{\n  "a": 1\n}', code: true },
      { label: 'Text', value: 'one\ntwo', code: false },
    ]);
  });

  it('shows any other action\'s body as it is, and nothing when there is none', () => {
    expect(actionDetails({ kind: 'jira_comment', payload: '{"body":"h3. x"}' })?.fields).toEqual([{ label: '', value: 'h3. x', code: false }]);
    expect(actionDetails({ kind: 'jira_link', payload: '{"bug_key":"PD-4"}' })).toBeNull();
  });

  it('draws markdown bold without its stars and touches nothing else', () => {
    expect(boldRuns('**A** b **c**')).toEqual([
      { bold: true, text: 'A' },
      { bold: false, text: ' b ' },
      { bold: true, text: 'c' },
    ]);
    expect(boldRuns('a * b <i>')).toEqual([{ bold: false, text: 'a * b <i>' }]);
  });
});
