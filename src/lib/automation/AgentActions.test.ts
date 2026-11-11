// Review w11-13: one item's call returning must not re-enable another
// item's buttons while that item's own call is still in flight.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { tick } from 'svelte';

const decide = new Map<number, (v: unknown) => void>();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((cmd: string, args: { id?: number }) => {
    if (cmd === 'agents_decide') return new Promise((res) => decide.set(args.id!, res));
    return Promise.resolve({ rules: [], jobs: [], pending: [action(1), action(2)] });
  }),
}));

import AgentActions from './AgentActions.svelte';
import { agents, type AgentAction } from '../agents';

function action(id: number): AgentAction {
  return {
    id,
    job_id: 1,
    kind: 'kb_learn',
    payload: '{}',
    state: 'pending',
    decided_by: null,
    decided_at: null,
    executed_at: null,
    error: null,
    created_at: 1,
  };
}

afterEach(() => {
  agents.set(null);
  decide.clear();
});

describe('AgentActions', () => {
  it('keeps B busy when A returns first', async () => {
    agents.set({ rules: [], jobs: [], pending: [action(1), action(2)] });
    render(AgentActions);
    await tick();
    const [a, b] = screen.getAllByTestId('agent-action-approve');
    await fireEvent.click(a);
    await fireEvent.click(b);
    expect(a).toBeDisabled();
    expect(b).toBeDisabled();
    decide.get(1)!(action(1));
    await vi.waitFor(() => expect(a).not.toBeDisabled());
    expect(b).toBeDisabled();
  });

  it('folds one agent\'s repeats of a write into one card that names each item', async () => {
    const call = (id: number, key: string) => ({
      ...action(id),
      kind: 'mcp_call',
      payload: JSON.stringify({ binding: 'jira', tool: 'jira_update_issue', arguments: { issue_key: key }, body: 'x' }),
    });
    const about = (id: number, key: string) => ({
      id,
      rule_id: 4,
      from: 'jira-triage · PD board',
      subject: 'a scheduled run',
      title: `Jira: update issue ${key} (status)`,
      what: 'Jira: update issue',
      group: '4:mcp_call:jira_update_issue',
    });
    agents.set({
      rules: [],
      jobs: [],
      pending: [call(1, 'PD-1'), call(2, 'PD-2'), action(3)],
      pending_about: [about(1, 'PD-1'), about(2, 'PD-2')],
    });
    render(AgentActions);
    await tick();
    const group = screen.getByTestId('agent-action-group');
    expect(group.textContent).toContain('2 proposals: Jira: update issue');
    expect(group.textContent).toContain('from jira-triage · PD board');
    expect(screen.getByTestId('agent-group-approve').textContent).toContain('Approve all 2');
    // The other proposal stays a card of its own.
    expect(screen.getAllByTestId('agent-action')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('agent-group-show'));
    const items = screen.getByTestId('agent-group-items');
    expect(items.textContent).toContain('Jira: update issue PD-1 (status)');
    expect(items.textContent).toContain('on a scheduled run');
  });

  it('shows what an mcp_call will send as labelled, readable fields, not escaped JSON', async () => {
    const description = '**Context**\n\nThe report page, **Custom range**.\n\nPicking a ("Custom") range…';
    agents.set({
      rules: [],
      jobs: [],
      pending: [
        {
          ...action(1),
          kind: 'mcp_call',
          payload: JSON.stringify({
            binding: 'jira',
            tool: 'jira_update_issue',
            arguments: { description, issue_key: 'DEMO-42', returnIssue: true },
            body: 'jira.jira_update_issue({…})',
          }),
        },
      ],
    });
    render(AgentActions);
    await tick();
    await fireEvent.click(screen.getByTestId('agent-action-show'));
    const body = screen.getByTestId('agent-action-body');
    const labels = Array.from(body.querySelectorAll('dt')).map((d) => d.textContent);
    // Short fields first, the long text last.
    expect(labels).toEqual(['Issue key', 'Return issue', 'Description']);
    expect(body.textContent).toContain('jira · jira_update_issue');
    expect(body.textContent).toContain('yes');
    expect(body.textContent).not.toContain('\\n');
    expect(body.textContent).not.toContain('**');
    expect(body.textContent).toContain('("Custom")');
    expect(Array.from(body.querySelectorAll('strong')).map((b) => b.textContent)).toEqual(['Context', 'Custom range']);
  });
});
