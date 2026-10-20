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
});
