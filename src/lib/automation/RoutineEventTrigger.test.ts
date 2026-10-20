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
import { EVENT_CHOICES, eventFilterInput, eventFilterOf, eventFilterWords, eventHostChoices, eventLabel, failing, startedByWords, triggerWords, type RoutineRow } from '../routines';
import { stashDraft } from './routine_draft';

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
  // A changed draft an earlier test left open must not come back (review w05-2).
  stashDraft(null);
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
    await fireEvent.click(screen.getByTestId('routine-trigger-event'));
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
    const f = { repo: 'acme/web', author: 'anyone' as const, rate: '600', host: '' };
    expect(eventFilterInput('event', 'pr_merged', f)).toEqual({ event_repo: 'acme/web', event_author: 'anyone', event_rate_secs: 600 });
    expect(eventFilterInput('event', 'stuck', f)).toEqual({ event_rate_secs: 600 });
    expect(eventFilterInput('cron', 'pr_merged', f)).toEqual({});
    expect(eventFilterInput('event', 'pr_merged', { repo: ' ', author: 'me', rate: '', host: '' })).toEqual({});
    expect(eventFilterOf({})).toEqual({ repo: '', author: 'me', rate: '', host: '' });
  });

  it('words a trigger and its filters', () => {
    expect(triggerWords({ trigger: 'event', event: 'pr_ci_failed' })).toBe('When a pull request’s checks fail');
    expect(triggerWords({ trigger: 'event', event: 'stuck' })).toBe('When a session is stuck');
    expect(eventFilterWords({ event: 'pr_merged' })).toBe('mine');
    expect(eventFilterWords({ event: 'stuck', event_rate_secs: 86400 })).toBe('once per session per day');
    expect(eventFilterWords({ event: 'stuck' })).toBe('');
  });
});

describe('fleet events and the host filter', () => {
  it('offers every event the hub fires on, each with its own words', () => {
    for (const e of ['needs_you', 'auto_restored', 'account_switched', 'session_moved', 'pr_opened', 'host_offline', 'account_limited', 'mission_done'])
      expect(EVENT_CHOICES).toContain(e);
    expect(new Set(EVENT_CHOICES.map(eventLabel)).size).toBe(EVENT_CHOICES.length);
    expect(triggerWords({ trigger: 'event', event: 'host_offline' })).toBe('When a host goes offline');
    expect(triggerWords({ trigger: 'event', event: 'account_limited' })).toBe('When an account hits its limit');
    expect(triggerWords({ trigger: 'event', event: 'needs_you' })).toBe('When a session is waiting for you');
  });

  it('sends a host only for an event about a host', () => {
    const f = { repo: '', author: 'me' as const, rate: '3600', host: 'nas' };
    expect(eventFilterInput('event', 'host_offline', f)).toEqual({ event_host: 'nas', event_rate_secs: 3600 });
    expect(eventFilterInput('event', 'stuck', f)).toEqual({ event_host: 'nas', event_rate_secs: 3600 });
    expect(eventFilterInput('event', 'account_limited', f)).toEqual({ event_rate_secs: 3600 });
    expect(eventFilterOf({ event_host: 'nas' }).host).toBe('nas');
    expect(eventFilterWords({ event: 'host_offline', event_host: 'nas', event_rate_secs: 3600 })).toBe('on nas · once per host per hour');
    expect(eventFilterWords({ event: 'account_limited', event_rate_secs: 86400 })).toBe('once per account per day');
  });

  it('saves a routine that starts when a host goes offline, on one host', async () => {
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    await fireEvent.click(screen.getByTestId('routine-trigger-event'));
    await fireEvent.change(screen.getByTestId('routine-event'), { target: { value: 'host_offline' } });
    const host = (await screen.findByTestId('routine-event-host')) as HTMLSelectElement;
    expect(Array.from(host.options).map((o) => o.value)).toEqual(['', 'mac']);
    await fireEvent.change(host, { target: { value: 'mac' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    expect(argsOf('save').routine).toMatchObject({ trigger: 'event', event: 'host_offline', event_host: 'mac' });
  });
});

describe('a rate no preset names (review w05-10)', () => {
  it('opens on the saved rate in words, not a blank select', async () => {
    route([{ ...onReview, event: 'stuck', event_repo: undefined, event_author: undefined, event_rate_secs: 1800 }]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    const rate = screen.getByTestId('routine-event-rate') as HTMLSelectElement;
    expect(rate.value).toBe('1800');
    expect(rate.selectedOptions[0].textContent).toBe('once per session per 30 minutes');
  });
});

describe('review w06: event words, rate, host choices and run refs', () => {
  it('names the failing session events whole, as the phone does', () => {
    expect(eventLabel('stop_failure')).toBe('A session’s turn fails');
    expect(eventLabel('auto_restore_failed')).toBe('An automatic restore attempt fails');
    expect(eventLabel('account_switch_failed')).toBe('An account switch fails');
    expect(triggerWords({ trigger: 'event', event: 'stop_failure' })).toBe('When a session’s turn fails');
    expect(triggerWords({ trigger: 'event', event: 'auto_restore_failed' })).toBe('When an automatic restore attempt fails');
  });

  it('counts a signed-out login per login, as the hub does', () => {
    expect(eventFilterWords({ event: 'login_signed_out', event_rate_secs: 3600 })).toBe('once per login per hour');
  });

  it('offers a fleet event only the hosts of the routine host’s org', () => {
    const list = [
      { alias: 'mac', hidden: false, org_id: 1 },
      { alias: 'nas', hidden: false, org_id: 2 },
      { alias: 'pi', hidden: false, org_id: 1 },
      { alias: 'old', hidden: true, org_id: 1 },
    ];
    expect(eventHostChoices(list, 'host_offline', 'mac')).toEqual(['mac', 'pi']);
    expect(eventHostChoices(list, 'stuck', 'mac')).toEqual(['mac', 'nas', 'pi']);
    expect(eventHostChoices(list, 'host_offline', undefined)).toEqual(['mac', 'nas', 'pi']);
    expect(eventHostChoices([{ alias: 'a' }, { alias: 'b', org_id: 3 }], 'host_low', 'a')).toEqual(['a']);
  });

  it('says what an event run was about, not its raw ref', () => {
    expect(startedByWords({ trigger: 'event', trigger_ref: 'session:12:345' })).toBe('an event on session #12');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'pr:2:9' })).toBe('an event on a pull request');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'fleet:host:mac:42' })).toBe('an event on host mac');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'fleet:mission:4:7' })).toBe('an event on mission #4');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'fleet:account:u-1:8' })).toBe('an event on account u-1');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'fleet:login:mac:work:9' })).toBe('an event on the work login on mac');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'odd' })).toBe('an event (odd)');
  });
});
