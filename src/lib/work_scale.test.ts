// Work graph M12.2 (scale): the sidebar's group-by-work, `rowMatches` and the
// Today view model over a 2,000-session fleet. Each test asserts the
// algorithmic shape first (every per-row callback runs once per row, never
// per row × group) and a generous wall-clock budget second, printing the
// measured p50 / p95 so a slow CI runner's numbers are in the log.
import { describe, it, expect } from 'vitest';
import type { SessionRow, SessionWork } from './sessions';
import type { TrackerRow } from './trackers';
import {
  buildSessionsByWork,
  rowMatches,
  sortWorkGroups,
  type FilterRow,
  type RowFilters,
} from './sidebar_index';
import { workKeyFor } from './work_keys';
import {
  DEFAULT_WORK_FILTERS,
  sessionWorkRow,
  workFilterPredicate,
  type WorkFilters,
} from './work_filters';
import { scopeOfSession } from './orgs';
import { scopeToday, standupText, type Today, type TodayGroup, type TodaySession } from './today';

const SESSIONS = 2_000;
const HOSTS = 20;
const ORGS = 3;
const KEYS = 800;
const RUNS = 15;

/** mulberry32: a tiny seeded PRNG, so every run builds the same fleet. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
  };
}

const PREFIXES = ['ACME', 'BETA', 'CORE'];
const CATEGORIES = ['todo', 'in_progress', 'done'];

const trackers: TrackerRow[] = PREFIXES.map((p, i) => ({
  id: i + 1,
  provider: 'jira',
  name: p.toLowerCase(),
  site_url: `https://${p.toLowerCase()}.atlassian.net`,
  config: { key_prefixes: [p] },
  state: 'ok',
  created_at: 1,
}));

function row(i: number, r: () => number): SessionRow {
  const host = `h${i % HOSTS}`;
  const org = (i % HOSTS) % (ORGS + 1); // hosts 0,4,8… unassigned
  const roll = r();
  let work: SessionWork | null = null;
  let work_suggested: SessionWork | null = null;
  if (roll < 0.6) {
    const k = Math.floor(r() * KEYS);
    const prefix = PREFIXES[k % PREFIXES.length];
    work = {
      link_id: i + 1,
      item_id: k + 1,
      key: `${prefix}-${k}`,
      title: `Ticket ${k}`,
      source: r() < 0.5 ? 'branch' : 'manual',
      state: 'confirmed',
      strength: 'strong',
      status_category: CATEGORIES[k % 3],
      org_id: org === 0 ? null : org,
      archived_at: r() < 0.1 ? 1 : null,
    };
  } else if (roll < 0.7) {
    work_suggested = {
      link_id: i + 1,
      item_id: null,
      key: `ACME-${i}`,
      title: '',
      source: 'prompt',
      state: 'suggested',
      strength: 'weak',
    };
  }
  return {
    id: i + 1,
    tmux_name: `s${i}`,
    host_alias: host,
    project_id: (i % 40) + 1,
    worktree_id: null,
    created_at: 1_700_000_000 + i,
    last_activity_at: 1_700_000_000 + i * 7,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: i % 25 === 0 ? 'bg' : i % 97 === 0 ? 'external' : 'work',
    reviews_session_id: null,
    // A keyless row on a ticket branch exercises the recogniser fallback.
    worktree_key: roll >= 0.7 && roll < 0.8 ? `feat/CORE-${i}-thing` : `wt${i}`,
    lost_at: null,
    claude_session_id: null,
    claude_status: r() < 0.1 ? 'blocked' : 'idle',
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null,
    turn_seq: 0,
    last_stop_at: null,
    parent_session_id: null,
    tags: [],
    model: null,
    context_tokens: null,
    context_window: null,
    context_source: null,
    context_at: null,
    context_stale: false,
    tmux_pane_id: null,
    pending_input: null,
    work,
    work_suggested,
    org_id: org === 0 ? null : org,
  } as SessionRow;
}

function fleet(seed = 12): SessionRow[] {
  const r = rng(seed);
  return Array.from({ length: SESSIONS }, (_, i) => row(i, r));
}

/** Run `f` RUNS times; print and return p50 / p95 in ms. */
function measure(label: string, f: () => void): { p50: number; p95: number } {
  f(); // warm the JIT
  const t: number[] = [];
  for (let i = 0; i < RUNS; i++) {
    const t0 = performance.now();
    f();
    t.push(performance.now() - t0);
  }
  t.sort((a, b) => a - b);
  const p50 = t[Math.floor(RUNS * 0.5)];
  const p95 = t[Math.min(RUNS - 1, Math.ceil(RUNS * 0.95) - 1)];
  console.log(`[m12.2 scale] ${label}: p50 ${p50.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`);
  return { p50, p95 };
}

