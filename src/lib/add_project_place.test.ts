import { describe, it, expect, vi } from 'vitest';
import { coveringOrgId, placeProject, reposWith, ruleFor } from './add_project_place';
import type { OrgDetail } from './orgs';
import type { TrackerRow } from './trackers';

const org = (id: number, rules: OrgDetail['rules']): OrgDetail =>
  ({ id, name: `org${id}`, created_at: 0, rules, hosts: [], trackers: [] }) as OrgDetail;

const gh = (repos: string[] | undefined): TrackerRow => ({
  id: 5,
  provider: 'github',
  name: 'GitHub issues',
  site_url: 'https://github.com',
  state: 'ok',
  created_at: 0,
  settings: repos ? { repos } : {},
});

const project = (owner: string, repo: string, base_path = `/p/${owner}/${repo}`) => ({
  id: 1,
  owner,
  repo,
  base_path,
  last_session_at: null,
  adopted: owner === 'local',
  system: false,
});

describe('coveringOrgId', () => {
  const orgs = [
    org(1, [{ id: 1, org_id: 1, owner: 'papaya-pos' }]),
    org(2, [{ id: 2, org_id: 2, owner: 'Papaya-POS', repo: 'legacy' }]),
    org(3, [{ id: 3, org_id: 3, owner: 'papaya-pos', path_prefix: '/w' }]),
  ];
  it('a rule naming the repository beats one naming its owner, case aside', () => {
    expect(coveringOrgId(orgs, 'papaya-pos', 'LEGACY')).toBe(2);
    expect(coveringOrgId(orgs, 'papaya-pos', 'api')).toBe(1);
  });
  it('a path or host rule never covers a repository by name, and an unknown owner has none', () => {
    expect(coveringOrgId([orgs[2]], 'papaya-pos', 'api')).toBeNull();
    expect(coveringOrgId(orgs, 'acme', 'api')).toBeNull();
  });
});

describe('ruleFor', () => {
  it('names the repository, or the path of an adopted folder', () => {
    expect(ruleFor(4, project('acme', 'api'))).toEqual({ org_id: 4, owner: 'acme', repo: 'api' });
    expect(ruleFor(4, project('local', 'thing', '/Users/me/thing'))).toEqual({ org_id: 4, path_prefix: '/Users/me/thing' });
  });
});

describe('reposWith', () => {
  it('appends the repository to a tracker that lists some', () => {
    expect(reposWith(gh(['acme/web']), 'Acme', 'API')).toEqual(['acme/web', 'acme/api']);
  });
  it('changes nothing when it is listed, or when the tracker lists none (it covers everything)', () => {
    expect(reposWith(gh(['acme/api']), 'acme', 'api')).toBeNull();
    expect(reposWith(gh([]), 'acme', 'api')).toBeNull();
    expect(reposWith(gh(undefined), 'acme', 'api')).toBeNull();
  });
});

describe('placeProject', () => {
  it('writes the org rule and the tracker repos, and says what was refused', async () => {
    const addOrgRule = vi.fn(async () => ({ ok: false as const, error: { code: 'E_FORBIDDEN', message: 'not a trusted device' } }));
    const updateTracker = vi.fn(async () => ({ ok: true as const, value: gh(['acme/web', 'acme/api']) }));
    const problems = await placeProject(project('acme', 'api'), { orgId: 7, tracker: gh(['acme/web']) }, [], {
      addOrgRule,
      updateTracker,
    });
    expect(addOrgRule).toHaveBeenCalledWith({ org_id: 7, owner: 'acme', repo: 'api' });
    expect(updateTracker).toHaveBeenCalledWith(5, { settings: { repos: ['acme/web', 'acme/api'] } });
    expect(problems).toEqual(['acme/api was added but not put in the organisation: not a trusted device']);
  });

  it('writes nothing when the org already covers it and no tracker is asked', async () => {
    const addOrgRule = vi.fn();
    const updateTracker = vi.fn();
    const problems = await placeProject(
      project('acme', 'api'),
      { orgId: 1, tracker: null },
      [org(1, [{ id: 1, org_id: 1, owner: 'acme' }])],
      { addOrgRule, updateTracker },
    );
    expect(problems).toEqual([]);
    expect(addOrgRule).not.toHaveBeenCalled();
    expect(updateTracker).not.toHaveBeenCalled();
  });
});
