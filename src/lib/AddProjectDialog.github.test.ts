// Add project's From GitHub source as the AddProject board draws it
// (redesign 6.11): the four sources in the board's order with From GitHub
// first, an owner to list, ticked rows, "already in fleet", and one verb
// that adds every ticked repository.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import AddProjectDialog from './AddProjectDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE } from './hub';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { projects, type ProjectTreeRow } from './projects';
import { orgs, type OrgDetail } from './orgs';
import { trackers, type TrackerRow } from './trackers';
import { toasts } from './toasts';
import { get } from 'svelte/store';

const mockedInvoke = invoke as ReturnType<typeof vi.fn>;

type Handler = (args: any) => unknown;

function route(handlers: Record<string, Handler>) {
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    const h = handlers[cmd];
    return h ? h(args) : null;
  });
}

function calls(cmd: string): any[] {
  return mockedInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1]);
}

async function flush() {
  for (let i = 0; i < 8; i++) await tick();
}

function treeRow(id: number, owner: string, repo: string, system = false): ProjectTreeRow {
  return {
    project: { id, owner, repo, base_path: `/p/${owner}/${repo}`, last_session_at: null, adopted: false, system },
    worktrees: [],
  };
}

const repo = (name_with_owner: string, extra: Record<string, unknown> = {}) => ({
  name_with_owner,
  description: null,
  is_private: false,
  updated_at: null,
  ...extra,
});

function mount() {
  const onCreated = vi.fn();
  const onCancel = vi.fn();
  render(AddProjectDialog, { props: { onCreated, onCancel } });
  return { onCreated, onCancel };
}

function rowNamed(name: string): HTMLElement {
  return screen.getAllByTestId('gh-repo-row').find((r) => r.dataset.key === name)!;
}

function tickRow(name: string) {
  return fireEvent.click(rowNamed(name).querySelector('input[type="checkbox"]')!);
}

beforeEach(() => {
  mockedInvoke.mockReset();
  hubStatus.set({ ...STANDALONE });
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  ]);
  fleetSettings.set({ ...SETTING_DEFAULTS });
  projects.set([]);
  orgs.set([]);
  trackers.set([]);
  toasts.set([]);
  localStorage.clear();
});

