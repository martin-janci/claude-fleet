// Work graph M12.2 (scale): the sidebar's group-by-work, `rowMatches` and the
// Today view model over a 2,000-session fleet. Each test asserts the
// algorithmic shape first (every per-row callback runs once per row, never
// per row × group) and a generous wall-clock budget second, printing the
// measured p50 / p95 so a slow CI runner's numbers are in the log.
import { describe, it, expect, afterAll, beforeAll } from 'vitest';
import { get } from 'svelte/store';
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
import {
  accessOf,
  resetAccessForTests,
  sessionAccess,
  setMyGrants,
  type SessionAccess,
} from './access';
import { hubStatus, STANDALONE } from './hub';
import { scopeToday, standupText, type Today, type TodayGroup, type TodaySession } from './today';

const SESSIONS = 2_000;
const HOSTS = 20;
const ORGS = 3;
const KEYS = 800;
const RUNS = 15;
/** People on the fleet (multi-user M1). Five is enough to make the owner
 *  comparison miss four times out of five, which is what the derivation's
 *  cost is being measured against. */
const PEOPLE = 5;
/** How many of the fleet's sessions are shared with the measuring person.
 *  The budget has to measure a `Map` LOOKUP, not an empty map's fast path. */
const GRANTS = 500;

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
    // Multi-user M1. Derived from `i`, deliberately NOT from the rng: taking a
    // draw here would shift every later value and move the counts the other
    // tests in this file assert against the fixture.
    //
    // One row in 37 is `unclaimed` — a tmux session fleet did not start — and
    // an unclaimed row has no owner, which is also the shape that makes the
    // derivation's `owner_person_id != null` guard matter.
    visibility: i % 37 === 0 ? 'unclaimed' : 'private',
    owner_person_id: i % 37 === 0 ? null : (i % PEOPLE) + 1,
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

  // Multi-user M1 (F2): `sessionAccess` is now on the render path for every
  // row the sidebar draws — each one asks it whether its action buttons are
  // enabled, and `SessionRowItem` asks it again for the privacy badge. So it
  // gets the same treatment as every other per-row function here: the shape
  // first (O(1) per row — two field reads and one `Map` lookup, and in
  // particular NOT a scan of the grant set), and a budget second.
  describe('the per-row access derivation at scale (multi-user M1)', () => {
    /** The measuring person owns one fifth of the fleet and is granted `watch`
     *  or `drive` on GRANTS of the rest, so the map is big enough that its
     *  lookup is what the clock is measuring. */
    const me = 1;
    const granted = rows
      .filter((s) => s.owner_person_id !== me)
      .slice(0, GRANTS)
      .map((s, i) => ({ session_id: s.id, level: i % 2 === 0 ? 'watch' : 'drive' }));

    beforeAll(() => {
      // A PAIRED desktop: standalone short-circuits to `own` on the backend
      // mode alone (rule 1) and would measure nothing at all.
      hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
      setMyGrants(me, granted);
    });
    afterAll(() => {
      hubStatus.set({ ...STANDALONE });
      resetAccessForTests();
    });

    it('answers every row from two field reads and one map lookup', () => {
      const grantLevel = new Map(granted.map((g) => [g.session_id, g.level]));
      const of = get(accessOf);
      const want = (s: SessionRow): SessionAccess => {
        if (s.owner_person_id === me) return 'own';
        return (grantLevel.get(s.id) as 'watch' | 'drive' | undefined) ?? null;
      };
      // Correctness at scale, including the two traps: an `unclaimed` row
      // (`owner_person_id` null) is NOT owned on a paired desktop, and a row
      // owned by somebody else with no grant answers `null` rather than a
      // level.
      for (const s of rows) expect(of(s)).toBe(want(s));
      expect(rows.some((s) => of(s) === 'own')).toBe(true);
      expect(rows.some((s) => of(s) === 'watch')).toBe(true);
      expect(rows.some((s) => of(s) === 'drive')).toBe(true);
      expect(rows.filter((s) => of(s) === null).length).toBeGreaterThan(SESSIONS / 2);
      // The map is actually populated: an empty one would make the budget
      // below measure the wrong thing.
      expect(grantLevel.size).toBe(GRANTS);

      // The RENDER path: a component reads `$accessOf` once and applies it per
      // row, so the three store reads happen once for the whole list.
      const applied = measure(`accessOf, ${SESSIONS} rows / ${GRANTS} grants`, () => {
        const f = get(accessOf);
        for (const s of rows) f(s);
      });
      expect(applied.p95).toBeLessThan(100);

      // The bare function with its `get()` defaults, for contrast: it subscribes
      // and unsubscribes to three stores PER CALL, which is the constant the
      // derived store exists to pay once. Measured so the gap is on the record
      // rather than rediscovered by whoever next calls it in a loop — the budget
      // is deliberately loose, because what matters is that it is still linear.
      const bare = measure(`sessionAccess (get() per row), ${SESSIONS} rows`, () => {
        for (const s of rows) sessionAccess(s);
      });
      expect(bare.p95).toBeLessThan(400);
    });
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
