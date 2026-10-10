import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import {
  inboxCount,
  inboxRows,
  inboxSections,
  nextInInbox,
  notWaiting,
  notWaitingSaid,
  notWaitingText,
  proposedRows,
  proposedText,
  sayNotWaiting,
} from './inbox';
import { sessions } from './sessions';
import { waitingMissions, waitingOf } from './mission_waits';
import { attentionState, countNeedsYou } from './attention';
import { session } from './hosts_fixture';

// Redesign step 3.3: the Inbox lists exactly what raises the badge.

const opts = { idleSecs: 600, now: 2_000_000_000 };

const rows = [
  session('mac', 'asking', { claude_status: 'blocked' }),
  session('mac', 'crashed', { claude_status: 'failed' }),
  session('mac', 'busy', { claude_status: 'working' }),
  session('mac', 'quiet', { claude_status: 'idle', last_activity_at: opts.now - 60 }),
  session('mac', 'gone', { status: 'ghost', claude_status: 'idle' }),
  session('mac', 'shell', { kind: 'shell', claude_status: 'blocked' }),
];

describe('inbox', () => {
  it('lists the rows the badge counts, worst first, and nothing else', () => {
    const inbox = inboxRows(rows, opts);
    expect(inbox.map((s) => s.tmux_name)).toEqual(['asking', 'crashed']);
    expect(inbox.length).toBe(countNeedsYou(rows, opts));
  });

  it('a ghost leaves the Inbox and is counted below it as paused', () => {
    expect(attentionState(rows[4], opts)).toBe('paused');
    const n = notWaiting(rows, opts);
    expect(n).toEqual({ working: 1, idle: 2, completedToday: 0, done: 0, paused: 1 });
    expect(notWaitingText(n)).toBe('1 running · 2 idle · 1 paused');
    expect(notWaitingText({ working: 0, idle: 0, completedToday: 0, done: 0, paused: 0 })).toBe('');
  });
});

describe('nextInInbox (redesign 5.9)', () => {
  const row = (id: number) => ({ id }) as unknown as import('./sessions').SessionRow;
  it('takes the next row in order, wraps round, and never the one just answered', () => {
    const q = [row(1), row(2), row(3)];
    expect(nextInInbox(q, 2)?.id).toBe(3);
    expect(nextInInbox(q, 3)?.id).toBe(1);
    expect(nextInInbox(q, 9)?.id).toBe(1);
    expect(nextInInbox([row(4)], 4)).toBeNull();
    expect(nextInInbox([], 4)).toBeNull();
  });
});

describe('G1.6: Jev proposals and waiting missions', () => {
  const jev = session('mac', 'maybe', { claude_status: 'idle', turn_outcome: 'asked', last_stop_at: 50 });

  it('keeps a Jev proposal out of the Inbox and the badge, in its own list', () => {
    const all = [...rows, jev];
    expect(inboxRows(all, opts).map((s) => s.tmux_name)).toEqual(['asking', 'crashed']);
    expect(countNeedsYou(all, opts)).toBe(2);
    expect(proposedRows(all, opts).map((s) => s.tmux_name)).toEqual(['maybe']);
    expect(proposedText(1)).toBe('+1 proposed');
    expect(proposedText(0)).toBe('');
  });

  it('"Not waiting" sets one reading aside, and a later turn is a new one', () => {
    sayNotWaiting(jev);
    const aside = get(notWaitingSaid);
    expect(proposedRows([jev], opts, aside)).toEqual([]);
    expect(proposedRows([{ ...jev, last_stop_at: 90 }], opts, aside)).toHaveLength(1);
    notWaitingSaid.set(new Set());
  });

  it('counts a mission waiting on a person in the rail badge', () => {
    sessions.set([]);
    waitingMissions.set([]);
    expect(get(inboxCount)).toBe(0);
    waitingMissions.set(
      waitingOf([
        { id: 1, name: 'Hub federation v2', goal: 'g', mode: 'finite', state: 'active', level: 2, plan_version: 1, created_at: 1, updated_at: 1, version: 1, waiting_on: { reason: 'sign_grant', since: 10, open_cards: 0 } },
      ]),
    );
    expect(get(inboxCount)).toBe(1);
    waitingMissions.set([]);
  });
});

describe('G3.1: the Inbox by state, from the attention model', () => {
  const none = { missions: 0, failingRoutines: 0, proposed: 0 };
  const limited = session('mac', 'limited', { claude_status: 'idle', account_uuid: 'acc' });
  const facts = { limited_accounts: { acc: { window: 'weekly' as const, resets_at: opts.now + 3600 } } };
  const withFacts = { ...opts, facts };

  it('splits the queue into Needs you (Action required and Blocked) and Failed', () => {
    const all = [...rows, limited];
    const secs = inboxSections(all, withFacts, 'state', none);
    expect(secs.map((s) => [s.key, s.label, s.count])).toEqual([
      ['needs_you', 'Needs you', 2],
      ['failed', 'Failed', 1],
    ]);
    expect(secs[0].rows.map((s) => s.tmux_name)).toEqual(['asking', 'limited']);
    expect(secs[1].rows.map((s) => s.tmux_name)).toEqual(['crashed']);
    // The sections add up to the badge.
    expect(secs.reduce((n, s) => n + s.count, 0)).toBe(countNeedsYou(all, withFacts));
  });

  it('puts missions and Jev proposals in Needs you and failed routines in Failed', () => {
    const secs = inboxSections([], opts, 'state', { missions: 1, failingRoutines: 2, proposed: 1 });
    expect(secs.map((s) => [s.key, s.count, s.missions, s.proposed, s.routines])).toEqual([
      ['needs_you', 1, true, true, false],
      ['failed', 2, false, false, true],
    ]);
    // A proposal alone opens Needs you, and is never in its count.
    expect(inboxSections([], opts, 'state', { ...none, proposed: 1 }).map((s) => [s.key, s.count])).toEqual([
      ['needs_you', 0],
    ]);
    expect(inboxSections([rows[2]], opts, 'state', none)).toEqual([]);
  });

  it('as one queue: every row worst first, under no header', () => {
    const secs = inboxSections(rows, opts, 'none', { ...none, missions: 1 });
    expect(secs).toHaveLength(1);
    expect(secs[0].label).toBeNull();
    expect(secs[0].rows.map((s) => s.tmux_name)).toEqual(['asking', 'crashed']);
    expect(secs[0].count).toBe(3);
    expect(inboxSections([rows[2]], opts, 'none', none)).toEqual([]);
  });

  it('counts the turns that finished since midnight as completed today', () => {
    const midnight = opts.now - 3600;
    const today = [
      session('mac', 'shipped', { claude_status: 'completed', last_stop_at: opts.now - 60 }),
      session('mac', 'unread', { claude_status: 'idle', last_stop_at: opts.now - 30, started_at: opts.now - 900 }),
      session('mac', 'yesterday', { claude_status: 'idle', last_stop_at: midnight - 60 }),
      session('mac', 'busy', { claude_status: 'working', last_stop_at: opts.now - 60 }),
    ];
    expect(attentionState(today[1], opts)).toBe('done');
    const n = notWaiting(today, opts, midnight);
    expect(n).toEqual({ working: 1, idle: 1, completedToday: 2, done: 0, paused: 0 });
    expect(notWaitingText(n)).toBe('1 running · 1 idle · 2 completed today');
  });
});