describe('Add project, as on the AddProject board', () => {
  it('opens on From GitHub, with the four sources in the board order and its lead sentence', async () => {
    route({ list_github_repos: () => [] });
    mount();
    await flush();
    const labels = Array.from(document.querySelectorAll('[data-testid^="add-mode-"]')).map((b) => b.textContent?.trim());
    expect(labels).toEqual(['From GitHub', 'Clone a URL', 'Existing folder', 'New repo']);
    expect(screen.getByTestId('add-mode-github').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('add-lead').textContent?.trim()).toBe(
      'Bring a repository into the fleet so sessions can start in it.',
    );
    // Listed at once, for the host login's own repositories: no owner sent.
    expect(calls('list_github_repos')).toEqual([{ args: { host_alias: 'local' } }]);
  });

  it('still opens on Clone a URL when the switcher hands it a URL', async () => {
    route({});
    render(AddProjectDialog, { props: { onCreated: vi.fn(), onCancel: vi.fn(), initialCloneUrl: 'acme/w' } });
    await flush();
    expect(screen.getByTestId('add-mode-clone').getAttribute('aria-pressed')).toBe('true');
    expect(calls('list_github_repos')).toHaveLength(0);
  });

  it("a row shows the repository's language, age and privacy", async () => {
    const updated = new Date(Date.now() - 2 * 3600 * 1000).toISOString();
    route({
      list_github_repos: () => [repo('papaya-pos/receipts', { language: 'TypeScript', updated_at: updated, is_private: true })],
    });
    mount();
    await flush();
    expect(rowNamed('papaya-pos/receipts').textContent).toContain('TypeScript · updated 2h ago · private');
  });

  it('lists another owner on Enter, offering the owners already in the fleet', async () => {
    projects.set([treeRow(1, 'papaya-pos', 'api'), treeRow(2, '32bit', 'site'), treeRow(3, 'fleet', 'operator', true)]);
    route({
      list_github_repos: (a) =>
        a.args.owner === 'papaya-pos' ? [repo('papaya-pos/receipts'), repo('papaya-pos/api')] : [repo('me/mine')],
    });
    mount();
    await flush();
    const known = Array.from(document.querySelectorAll('#gh-owner-known option')).map((o) => (o as HTMLOptionElement).value);
    expect(known).toEqual(['32bit', 'papaya-pos']);
    const owner = screen.getByTestId('gh-owner');
    await fireEvent.input(owner, { target: { value: 'papaya-pos' } });
    await fireEvent.keyDown(owner, { key: 'Enter' });
    await flush();
    expect(calls('list_github_repos').at(-1)).toEqual({ args: { host_alias: 'local', owner: 'papaya-pos' } });
    expect(screen.getAllByTestId('gh-repo-row').map((r) => r.dataset.key)).toEqual(['papaya-pos/receipts', 'papaya-pos/api']);
    // Enter in the owner field lists; it never adds.
    expect(calls('add_project')).toHaveLength(0);
  });

  it("an older hub that ignores the owner still shows only that owner's repositories", async () => {
    route({ list_github_repos: () => [repo('me/mine'), repo('acme/tool')] });
    mount();
    await flush();
    const owner = screen.getByTestId('gh-owner');
    await fireEvent.input(owner, { target: { value: 'acme' } });
    await fireEvent.change(owner);
    await flush();
    expect(screen.getAllByTestId('gh-repo-row').map((r) => r.dataset.key)).toEqual(['acme/tool']);
  });

  it('a repository already in the fleet says so and cannot be ticked', async () => {
    projects.set([treeRow(1, 'Papaya-POS', 'Legacy')]);
    route({ list_github_repos: () => [repo('papaya-pos/legacy'), repo('papaya-pos/web')] });
    mount();
    await flush();
    const legacy = rowNamed('papaya-pos/legacy');
    expect(legacy.querySelector('[data-testid="gh-in-fleet"]')?.textContent).toBe('already in fleet');
    expect((legacy.querySelector('input') as HTMLInputElement).disabled).toBe(true);
    expect(rowNamed('papaya-pos/web').querySelector('[data-testid="gh-in-fleet"]')).toBeNull();
  });

  it('adds every ticked repository on the chosen host, then hands back the first', async () => {
    const rows: Record<string, ProjectTreeRow> = {
      'acme/api': treeRow(11, 'acme', 'api'),
      'acme/web': treeRow(12, 'acme', 'web'),
    };
    route({
      list_github_repos: () => [repo('acme/api'), repo('acme/web'), repo('acme/docs')],
      add_project: (a) => rows[a.args.source.url],
    });
    const { onCreated } = mount();
    await flush();
    await fireEvent.click(
      Array.from(document.querySelectorAll<HTMLButtonElement>('.host-pick')).find((b) => b.dataset.alias === 'mefistos')!,
    );
    await flush();
    await tickRow('acme/api');
    await tickRow('acme/web');
    await tick();
    expect(screen.getByTestId('add-create').textContent).toBe('Add 2 projects');
    expect(screen.getByTestId('add-summary').textContent).toBe('2 repos · on mefistos');
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('add_project').map((c) => [c.args.host_alias, c.args.source])).toEqual([
      ['mefistos', { kind: 'clone', url: 'acme/api' }],
      ['mefistos', { kind: 'clone', url: 'acme/web' }],
    ]);
    expect(onCreated).toHaveBeenCalledTimes(1);
    expect(onCreated).toHaveBeenCalledWith(rows['acme/api'], 'mefistos');
    // The second is kept in the New session picker too, quietly.
    expect(calls('set_project_pick').map((c) => [c.args.owner, c.args.repo, c.args.vis])).toEqual([
      ['acme', 'web', 'keep'],
    ]);
  });

  it('a failure stops the rest, says what was added, and Add carries on with what is left', async () => {
    let fail = true;
    route({
      list_github_repos: () => [repo('acme/api'), repo('acme/web')],
      add_project: (a) => {
        if (a.args.source.url === 'acme/web' && fail) throw { code: 'E_GIT', message: 'clone failed: no access' };
        return treeRow(a.args.source.url === 'acme/api' ? 11 : 12, 'acme', a.args.source.url.split('/')[1]);
      },
    });
    const { onCreated } = mount();
    await flush();
    await tickRow('acme/api');
    await tickRow('acme/web');
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(screen.getByTestId('add-error').textContent).toBe('Added acme/api. clone failed: no access');
    expect(onCreated).not.toHaveBeenCalled();
    // acme/api is in the fleet now; only acme/web is still ticked.
    expect(rowNamed('acme/api').querySelector('[data-testid="gh-in-fleet"]')).not.toBeNull();
    expect(screen.getByTestId('add-create').textContent).toBe('Add project');
    fail = false;
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('add_project').map((c) => c.args.source.url)).toEqual(['acme/api', 'acme/web', 'acme/web']);
    expect(onCreated).toHaveBeenCalledWith(treeRow(12, 'acme', 'web'), 'local');
  });

  it('on a hub client asked for Existing folder, opens on From GitHub instead', async () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://fleet.example.com' });
    route({ list_github_repos: () => [] });
    render(AddProjectDialog, { props: { onCreated: vi.fn(), onCancel: vi.fn(), initialMode: 'folder' } });
    await flush();
    expect(screen.queryByTestId('add-mode-folder')).toBeNull();
    expect(screen.getByTestId('add-mode-github').getAttribute('aria-pressed')).toBe('true');
  });
});

