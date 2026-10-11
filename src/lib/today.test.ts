// The Today view's pure half (work graph M9.1): scoping the hub's digest the
// way the sidebar scopes rows, and the standup built from what is shown.
import { describe, it, expect } from 'vitest';
import { session } from './hosts_fixture';
import {
  shippedWhen,
  bucketOf,
  localMidnight,
  scopeToday,
  standupText,
  isEmptyView,
  type Today,
  type TodaySession,
} from './today';
import type { SessionRow } from './sessions';
import { ALL_SCOPES } from './orgs';

const s = (id: number, name: string, over: Partial<TodaySession> = {}): TodaySession => ({
  id,
  name,
  host_alias: 'mefistos',
  last_activity_at: 100,
  ...over,
});

const digest: Today = {
  since: 0,
  now: 200,
  groups: [
    {
      bucket: 'waiting',
      key: 'PAY-7',
      title: 'Refund flow',
      status_name: 'In Progress',
      sessions: [s(1, 'pay', { attention: 'waiting' }), s(2, 'pay-tests', { org_id: 2 })],
    },
    {
      bucket: 'in_progress',
      key: 'PAY-9',
      title: 'Ledger',
      sessions: [s(3, 'ledger', { pr_url: 'https://gh/pr/9', ci_status: 'passing' })],
    },
    { bucket: 'stale', key: 'OLD-1', title: '', sessions: [s(4, 'old', { stale: 'idle' })] },
    { bucket: 'in_progress', sessions: [s(5, 'scratch')] },
  ],
  shipped: [
    { how: 'done', key: 'PAY-3', title: 'Receipts', url: 'https://x/PAY-3', pr_url: 'https://gh/pr/3', at: 150, org_id: 1 },
    { how: 'pr', key: 'ENG-2', title: 'Other', pr_url: 'https://gh/pr/4', at: 140, org_id: 2 },
  ],
};

function rows(): SessionRow[] {
  return [
    session('mefistos', 'pay', { id: 1, org_id: 1 }),
    session('mefistos', 'pay-tests', { id: 2, org_id: 2 }),
    session('mefistos', 'ledger', { id: 3, org_id: 1 }),
    session('mefistos', 'old', { id: 4, org_id: 1 }),
    session('mefistos', 'scratch', { id: 5, org_id: 1 }),
  ];
}
const scopeOf = (r: SessionRow) => (r.org_id != null ? `org:${r.org_id}` : 'unassigned');

describe('bucketOf', () => {
  it('waiting beats stale beats in progress, as on the hub', () => {
    expect(bucketOf([s(1, 'a', { attention: 'stuck' }), s(2, 'b', { stale: 'idle' })])).toBe('waiting');
    expect(bucketOf([s(1, 'a', { stale: 'idle' }), s(2, 'b', { stale: 'done' })])).toBe('stale');
    expect(bucketOf([s(1, 'a', { stale: 'idle' }), s(2, 'b')])).toBe('in_progress');
    expect(bucketOf([])).toBe('in_progress');
  });
});

describe('scopeToday', () => {
  it('all: every group in its bucket, every shipped entry', () => {
    const v = scopeToday(digest, 'all', rows(), scopeOf);
    expect(v.waiting.map((g) => g.key)).toEqual(['PAY-7']);
    expect(v.inProgress.map((g) => g.key ?? null)).toEqual(['PAY-9', null]);
    expect(v.stale.map((g) => g.key)).toEqual(['OLD-1']);
    expect(v.shipped).toHaveLength(2);
  });

  it('a scope drops the other org’s sessions and re-buckets what is left', () => {
    const v = scopeToday(digest, 'org:2', rows(), scopeOf);
    // PAY-7 waited because of session 1 (org 1); in org 2 only pay-tests is left.
    expect(v.waiting).toEqual([]);
    expect(v.inProgress.map((g) => g.key)).toEqual(['PAY-7']);
    expect(v.inProgress[0].sessions.map((x) => x.id)).toEqual([2]);
    expect(v.shipped.map((x) => x.key)).toEqual(['ENG-2']);
  });

  it('a session with no live row is kept only for all', () => {
    const v = scopeToday(digest, 'org:1', rows().filter((r) => r.id !== 3), scopeOf);
    expect(v.inProgress.map((g) => g.key ?? null)).toEqual([null]);
    expect(scopeToday(digest, 'owner:acme', rows(), scopeOf).shipped).toEqual([]);
  });
});

