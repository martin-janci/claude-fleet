import { describe, expect, it } from 'vitest';
import {
  activityEvents,
  commentAuthor,
  commentTimeline,
  deliveryOf,
  globMatch,
  hasDelivery,
  postStateLabel,
  promoteTitle,
  promotionLabel,
  startRuleFor,
  workSpan,
} from './task_detail';
import { link, task } from './work_view_fixture';

describe('task_detail', () => {
  it('promotionLabel says a ticket is on its way, or why it was not made', () => {
    expect(promotionLabel(null)).toBeNull();
    expect(promotionLabel({ state: 'pending', tracker: 'Jira' })).toBe('Creating the ticket in Jira…');
    expect(promotionLabel({ state: 'failed', tracker: 'Jira', error: 'not permitted: 403' })).toBe('Not created in Jira: not permitted: 403');
    expect(promotionLabel({ state: 'failed', tracker: '' })).toBe('Not created in the tracker: the tracker refused it');
  });

  it('promoteTitle names the parent ticket, the one tracker, or none', () => {
    const t = { tracker_id: 1, tracker: 'Jira', provider: 'jira' };
    expect(promoteTitle([])).toBe('');
    expect(promoteTitle([{ ...t, parent_key: 'ABC-1' }])).toBe('Create it as a subtask of ABC-1 in Jira');
    expect(promoteTitle([t])).toBe('Create it as a ticket in Jira');
    expect(promoteTitle([t, { ...t, tracker_id: 2, tracker: 'Asana' }])).toBe('Create it as a ticket in a tracker');
  });

  it('globMatch mirrors the backend: * is any run, case ignored', () => {
    expect(globMatch('PD-*', 'pd-12')).toBe(true);
    expect(globMatch('PD-*', 'PX-12')).toBe(false);
    expect(globMatch('*-12', 'PD-12')).toBe(true);
    expect(globMatch('PD-1*2', 'PD-1002')).toBe(true);
    expect(globMatch('PD-1', 'PD-12')).toBe(false);
    expect(globMatch('PD-**', 'PD-')).toBe(true);
  });

  it('startRuleFor picks the most specific active rule', () => {
    const rules = [
      { pattern: 'PD-*', state: 'active' },
      { pattern: 'PD-1*', state: 'active' },
      { pattern: 'PD-12', state: 'offered' },
    ];
    expect(startRuleFor('PD-12', rules)?.pattern).toBe('PD-1*');
    expect(startRuleFor('PD-2', rules)?.pattern).toBe('PD-*');
    expect(startRuleFor(null, rules)).toBeNull();
    expect(startRuleFor('AB-1', rules)).toBeNull();
  });

  it('workSpan runs from the first link to the last end, or now while one is live', () => {
    const ended = task({
      sessions: [
        link({ link_id: 1, state: 'ended', created_at: 1000, ended_at: 4600 }),
        link({ link_id: 2, state: 'ended', created_at: 2000, ended_at: 8200 }),
      ],
    });
    expect(workSpan(ended, 99_999)).toBe(7200);
    const live = task({ sessions: [link({ link_id: 1, state: 'active', created_at: 1000 })] });
    expect(workSpan(live, 1600)).toBe(600);
    expect(workSpan(task({ sessions: [] }), 10)).toBeNull();
  });

  it('a task with nothing delivered has no block', () => {
    const d = deliveryOf(task({ sessions: [], status_name: null, cost_micros: 0, assignees: [], due_at: null, mine: false }), null, new Map(), 0);
    expect(hasDelivery(d)).toBe(false);
  });
});

describe('comments and activity', () => {
  it('merges fleet’s comments and the tracker’s by time, fleet first on a tie', () => {
    const fleet = [
      { id: 1, item_id: 1, author: 'desktop', body: 'a', created_at: 200 },
      { id: 2, item_id: 1, author: 'desktop', body: 'b', created_at: 400 },
    ];
    const tracker = [
      { id: '10', author: 'Ann', body: 'x', created_at: 100 },
      { id: '11', author: 'Bob', body: 'y', created_at: 200 },
    ];
    expect(commentTimeline(fleet, tracker).map((e) => `${e.kind}:${e.comment.id}`)).toEqual(['tracker:10', 'fleet:1', 'tracker:11', 'fleet:2']);
  });

  it('says where a fleet comment’s post to the tracker stands', () => {
    expect(postStateLabel(null, 'Jira')).toBeNull();
    expect(postStateLabel({ state: 'done' }, 'Jira')).toBe('Sent to Jira');
    expect(postStateLabel({ state: 'pending' }, 'Jira')).toBe('Sending…');
    expect(postStateLabel({ state: 'pending', error: 'rate limited' }, 'Jira')).toBe('Sending… (retrying: rate limited)');
    expect(postStateLabel({ state: 'failed', error: 'not permitted: 403' }, 'Jira')).toBe('Not sent: not permitted: 403');
  });

  it('names a comment’s author as a person reads it', () => {
    expect(commentAuthor({ author: 'client:phone' })).toBe('phone');
    expect(commentAuthor({ author: 'host:mac' })).toBe('An agent on mac');
    expect(commentAuthor({ author: 'desktop' })).toBe('This desktop');
    expect(commentAuthor({ author: '' })).toBe('Someone');
    expect(commentAuthor({ author: 'client:phone', mine: true })).toBe('You');
  });

  it('lists what happened, newest first', () => {
    const t = task({
      sessions: [
        link({ link_id: 1, name: 'api', host: 'h-a', state: 'ended', created_at: 100, ended_at: 300, end_reason: 'switched' }),
        link({ link_id: 2, name: 'web', host: null, state: 'suggested', created_at: 200 }),
        link({ link_id: 3, name: 'old', host: null, state: 'rejected', created_at: 50, decided_at: 60 }),
      ],
    });
    const comments = [{ id: 1, item_id: 1, author: 'client:phone', body: 'x', created_at: 250 }];
    expect(activityEvents({ task: t, comments }).map((e) => [e.at, e.text])).toEqual([
      [300, 'api stopped (switched)'],
      [250, 'phone commented'],
      [200, 'web was suggested'],
      [100, 'api on h-a started on it'],
      [60, 'old was marked “not this”'],
      [50, 'old was suggested'],
    ]);
  });
});
