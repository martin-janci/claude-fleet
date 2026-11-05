// Keep an account off a project (docs/accounts.md): the account's list of
// forbidden projects, Allow on each and the picker that adds one.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import AccountProjects from './AccountProjects.svelte';
import { accountsForbiddenFor, type AccountRow } from './accounts';
import { projects, type ProjectTreeRow } from './projects';
import { orgs, projectOrgId, type OrgDetail } from './orgs';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const project = (id: number, owner: string, repo: string, system = false): ProjectTreeRow => ({
  project: { id, owner, repo, base_path: `/src/${repo}`, last_session_at: null, adopted: false, system },
  worktrees: [],
});

const account = (forbidden?: number[]): AccountRow => ({
  uuid: 'acc-1',
  email: 'ada@example.com',
  display_name: null,
  organization_name: null,
  organization_uuid: null,
  seat_tier: null,
  last_seen_at: null,
  nickname: null,
  has_extra_usage: false,
  ...(forbidden ? { forbidden_project_ids: forbidden } : {}),
});

const sets = () => invoke.mock.calls.filter((c) => c[0] === 'set_account_projects').map((c) => c[1]);

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, a: any) =>
    cmd === 'set_account_projects' ? { ...account(), forbidden_project_ids: a.args.forbidden_project_ids } : null,
  );
  projects.set([project(1, 'acme', 'api'), project(2, 'acme', 'web'), project(3, 'fleet', 'operator', true)]);
  orgs.set([]);
});

describe('AccountProjects', () => {
  it('lists the forbidden projects and allows one again', async () => {
    render(AccountProjects, { props: { account: account([2]) } });
    expect(screen.getByTestId('account-projects-list').textContent).toContain('acme/web');
    await fireEvent.click(screen.getByTestId('account-projects-allow'));
    expect(sets()).toEqual([{ args: { uuid: 'acc-1', forbidden_project_ids: [] } }]);
  });

  it('adds a project from the picker, never fleet’s own', async () => {
    render(AccountProjects, { props: { account: account([2]) } });
    const select = screen.getByTestId('account-projects-add') as HTMLSelectElement;
    const offered = Array.from(select.options).map((o) => o.textContent);
    expect(offered).toContain('acme/api');
    expect(offered).not.toContain('acme/web');
    expect(offered).not.toContain('fleet/operator');
    await fireEvent.change(select, { target: { value: '1' } });
    expect(sets()).toEqual([{ args: { uuid: 'acc-1', forbidden_project_ids: [2, 1] } }]);
  });

  it('an older hub sends no list: nothing is forbidden', () => {
    render(AccountProjects, { props: { account: account() } });
    expect(screen.queryByTestId('account-projects-list')).toBeNull();
    expect(screen.getByTestId('account-projects-add')).toBeTruthy();
  });

  it('is read-only while the hub cannot take the change', () => {
    render(AccountProjects, { props: { account: account([2]), blocked: 'The hub is not reachable' } });
    expect((screen.getByTestId('account-projects-add') as HTMLSelectElement).disabled).toBe(true);
    expect((screen.getByTestId('account-projects-allow') as HTMLButtonElement).disabled).toBe(true);
  });
});

const org = (id: number, name: string, rules: OrgDetail['rules']): OrgDetail =>
  ({ id, name, created_at: 0, rules, hosts: [], trackers: [] }) as OrgDetail;

describe('AccountProjects, whole orgs', () => {
  it('forbids a whole org and allows it again, leaving the projects alone', async () => {
    orgs.set([org(5, 'SalesTwins', []), org(6, 'Acme', [])]);
    const { unmount } = render(AccountProjects, { props: { account: { ...account([2]), forbidden_org_ids: [5] } } });
    expect(screen.getByTestId('account-orgs-list').textContent).toContain('All of SalesTwins');
    const select = screen.getByTestId('account-orgs-add') as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.textContent)).toEqual(['Forbid for a whole org…', 'Acme']);
    await fireEvent.change(select, { target: { value: '6' } });
    await fireEvent.click(screen.getByTestId('account-orgs-allow'));
    expect(sets()).toEqual([
      { args: { uuid: 'acc-1', forbidden_org_ids: [5, 6] } },
      { args: { uuid: 'acc-1', forbidden_org_ids: [] } },
    ]);
    unmount();
  });

  it('offers no org picker without orgs', () => {
    render(AccountProjects, { props: { account: account() } });
    expect(screen.queryByTestId('account-orgs-add')).toBeNull();
  });
});

describe('projectOrgId', () => {
  const p = { owner: 'acme', repo: 'api', base_path: '/srv/acme/api' };
  it('takes the most specific rule on the project, never a host rule', () => {
    const owner = org(1, 'Owner', [{ id: 1, org_id: 1, owner: 'ACME' }]);
    const repo = org(2, 'Repo', [{ id: 2, org_id: 2, owner: 'acme', repo: 'api' }]);
    const path = org(3, 'Path', [{ id: 3, org_id: 3, path_prefix: '/srv/acme' }]);
    const host = org(4, 'Host', [{ id: 4, org_id: 4, path_prefix: '/srv/acme/api', host_alias: 'mac' }]);
    expect(projectOrgId(p, [owner])).toBe(1);
    expect(projectOrgId(p, [owner, repo])).toBe(2);
    expect(projectOrgId(p, [owner, repo, path, host])).toBe(3);
    expect(projectOrgId(p, [host])).toBeNull();
    expect(projectOrgId({ ...p, base_path: '/srv/acme-other' }, [path])).toBeNull();
    expect(projectOrgId({ ...p, owner: 'local' }, [owner])).toBeNull();
  });
});

describe('accountsForbiddenFor', () => {
  it('names the accounts a project may not run on', () => {
    const list = [account([1, 2]), { ...account([2]), uuid: 'acc-2' }, { ...account(), uuid: 'acc-3' }];
    expect(accountsForbiddenFor(1, list)).toEqual(['acc-1']);
    expect(accountsForbiddenFor(2, list)).toEqual(['acc-1', 'acc-2']);
    expect(accountsForbiddenFor(null, list)).toEqual([]);
    // Through the project's org.
    const byOrg = [{ ...account(), uuid: 'acc-4', forbidden_org_ids: [9] }];
    expect(accountsForbiddenFor(1, byOrg, 9)).toEqual(['acc-4']);
    expect(accountsForbiddenFor(1, byOrg, 8)).toEqual([]);
    expect(accountsForbiddenFor(1, byOrg)).toEqual([]);
  });
});