const owners = new Map(Array.from({ length: 40 }, (_, i) => [i + 1, `owner${i % 5}`] as [number, string]));
const scopeOf = (s: SessionRow) => scopeOfSession(s, owners);

describe('work graph at scale (M12.2)', () => {
  const rows = fleet();
  const branchById = new Map<number, string>();
  const ctx = { trackers, mine: new Set(Array.from({ length: 200 }, (_, i) => i * 3 + 1)) };

  it('the fixture is deterministic and has the shape it claims', () => {
    expect(fleet()).toEqual(rows);
    expect(rows).toHaveLength(SESSIONS);
    expect(new Set(rows.map((r) => r.host_alias)).size).toBe(HOSTS);
    expect(new Set(rows.map((r) => r.org_id).filter((o) => o != null)).size).toBe(ORGS);
    const linked = rows.filter((r) => r.work).length;
    expect(linked).toBeGreaterThan(SESSIONS * 0.5);
    expect(linked).toBeLessThan(SESSIONS * 0.7);
  });

  it('group-by-work under the work filters is linear in the rows', () => {
    const filters: WorkFilters = { ...DEFAULT_WORK_FILTERS, status: 'in_progress', archived: false };
    const workPredicate = workFilterPredicate(filters, ctx);
    expect(workPredicate).not.toBeNull();
    let keyCalls = 0;
    let predicateCalls = 0;
    let scopeCalls = 0;
    const predicate = (s: SessionRow) => {
      predicateCalls++;
      return workPredicate!(s);
    };
    const keyOf = (s: SessionRow) => {
      keyCalls++;
      return workKeyFor(s, branchById);
    };
    const scope = {
      id: 'org:1',
      of: (s: SessionRow) => {
        scopeCalls++;
        return scopeOf(s);
      },
    };
    const build = () => buildSessionsByWork(rows, 'all', true, predicate, keyOf, scope);

    const out = build();
    const nonExternal = rows.filter((s) => s.kind !== 'external').length;
    // Shape: one key read per non-external row; the scope and the predicate
    // at most once per keyed row — never per row × group.
    expect(keyCalls).toBe(nonExternal);
    expect(out.keyed.size).toBeLessThanOrEqual(nonExternal);
    expect(scopeCalls).toBe(out.keyed.size);
    expect(predicateCalls).toBeLessThanOrEqual(out.keyed.size);

    // Correctness at scale: the groups are exactly the visible keyed rows.
    const expected = rows.filter(
      (s) =>
        s.kind !== 'external' &&
        s.work?.key &&
        scopeOf(s) === 'org:1' &&
        s.work.status_category === 'in_progress' &&
        s.work.archived_at == null,
    );
    const grouped = out.groups.flatMap((g) => g.sessions);
    expect(grouped.map((s) => s.id).sort((a, b) => a - b)).toEqual(expected.map((s) => s.id));
    for (const g of out.groups) {
      for (const s of g.sessions) expect(out.keyed.get(s.id)?.key).toBe(g.key);
    }

    let severityCalls = 0;
    const sorted = sortWorkGroups(out.groups, (s) => {
      severityCalls++;
      return s.claude_status === 'blocked' ? 2 : 0;
    });
    expect(severityCalls).toBe(grouped.length);
    expect(sorted).toHaveLength(out.groups.length);

    const { p95 } = measure(`buildSessionsByWork + sortWorkGroups, ${SESSIONS} rows`, () => {
      const o = build();
      sortWorkGroups(o.groups, (s) => (s.claude_status === 'blocked' ? 2 : 0));
    });
    expect(p95).toBeLessThan(250);
  });

  it('rowMatches over 2,000 work rows under every filter combination', () => {
    const fr: FilterRow[] = rows.map((s) => sessionWorkRow(s, ctx, scopeOf));
    const combos: RowFilters[] = [
      {},
      { host: 'h3' },
      { scope: 'org:2' },
      { tracker: 2, status: 'todo' },
      { assignee: '@me', archived: false },
      { hasSession: 'no' },
      { host: 'h1', scope: 'org:1', showBgAgents: false, status: 'done', predicate: (s) => s.claude_status === 'blocked' },
    ];
    // Shape: each count agrees with a direct reading of the fixture.
    const count = (f: RowFilters) => fr.filter((r) => rowMatches(r, f)).length;
    expect(count({})).toBe(SESSIONS);
    expect(count({ host: 'h3' })).toBe(SESSIONS / HOSTS);
    expect(count({ scope: 'org:2' })).toBe(rows.filter((s) => s.org_id === 2).length);
    expect(count({ hasSession: 'no' })).toBe(0);
    expect(count({ tracker: 2, status: 'todo' })).toBe(
      rows.filter((s) => s.work?.key?.startsWith('BETA-') && s.work.status_category === 'todo').length,
    );
    let predicateCalls = 0;
    fr.filter((r) => rowMatches(r, { predicate: () => (predicateCalls++, true) }));
    expect(predicateCalls).toBe(SESSIONS);

    const { p95 } = measure(`rowMatches × ${combos.length} filters, ${SESSIONS} rows`, () => {
      for (const f of combos) count(f);
    });
    expect(p95).toBeLessThan(100);
    const build = measure(`sessionWorkRow, ${SESSIONS} rows`, () => {
      rows.map((s) => sessionWorkRow(s, ctx, scopeOf));
    });
    expect(build.p95).toBeLessThan(150);
  });

  it('the Today view model over a full digest is linear', () => {
    // The hub caps groups at TODAY_MAX (200); every live session is in one.
    const byKey = new Map<string, TodaySession[]>();
    const noWork: TodaySession[] = [];
    for (const s of rows) {
      const ts: TodaySession = {
        id: s.id,
        name: s.tmux_name,
        host_alias: s.host_alias,
        org_id: s.org_id,
        attention: s.claude_status === 'blocked' ? 'waiting' : null,
        stale: s.id % 11 === 0 ? 'idle' : null,
        pr_url: s.id % 13 === 0 ? `https://github.com/o/r/pull/${s.id}` : null,
        last_activity_at: s.last_activity_at,
      };
      const k = s.work?.key;
      if (!k) noWork.push(ts);
      else if (byKey.has(k) || byKey.size < 199) (byKey.get(k) ?? byKey.set(k, []).get(k)!).push(ts);
      else noWork.push(ts);
    }
    const groups: TodayGroup[] = [...byKey.entries()].map(([key, sessions]) => ({
      bucket: 'in_progress',
      key,
      title: `Title of ${key}`,
      status_name: 'In Progress',
      sessions,
    }));
    groups.push({ bucket: 'in_progress', sessions: noWork });
    const today: Today = {
      since: 0,
      now: 1,
      groups,
      shipped: Array.from({ length: 200 }, (_, i) => ({
        how: i % 2 ? 'done' : 'pr',
        key: `ACME-${i}`,
        title: `Shipped ${i}`,
        at: i,
        org_id: (i % 3) + 1,
      })),
    };
    const total = groups.reduce((n, g) => n + g.sessions.length, 0);
    expect(total).toBe(SESSIONS);

    let scopeCalls = 0;
    const counted = (s: SessionRow) => {
      scopeCalls++;
      return scopeOf(s);
    };
    const view = scopeToday(today, 'org:2', rows, counted);
    // Shape: the scope is read once per digest session, never per group × row.
    expect(scopeCalls).toBe(total);
    const kept = [...view.waiting, ...view.inProgress, ...view.stale].flatMap((g) => g.sessions);
    expect(kept.length).toBe(rows.filter((s) => s.org_id === 2).length);
    expect(view.shipped.every((x) => x.org_id === 2)).toBe(true);
    const text = standupText(view);
    expect(text.split('\n').length).toBeGreaterThan(view.shipped.length);

    const { p95 } = measure(`scopeToday + standupText, ${SESSIONS} sessions / ${groups.length} groups`, () => {
      standupText(scopeToday(today, 'org:2', rows, scopeOf));
      standupText(scopeToday(today, 'all', rows, scopeOf));
    });
    expect(p95).toBeLessThan(150);
  });
});