describe('Add project: Also clone on, Organisation and Tracker (6.11 follow-ups)', () => {
  const papaya = {
    id: 3,
    name: 'Papaya',
    created_at: 0,
    rules: [{ id: 1, org_id: 3, owner: 'papaya-pos' }],
    hosts: [],
    trackers: [],
  } as OrgDetail;
  const acmeOrg = { id: 4, name: 'Acme', created_at: 0, rules: [], hosts: [], trackers: [] } as OrgDetail;
  const issues: TrackerRow = {
    id: 9,
    provider: 'github',
    name: 'Acme issues',
    site_url: 'https://github.com',
    state: 'ok',
    created_at: 0,
    settings: { repos: ['acme/web'] },
  };

  beforeEach(() => {
    hosts.update((h) => [
      ...h,
      { alias: 'trn', ssh_alias: 'trn', reachable: true, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
  });

  it('clones on the chosen host, then onto each other host the person ticked', async () => {
    const row = treeRow(11, 'acme', 'api');
    route({ list_github_repos: () => [repo('acme/api')], add_project: () => row });
    const { onCreated } = mount();
    await flush();
    await fireEvent.click(
      Array.from(document.querySelectorAll<HTMLButtonElement>('.host-pick')).find((b) => b.dataset.alias === 'mefistos')!,
    );
    await flush();
    // `local` and the chosen host are not offered again.
    const also = screen.getAllByTestId('add-also-host').map((b) => b.dataset.alias);
    expect(also).toEqual(['trn']);
    await fireEvent.click(screen.getAllByTestId('add-also-host')[0]);
    await tickRow('acme/api');
    await tick();
    expect(screen.getByTestId('add-summary').textContent).toBe('1 repo · on mefistos, trn');
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('add_project').map((c) => [c.args.host_alias, c.args.source])).toEqual([
      ['mefistos', { kind: 'clone', url: 'acme/api' }],
      ['trn', { kind: 'clone', url: 'acme/api', existing: true }],
    ]);
    expect(onCreated).toHaveBeenCalledWith(row, 'mefistos');
  });

  it("an older hub that can't add a second host still adds the project, and says so", async () => {
    const row = treeRow(11, 'acme', 'api');
    route({
      add_project: (a) => {
        if (a.args.source.existing) throw { code: 'E_EXISTS', message: 'acme/api is already a fleet project' };
        return row;
      },
    });
    const onCreated = vi.fn();
    render(AddProjectDialog, { props: { onCreated, onCancel: vi.fn(), initialCloneUrl: 'acme/api' } });
    await flush();
    await fireEvent.click(screen.getAllByTestId('add-also-host').find((b) => b.dataset.alias === 'trn')!);
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(onCreated).toHaveBeenCalledWith(row, 'local');
    expect(get(toasts).map((t) => t.message)).toEqual([
      "acme/api was not cloned on trn: this hub can't add a second host yet. It is cloned there when a session starts.",
    ]);
  });

  it('shows the organisation a repository already belongs to and writes no rule for it', async () => {
    orgs.set([papaya, acmeOrg]);
    route({ list_github_repos: () => [repo('papaya-pos/receipts')], add_project: () => treeRow(12, 'papaya-pos', 'receipts') });
    mount();
    await flush();
    await tickRow('papaya-pos/receipts');
    await tick();
    expect((screen.getByTestId('add-org') as HTMLSelectElement).value).toBe('3');
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('add_org_rule')).toHaveLength(0);
  });

  it('puts the new project in the organisation picked, by its repository', async () => {
    orgs.set([papaya, acmeOrg]);
    route({ add_project: () => treeRow(13, 'acme', 'api'), add_org_rule: () => ({ id: 2, org_id: 4, owner: 'acme', repo: 'api' }) });
    render(AddProjectDialog, { props: { onCreated: vi.fn(), onCancel: vi.fn(), initialCloneUrl: 'acme/api' } });
    await flush();
    expect((screen.getByTestId('add-org') as HTMLSelectElement).value).toBe('');
    await fireEvent.change(screen.getByTestId('add-org'), { target: { value: '4' } });
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('add_org_rule')).toEqual([{ args: { org_id: 4, owner: 'acme', repo: 'api' } }]);
  });

  it('points a GitHub tracker at the new repository', async () => {
    trackers.set([issues, { ...issues, id: 10, provider: 'jira', name: 'PD board' }]);
    route({ add_project: () => treeRow(13, 'acme', 'api'), update_tracker: () => issues });
    render(AddProjectDialog, { props: { onCreated: vi.fn(), onCancel: vi.fn(), initialCloneUrl: 'acme/api' } });
    await flush();
    // Only GitHub trackers can be pointed at a repository today.
    const options = Array.from((screen.getByTestId('add-tracker') as HTMLSelectElement).options).map((o) => o.textContent);
    expect(options).toEqual(['None', 'Acme issues']);
    await fireEvent.change(screen.getByTestId('add-tracker'), { target: { value: '9' } });
    await tick();
    expect(screen.getByTestId('add-tracker-note').textContent).toBe('Its issues will sync from Acme issues.');
    await fireEvent.click(screen.getByTestId('add-create'));
    await flush();
    expect(calls('update_tracker')).toEqual([{ args: { tracker_id: 9, settings: { repos: ['acme/web', 'acme/api'] } } }]);
  });
});
