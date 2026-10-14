import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AgentsDebug from './AgentsDebug.svelte';
import { jobCountsLine } from '../agents';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const rule = { id: 4, agent: 'review', scope: 'gh:o/r', level: 'watch', host_alias: 'mac', project_id: null, config: '{}', granted_by: 'x', granted_at: 1, expires_at: null };
const failedJob = { id: 9, rule_id: 4, agent: 'review', subject: 'gh:o/r#3', trigger: 'poll', dedup_key: 'k', state: 'failed', attempts: 3, next_attempt_at: 5, lease_until: null, error_kind: 'transient', error: 'claude exited 1', result: null, cost_micros: 0, created_at: 1, updated_at: 2 };

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a: { action: string; id: number | null }) => {
    if (cmd !== 'agents_debug') return null;
    if (a.action === 'explain')
      return {
        automation_paused: false,
        poll_interval_secs: 300,
        max_concurrent: 2,
        max_retries: 3,
        loops: [],
        rules: [
          { rule, active: true, state: 'warn', summary: 'Its last poll failed: gh: HTTP 401', polled_at: 100, poll_error: { at: 120, message: 'gh: HTTP 401' }, jobs: { failed: 1, done: 2 }, last_failed: failedJob },
        ],
      };
    return { job: failedJob, rule, actions: [], summary: 'Failed after 3 attempt(s): claude exited 1 (transient).' };
  });
});

describe('the Agents Debug section', () => {
  it('loads when opened, says why a rule is stuck, and opens its last failed job', async () => {
    render(AgentsDebug, { props: { clock: (s: number) => `t${s}` } });
    expect(invoke).not.toHaveBeenCalled();
    const d = screen.getByTestId('agents-debug') as HTMLDetailsElement;
    d.open = true;
    await fireEvent(d, new Event('toggle'));
    const rows = await screen.findAllByTestId('agents-debug-rule');
    expect(invoke).toHaveBeenCalledWith('agents_debug', { action: 'explain', id: null });
    expect(rows[0].textContent).toContain('Its last poll failed: gh: HTTP 401');
    expect(rows[0].textContent).toContain('1 failed · 2 done');
    expect(rows[0].textContent).toContain('Poll failed t120');
    await fireEvent.click(screen.getByTestId('agents-debug-last-failed'));
    await waitFor(() => expect(screen.getByTestId('agents-debug-job').textContent).toContain('Failed after 3 attempt(s)'));
    expect(invoke).toHaveBeenCalledWith('agents_debug', { action: 'job', id: 9 });
  });

  it('counts jobs in a fixed order', () => {
    expect(jobCountsLine({ done: 5, pending: 2, failed: 1 })).toBe('2 pending · 1 failed · 5 done');
    expect(jobCountsLine({})).toBe('no jobs yet');
  });
});
