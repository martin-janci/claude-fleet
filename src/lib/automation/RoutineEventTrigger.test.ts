// Pull request triggers in the routine editor (M15 step G2.4): the event,
// "Only when repo / author" and the rate, saved with the routine and read
// back in its Definition.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RoutinesPanel from './RoutinesPanel.svelte';
import { hosts } from '../hosts';
import { projects } from '../projects';
import { eventFilterInput, eventFilterOf, eventFilterWords, failing, triggerWords, type RoutineRow } from '../routines';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const onReview: RoutineRow = {
  id: 5,
  name: 'Answer reviews',
  enabled: true,
  trigger: 'event',
  event: 'pr_review',
  event_repo: 'martin-janci/claude-fleet',
  event_author: 'anyone',
  event_rate_secs: 3600,
  utc_offset_min: 0,
  host_alias: 'mac',
  project_id: 1,
  prompt: 'Answer the review comments.',
  overlap: 'skip',
  skip_next: false,
  created_at: 1,
  updated_at: 1,
};

const argsOf = (action: string) =>
  inv.mock.calls.map((c) => c[1]?.args).filter((a) => a?.action === action).at(-1);

function route(list: RoutineRow[]) {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a: { args: { action: string; routine?: { name: string } } }) => {
    if (cmd !== 'routines') return null;
    switch (a.args.action) {
      case 'list':
        return list;
      case 'get':
        return { routine: list[0], runs: [], may_change: true };
      case 'failing':
        return [];
      case 'save':
        return { ...onReview, id: 9, name: a.args.routine!.name };
      default:
        return list[0];
    }
  });
}

beforeEach(() => {
  hosts.set([{ alias: 'mac', hidden: false } as never]);
  projects.set([{ project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', system: false }, worktrees: [] } as never]);
  failing.set([]);
});

describe('pull request triggers in the routine editor', () => {
  it('saves a routine that starts when a PR gets a review, for one repo, anyone, once per PR per hour', async () => {
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    await fireEvent.change(screen.getByTestId('routine-trigger'), { target: { value: 'event' } });
    // A session event offers no repo or author.
    expect(screen.queryByTestId('routine-event-repo')).toBeNull();
    await fireEvent.change(screen.getByTestId('routine-event'), { target: { value: 'pr_review' } });
    const repo = (await screen.findByTestId('routine-event-repo')) as HTMLInputElement;
    await fireEvent.input(repo, { target: { value: ' claude-fleet ' } });
    await fireEvent.change(screen.getByTestId('routine-event-author'), { target: { value: 'anyone' } });
    const rate = screen.getByTestId('routine-event-rate') as HTMLSelectElement;
    expect(Array.from(rate.options).map((o) => o.textContent)).toContain('once per PR per hour');
    await fireEvent.change(rate, { target: { value: '3600' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    const saved = argsOf('save').routine;
    expect(saved).toMatchObject({
      trigger: 'event',
      event: 'pr_review',
      event_repo: 'claude-fleet',
      event_author: 'anyone',
      event_rate_secs: 3600,
    });
    expect(saved.cron).toBeUndefined();
  });

  it('reads a PR routine back in its Definition', async () => {
    route([onReview]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-tab-definition'));
    expect(screen.getByTestId('routine-definition').textContent).toContain(
      'When a pull request gets a review · only in martin-janci/claude-fleet · anyone’s · once per PR per hour',
    );
  });
});

describe('event filter helpers', () => {
  it('sends repo and author for a PR event only, the rate for any event, nothing for a schedule', () => {
    const f = { repo: 'acme/web', author: 'anyone' as const, rate: '600' };
    expect(eventFilterInput('event', 'pr_merged', f)).toEqual({ event_repo: 'acme/web', event_author: 'anyone', event_rate_secs: 600 });
    expect(eventFilterInput('event', 'stuck', f)).toEqual({ event_rate_secs: 600 });
    expect(eventFilterInput('cron', 'pr_merged', f)).toEqual({});
    expect(eventFilterInput('event', 'pr_merged', { repo: ' ', author: 'me', rate: '' })).toEqual({});
    expect(eventFilterOf({})).toEqual({ repo: '', author: 'me', rate: '' });
  });

  it('words a trigger and its filters', () => {
    expect(triggerWords({ trigger: 'event', event: 'pr_ci_failed' })).toBe('When a pull request’s checks fail');
    expect(triggerWords({ trigger: 'event', event: 'stuck' })).toBe('When a session is stuck');
    expect(eventFilterWords({ event: 'pr_merged' })).toBe('mine');
    expect(eventFilterWords({ event: 'stuck', event_rate_secs: 86400 })).toBe('once per session per day');
    expect(eventFilterWords({ event: 'stuck' })).toBe('');
  });
});