describe('standupText', () => {
  it('is plain text in a stable order: Shipped, In progress, Waiting on me, Stale', () => {
    expect(standupText(scopeToday(digest, 'all', rows(), scopeOf))).toBe(
      [
        'Shipped',
        '- PAY-3 Receipts — done https://gh/pr/3',
        '- ENG-2 Other — PR https://gh/pr/4',
        '',
        'In progress',
        '- PAY-9 Ledger — PR https://gh/pr/9 (CI passing) · ledger',
        '- scratch',
        '',
        'Waiting on me',
        '- PAY-7 Refund flow — In Progress · pay (waiting for an answer), pay-tests',
        '',
        'Stale',
        '- OLD-1 — old (idle)',
        '',
      ].join('\n'),
    );
  });

  it('says so when there is nothing', () => {
    const empty = scopeToday({ since: 0, now: 1, groups: [], shipped: [] }, 'all', [], scopeOf);
    expect(isEmptyView(empty)).toBe(true);
    expect(standupText(empty)).toBe('Nothing to report.\n');
  });

  it('keeps markup in a tracker title as the text it is', () => {
    const t: Today = {
      since: 0,
      now: 1,
      groups: [],
      shipped: [{ how: 'done', key: 'X-1', title: '<b>bold</b>', at: 1 }],
    };
    expect(standupText(scopeToday(t, 'all', [], scopeOf))).toContain('- X-1 <b>bold</b> — done');
  });
});

describe('localMidnight', () => {
  it('is the local start of the day, in seconds', () => {
    const noon = new Date(2026, 8, 25, 12, 30).getTime();
    expect(localMidnight(noon)).toBe(Math.floor(new Date(2026, 8, 25).getTime() / 1000));
  });
});

describe('G1.6: missions waiting on you and Jev proposals', () => {
  const t: Today = {
    since: 0,
    now: 200,
    groups: [{ bucket: 'in_progress', key: 'PD-3', title: 'Docs', sessions: [s(9, 'docs', { proposed: 'probably_waiting' })] }],
    shipped: [],
    missions: [
      { id: 1, name: 'Hub federation v2', waiting_on: { reason: 'sign_grant', since: 10, open_cards: 0 } },
      { id: 2, name: 'Other org', org_id: 2, waiting_on: { reason: 'question', since: 20, open_cards: 1 } },
    ],
  };

  it('lists the missions in the standup and keeps a proposal in progress', () => {
    const v = scopeToday(t, ALL_SCOPES, [], () => ALL_SCOPES);
    expect(v.waiting).toEqual([]);
    expect(v.inProgress).toHaveLength(1);
    expect(isEmptyView(v)).toBe(false);
    expect(standupText(v)).toContain('- Mission Hub federation v2 — sign the autonomy grant');
    expect(standupText(v)).toContain('docs (probably waiting)');
  });

  it('cuts the missions to an org scope', () => {
    const v = scopeToday(t, 'org:2' as never, [], () => ALL_SCOPES);
    expect(v.missions?.map((m) => m.id)).toEqual([2]);
  });
});

describe('shippedWhen and the standup provenance (G7.9)', () => {
  it('says how it shipped and the local time', () => {
    const at = new Date(2026, 9, 10, 9, 12).getTime() / 1000;
    expect(shippedWhen({ how: 'pr', at })).toBe('PR 09:12');
    expect(shippedWhen({ how: 'done', at })).toBe('done 09:12');
  });

  it('the standup names where a shipped entry came from', () => {
    const t = { since: 0, now: 10, groups: [], shipped: [{ how: 'pr', key: 'A-1', title: 'Sweep', at: 5, from: 'Morning PR sweep' }] };
    expect(standupText(scopeToday(t, ALL_SCOPES, [], () => ALL_SCOPES))).toContain('· from Morning PR sweep');
  });
});
