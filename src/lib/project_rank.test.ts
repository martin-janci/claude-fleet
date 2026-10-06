import { describe, it, expect } from 'vitest';
import type { ProjectTreeRow } from './projects';
import { pickKey, type ProjectPick } from './project_picks';
import {
  buildSections, entriesOf, groupProjects, hiddenReason, isDormant, searchEntries, FORKS, type Ctx,
} from './project_rank';

const NOW = 1_800_000_000;
const DAY = 86_400;
let id = 1;
const proj = (owner: string, repo: string, daysAgo: number | null = 1, system = false): ProjectTreeRow => ({
  project: { id: id++, owner, repo, base_path: `/p/${repo}`, last_session_at: daysAgo === null ? null : NOW - daysAgo * DAY, adopted: false, system },
  worktrees: [],
});
const picks = (...ps: Array<Partial<ProjectPick> & { owner: string; repo: string }>) =>
  new Map(ps.map((p) => [pickKey(p.owner, p.repo), { pinned: false, vis: null, grp: null, ...p } as ProjectPick]));
const noCtx: Ctx = { selectedProjectId: null, preferredHost: null, sessions: [] };
const labels = (s: { rows: { entry: { label: string } }[] } | undefined) => s?.rows.map((r) => r.entry.label) ?? [];

describe('hidden and dormant', () => {
  it('no session record is NOT hidden; a throwaway name unused for 30 days is', () => {
    expect(hiddenReason(proj('o', 'openmarket-app', null), undefined, NOW)).toBeNull();
    expect(hiddenReason(proj('o', 'ppt-epic-145', null), undefined, NOW)).toBe('throwaway name');
    expect(hiddenReason(proj('o', 'test-x', 2), undefined, NOW)).toBeNull();
    expect(hiddenReason(proj('o', 'contest-app', null), undefined, NOW)).toBeNull();
  });
  it('hide wins; keep, pin and a person’s group protect a throwaway name', () => {
    const p = proj('o', 'tmp-x', null);
    expect(hiddenReason(proj('o', 'a'), { owner: 'o', repo: 'a', pinned: false, vis: 'hide', grp: null }, NOW)).toBe('hidden by you');
    for (const k of [{ vis: 'keep' as const }, { pinned: true }, { grp: 'g' }]) {
      expect(hiddenReason(p, { owner: 'o', repo: 'tmp-x', pinned: false, vis: null, grp: null, ...k }, NOW)).toBeNull();
    }
  });
  it('dormant: no record, or nothing for 90 days', () => {
    expect(isDormant(proj('o', 'a', null), NOW)).toBe(true);
    expect(isDormant(proj('o', 'a', 91), NOW)).toBe(true);
    expect(isDormant(proj('o', 'a', 10), NOW)).toBe(false);
  });
});

describe('groupProjects', () => {
  it('longest shared prefix of ≥3, single token joins, others fall to owner buckets', () => {
    const rows = [
      proj('F', 'sales-twins-app'), proj('F', 'sales-twins-mobile', null), proj('F', 'sales-twins-revonaut-fixes', null),
      proj('F', 'stw-fix2', null),
      proj('p', 'openmarket-ai'), proj('p', 'openmarket-docs'), proj('p', 'openmarket-app', null), proj('p', 'openmarket', null),
      proj('p', 'dwh', null),
      proj('x', 'gods-eye-view', null),
    ];
    const g = groupProjects(rows, new Map());
    const name = (r: ProjectTreeRow) => g.get(r.project.id)?.name;
    expect(rows.slice(0, 3).map(name)).toEqual(['sales-twins', 'sales-twins', 'sales-twins']);
    expect(name(rows[3])).toBe('More from F');
    expect(rows.slice(4, 8).map(name)).toEqual(['openmarket', 'openmarket', 'openmarket', 'openmarket']);
    expect(name(rows[8])).toBe('More from p');
    expect(name(rows[9])).toBe(FORKS);
    expect(g.get(rows[0].project.id)?.sub).toBe('sales-twins-* · F');
  });
  it('a person’s group wins; a hide never moves a neighbour (clusters use every project)', () => {
    const rows = [proj('o', 'ab-1'), proj('o', 'ab-2'), proj('o', 'ab-3', null), proj('o', 'zz')];
    const g = groupProjects(rows, picks({ owner: 'o', repo: 'ab-3', vis: 'hide' }, { owner: 'o', repo: 'zz', grp: 'Mine' }));
    expect(g.get(rows[0].project.id)?.name).toBe('ab');
    expect(g.get(rows[3].project.id)).toMatchObject({ name: 'Mine', sub: 'your group' });
  });
});

describe('buildSections', () => {
  it('Pinned, Suggested (context first, chips, ⌘ numbers), groups list every member, Hidden folded', () => {
    const fleet = proj('me', 'claude-fleet', 0.01);
    const backend = proj('p', 'papayapos-backend', 5);
    const docs = proj('p', 'openmarket-docs', 0.5);
    const om1 = proj('p', 'openmarket-ai', 23);
    const om2 = proj('p', 'openmarket-app', null);
    const epic = proj('me', 'ppt-epic-145', null);
    const sys = proj('fleet', 'operator', 0, true);
    const e = entriesOf([fleet, backend, docs, om1, om2, epic, sys], picks({ owner: 'me', repo: 'claude-fleet', pinned: true }), NOW);
    const ctx: Ctx = { selectedProjectId: backend.project.id, preferredHost: null, sessions: [] };
    const s = buildSections(e, ctx, {}, NOW);
    const by = (k: string) => s.find((x) => x.key === k);
    expect(labels(by('pinned'))).toEqual(['claude-fleet']);
    expect(by('pinned')?.rows[0].kbd).toBe('⌘1');
    expect(labels(by('suggested'))[0]).toBe('papayapos-backend');
    expect(by('suggested')?.rows[0]).toMatchObject({ chip: 'current session', kbd: '⌘2' });
    const om = s.find((x) => x.label === 'openmarket');
    expect(labels(om)).toEqual(['openmarket-ai', 'openmarket-docs', 'openmarket-app']); // every member; dormant last
    expect(om?.openByDefault).toBe(true);
    expect(by('hidden')).toMatchObject({ foldable: true, openByDefault: false });
    expect(labels(by('hidden'))).toEqual(['ppt-epic-145']);
    expect(s.flatMap((x) => x.rows).some((r) => r.entry.project.project.system)).toBe(false);
  });
  it('a group with no session in 30 days starts folded', () => {
    const e = entriesOf([proj('o', 'a-1', 40), proj('o', 'a-2', null), proj('o', 'a-3', null)], new Map(), NOW);
    expect(buildSections(e, noCtx, {}, NOW).find((x) => x.label === 'a')?.openByDefault).toBe(false);
  });
  it('preferred host: projects with a session there, chip "on host"', () => {
    const a = proj('o', 'alpha', 3);
    const b = proj('o', 'beta', 2);
    const ctx: Ctx = {
      selectedProjectId: null,
      preferredHost: 'mefistos',
      sessions: [
        { project_id: a.project.id, host_alias: 'mefistos', last_activity_at: NOW - 10 },
        { project_id: b.project.id, host_alias: 'mac', last_activity_at: NOW },
      ],
    };
    const s = buildSections(entriesOf([a, b], new Map(), NOW), ctx, {}, NOW);
    expect(s.find((x) => x.key === 'suggested')?.rows[0]).toMatchObject({ chip: 'on mefistos' });
  });
  it('frecency outranks plain recency in Suggested; at most 7', () => {
    const many = Array.from({ length: 10 }, (_, i) => proj('o', `r${i}`, i + 1));
    const s = buildSections(entriesOf(many, new Map(), NOW), noCtx, { 'o/r9': { score: 5, at: NOW } }, NOW);
    const sugg = labels(s.find((x) => x.key === 'suggested'));
    expect(sugg[0]).toBe('r9');
    expect(sugg).toHaveLength(7);
  });
});

describe('searchEntries', () => {
  it('hidden is found but ranked last and tagged', () => {
    const used = proj('o', 'shop-api', 1);
    const hidden = proj('o', 'shop-app', 1);
    const e = entriesOf([hidden, used], picks({ owner: 'o', repo: 'shop-app', vis: 'hide' }), NOW);
    const r = searchEntries(e, 'shop', noCtx, {}, NOW);
    expect(r.map((x) => x.entry.label)).toEqual(['shop-api', 'shop-app']);
    expect(r[1].meta).toBe('hidden · hidden by you');
  });
  it('matches the group name too', () => {
    const e = entriesOf([proj('p', 'openmarket-ai'), proj('p', 'openmarket-docs'), proj('p', 'openmarket-app'), proj('p', 'zzz')], new Map(), NOW);
    expect(searchEntries(e, 'openm', noCtx, {}, NOW).map((x) => x.entry.label).sort()).toEqual(['openmarket-ai', 'openmarket-app', 'openmarket-docs']);
  });
});
