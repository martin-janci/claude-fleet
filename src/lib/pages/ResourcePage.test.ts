// Declarative pages P4: the Organisations page — a master_detail page over
// the `org` resource — carries everything the hand-written OrgSettings did
// (work graph M5.4, M7, D31), through the same commands with the same
// arguments.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { hosts } from '../hosts';
import { host } from '../hosts_fixture';
import { trackers, type TrackerRow } from '../trackers';
import { toasts } from '../toasts';
import { orgs, type OrgDetail } from '../orgs';
import { bundle, openRecordTab } from './testing';
import type { Page } from './pages';

const page = bundle.pages.find((p) => p.id === 'settings.orgs') as Page;
const orgResource = bundle.resources.find((r) => r.id === 'org')!;

const acme: OrgDetail = {
  id: 1,
  name: 'Company A',
  color: '#ff0000',
  isolate_sessions: false,
  created_at: 1,
  rules: [
    { id: 3, org_id: 1, owner: 'acme' },
    { id: 4, org_id: 1, path_prefix: '/w/acme' },
  ],
  hosts: ['hetzner-a'],
  trackers: [{ id: 2, name: 'Acme Jira' }],
};

const tracker = (id: number, name: string, org_id: number | null): TrackerRow =>
  ({ id, provider: 'jira_cloud', name, site_url: `https://${id}.example.com`, state: 'ok', created_at: 1, org_id }) as TrackerRow;

function route(extra: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in extra) {
      const v = extra[cmd];
      if (v instanceof Error) throw v;
      return v;
    }
    if (cmd === 'list_orgs') return [acme];
    if (cmd === 'org_suggestions') return [{ name: 'beta', owner: 'beta', sessions: 2, reason: '2 live sessions under beta/*' }];
    if (cmd === 'list_trackers') return get(trackers);
    return null;
  });
  return inv;
}

function show(readonly = false, reason: string | null = null) {
  render(ResourcePage, { props: { page, resource: orgResource, readonly, reason } });
}

const calls = (inv: ReturnType<typeof vi.fn>, cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd);
const argsOf = (inv: ReturnType<typeof vi.fn>, cmd: string) =>
  (calls(inv, cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  orgs.set([]);
  hosts.set([]);
  trackers.set([]);
  toasts.set([]);
});

describe('Organisations (master_detail over the org resource)', () => {
  it('shows the overview: counts, catalogs and the devices bound to it', async () => {
    const overview: OrgDetail = {
      ...acme,
      session_count: 3,
      needs_you: 1,
      catalogs: ['acme-assets'],
      devices: [
        { name: 'phone', mode: 'readonly', trusted: true },
        { name: 'laptop', mode: 'full', trusted: false },
      ],
    };
    route({ list_orgs: [overview] });
    show();
    await waitFor(() => expect(screen.getByTestId('value-session_count').textContent).toBe('3'));
    expect(screen.getByTestId('value-needs_you').textContent).toBe('1');
    expect(screen.getByTestId('item-catalogs').textContent).toContain('acme-assets');
    await openRecordTab('Devices');
    expect(screen.getAllByTestId('item-devices').map((c) => c.textContent?.replace('×', '').trim())).toEqual([
      'phone · watch only, trusted',
      'laptop',
    ]);
    // A catalog is shown, never changed here; a device is unbound from it
    // (org administration phase B).
    expect(screen.queryByTestId('item-remove-catalogs')).toBeNull();
    expect(screen.getAllByTestId('item-remove-devices')).toHaveLength(2);
  });

  it('reads an older hub as zero, and leaves out the lists it does not carry', async () => {
    route();
    show();
    await waitFor(() => expect(screen.getByTestId('value-session_count').textContent).toBe('0'));
    // `devices` is absent for anyone but the operator, and an older hub has
    // no `catalogs`: neither is shown as an empty list.
    expect(screen.queryByTestId('record-field-devices')).toBeNull();
    expect(screen.queryByTestId('record-field-catalogs')).toBeNull();
    expect(screen.getByTestId('record-field-hosts')).toBeTruthy();
  });

  it('lists orgs, and shows one with its rules, hosts and trackers as chips', async () => {
    route({ list_orgs: [acme, { ...acme, id: 2, name: 'Company B', isolate_sessions: true, auto_tidy: false }] });
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(2));
    const rowB = screen.getAllByTestId('resource-row')[1];
    expect(within(rowB).getAllByTestId('resource-badge').map((b) => b.textContent)).toEqual([
      'isolates sessions',
      'auto-tidy off',
    ]);
    const rules = screen.getAllByTestId('item-rules').map((c) => c.textContent?.replace('×', '').trim());
    expect(rules).toEqual(['acme/*', 'path: /w/acme']);
    expect(screen.getByTestId('item-hosts').textContent).toContain('hetzner-a');
    expect(screen.getByTestId('item-trackers').textContent).toContain('Acme Jira');
  });

  it('a suggestion is one click: the org, then its rule', async () => {
    const inv = route({ add_org: { id: 9, name: 'beta', created_at: 1 }, add_org_rule: { id: 1, org_id: 9, owner: 'beta' } });
    show();
    const btn = await screen.findByTestId('org-suggestion');
    expect(btn.textContent).toContain('Create org beta from beta/*');
    await fireEvent.click(btn);
    await waitFor(() => expect(calls(inv, 'add_org_rule')).toHaveLength(1));
    expect(argsOf(inv, 'add_org').name).toBe('beta');
    expect(argsOf(inv, 'add_org_rule')).toEqual({ org_id: 9, owner: 'beta' });
  });

  it('a tracker suggestion is one click: the org, then its tracker', async () => {
    const inv = route({
      org_suggestions: [{ name: 'Beta', tracker_id: 5, sessions: 0, reason: 'a tracker named Beta' }],
      add_org: { id: 9, name: 'Beta', created_at: 1 },
    });
    show();
    const btn = await screen.findByTestId('org-suggestion');
    expect(btn.textContent).toBe('Create org Beta with its tracker');
    await fireEvent.click(btn);
    await waitFor(() => expect(argsOf(inv, 'assign_tracker_org')).toEqual({ tracker_id: 5, org_id: 9 }));
    expect(calls(inv, 'add_org_rule')).toHaveLength(0);
  });

  it('edits are a draft: Apply sends only what changed, Discard drops it', async () => {
    const inv = route();
    show();
    const name = (await screen.findByTestId('edit-name')) as HTMLInputElement;
    await fireEvent.input(name, { target: { value: 'Company A2' } });
    expect(screen.getByTestId('record-apply-bar').textContent).toContain('1 unsaved change');
    await fireEvent.click(screen.getByTestId('record-discard'));
    expect(screen.queryByTestId('record-apply-bar')).toBeNull();
    expect((screen.getByTestId('edit-name') as HTMLInputElement).value).toBe('Company A');

    await fireEvent.input(screen.getByTestId('edit-color'), { target: { value: '#00ff00' } });
    await fireEvent.click(screen.getByTestId('record-apply'));
    await waitFor(() => expect(calls(inv, 'update_org')).toHaveLength(1));
    // The colour is the only field sent: no accidental isolation or auto-tidy change.
    expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, color: '#00ff00' });
    // …and the list is re-read.
    await waitFor(() => expect(calls(inv, 'list_orgs').length).toBeGreaterThanOrEqual(2));
  });

  it('isolation asks first, with a plain warning', async () => {
    const inv = route();
    show();
    await openRecordTab('Settings');
    await fireEvent.click(await screen.findByTestId('edit-isolate_sessions'));
    await fireEvent.click(screen.getByTestId('record-apply'));
    const dialog = await screen.findByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('break a controller');
    await fireEvent.click(within(dialog).getByText('Cancel'));
    expect(calls(inv, 'update_org')).toHaveLength(0);
    await fireEvent.click(screen.getByTestId('record-apply'));
    await fireEvent.click(await screen.findByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, isolate_sessions: true }));
  });

  it('per-org auto-tidy is inherit / on / off (work graph M7)', async () => {
    const inv = route();
    show();
    await openRecordTab('Settings');
    expect(((await screen.findByTestId('edit-auto_tidy-inherit')) as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByTestId('edit-auto_tidy-on'));
    await fireEvent.click(screen.getByTestId('record-apply'));
    await waitFor(() => expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, auto_tidy: 'on' }));
  });

  it('an org opts in to Jev, off by default, and is asked first (D31)', async () => {
    const inv = route();
    show();
    await openRecordTab('Settings');
    const box = (await screen.findByTestId('edit-jev_allowed')) as HTMLInputElement;
    expect(box.checked).toBe(false);
    await fireEvent.click(box);
    await fireEvent.click(screen.getByTestId('record-apply'));
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('sent to TypeSafe');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, jev: 'on' }));
  });

  it('what bound devices see of unassigned work is on by default, and off says so (D31)', async () => {
    const inv = route();
    show();
    await openRecordTab('Settings');
    const box = (await screen.findByTestId('edit-bound_sees_unassigned')) as HTMLInputElement;
    expect(box.checked).toBe(true);
    await fireEvent.click(box);
    await fireEvent.click(screen.getByTestId('record-apply'));
    await waitFor(() => expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, bound_sees_unassigned: false }));
  });

  it('adds an org by name, adds repository and path rules, removes a rule, a host and the org', async () => {
    const inv = route({ add_org: { id: 9, name: 'Company B', created_at: 1 } });
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    const run = screen.getByTestId('run-org.add') as HTMLButtonElement;
    expect(run.disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('param-org.add-name'), { target: { value: ' Company B ' } });
    await fireEvent.click(run);
    await waitFor(() => expect(argsOf(inv, 'add_org')).toEqual({ name: 'Company B', color: null, isolate_sessions: false }));

    // M15 G2.10: one rule form, "Match by" and its value.
    const by = screen.getByTestId('param-org.add_rule-match_by') as HTMLSelectElement;
    expect(Array.from(by.options).map((o) => o.textContent)).toEqual(['Repository', 'Path', 'Host', 'Owner']);
    await fireEvent.input(screen.getByTestId('param-org.add_rule-value'), { target: { value: 'acme/api' } });
    await fireEvent.click(screen.getByTestId('run-org.add_rule'));
    await waitFor(() =>
      expect(argsOf(inv, 'add_org_rule')).toEqual({ org_id: 1, match_by: 'repository', value: 'acme/api' }),
    );
    await fireEvent.change(screen.getByTestId('param-org.add_rule-match_by'), { target: { value: 'path' } });
    await fireEvent.input(screen.getByTestId('param-org.add_rule-value'), { target: { value: '/w/b' } });
    await fireEvent.click(screen.getByTestId('run-org.add_rule'));
    await waitFor(() => expect(argsOf(inv, 'add_org_rule')).toEqual({ org_id: 1, match_by: 'path', value: '/w/b' }));

    await fireEvent.click(screen.getByLabelText('Remove rule: acme/*'));
    await waitFor(() => expect(argsOf(inv, 'remove_org_rule')).toEqual({ rule_id: 3 }));
    await fireEvent.click(screen.getByLabelText('Take the host out: hetzner-a'));
    await waitFor(() => expect(argsOf(inv, 'assign_host_org')).toEqual({ host_alias: 'hetzner-a', org_id: null }));

    await fireEvent.click(screen.getByTestId('record-delete'));
    expect((await screen.findByTestId('record-remove')).textContent).toContain('become unassigned');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'remove_org')).toEqual({ org_id: 1 }));
  });

  it('M15 G2.10: a new org can be visible only to its members from the start', async () => {
    const inv = route({ add_org: { id: 9, name: 'Company B', created_at: 1 } });
    show();
    await fireEvent.click(await screen.findByTestId('resource-add'));
    await fireEvent.input(screen.getByTestId('param-org.add-name'), { target: { value: 'Company B' } });
    const box = screen.getByTestId('param-org.add-isolate_sessions') as HTMLInputElement;
    expect(box.checked).toBe(false);
    expect(box.closest('label')?.textContent).toContain('visible only to its members');
    await fireEvent.click(box);
    await fireEvent.click(screen.getByTestId('run-org.add'));
    await waitFor(() => expect(argsOf(inv, 'add_org')).toEqual({ name: 'Company B', color: null, isolate_sessions: true }));
  });

  it('M15 G2.10: the rule form says what the rule would match and move while it is typed', async () => {
    const inv = route({
      org_rule_preview: { matches: 14, moving: 3, from: [], kept: 0, sentence: 'Matches 14 sessions now; 3 of them are in Personal and would move.' },
    });
    show();
    await openRecordTab('Overview');
    await fireEvent.change(await screen.findByTestId('param-org.add_rule-match_by'), { target: { value: 'owner' } });
    await fireEvent.input(screen.getByTestId('param-org.add_rule-value'), { target: { value: 'beta' } });
    expect((await screen.findByTestId('preview-org.add_rule', {}, { timeout: 2000 })).textContent).toBe(
      'Matches 14 sessions now; 3 of them are in Personal and would move.',
    );
    expect(argsOf(inv, 'org_rule_preview')).toEqual({ org_id: 1, match_by: 'owner', value: 'beta' });
    expect(calls(inv, 'add_org_rule')).toHaveLength(0);
  });

  it('M15 G2.10: the project catalog lists entries and adds and removes one', async () => {
    const inv = route({
      list_orgs: [{ ...acme, projects: [{ id: 7, org_id: 1, name: 'api', remote: 'git@github.com:acme/api.git', hosts: ['hetzner-a'], created_at: 1 }] }],
    });
    show();
    await openRecordTab('Overview');
    const chips = await screen.findAllByTestId('item-projects');
    expect(chips[0].textContent).toContain('api · git@github.com:acme/api.git · on hetzner-a');
    await fireEvent.input(screen.getByTestId('param-org.add_project-name'), { target: { value: 'web' } });
    await fireEvent.input(screen.getByTestId('param-org.add_project-hosts'), { target: { value: 'hetzner-a' } });
    await fireEvent.click(screen.getByTestId('run-org.add_project'));
    await waitFor(() =>
      expect(argsOf(inv, 'add_org_project')).toEqual({ org_id: 1, name: 'web', remote: null, path: null, hosts: 'hetzner-a' }),
    );
    await fireEvent.click(screen.getByLabelText('Remove the project: api · git@github.com:acme/api.git · on hetzner-a'));
    await waitFor(() => expect(argsOf(inv, 'remove_org_project')).toEqual({ project_id: 7 }));
  });

  it('M15 G2.10: "members see only their own sessions" is on by default; off asks first', async () => {
    const inv = route();
    show();
    await openRecordTab('Settings');
    const box = (await screen.findByTestId('edit-members_own_sessions_only')) as HTMLInputElement;
    expect(box.checked).toBe(true);
    await fireEvent.click(box);
    await fireEvent.click(screen.getByTestId('record-apply'));
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('watch each other');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'update_org')).toEqual({ org_id: 1, members_own_sessions_only: false }));
  });

  it('M15 G2.10: adding a member offers the people the hub knows, and a new name still goes', async () => {
    const inv = route({
      list_orgs: [{ ...acme, members: [] }],
      list_people: [
        { id: 1, name: 'martin', owner: true, created_at: 1, devices: [] },
        { id: 2, name: 'jane', display_name: 'Jane D', owner: false, created_at: 1, devices: [] },
        { id: 3, name: 'gone', owner: false, created_at: 1, disabled_at: 5, devices: [] },
      ],
    });
    show();
    await openRecordTab('Members');
    const list = await screen.findByTestId('suggest-org.set_member-person');
    await waitFor(() =>
      expect(Array.from(list.querySelectorAll('option')).map((o) => o.getAttribute('value'))).toEqual(['martin', 'jane']),
    );
    await fireEvent.input(screen.getByTestId('param-org.set_member-person'), { target: { value: 'newbie' } });
    await fireEvent.click(screen.getByTestId('run-org.set_member'));
    await waitFor(() => expect(argsOf(inv, 'set_org_member')).toEqual({ org_id: 1, person: 'newbie', role: 'member' }));
  });

  it('M15 G4.7: the Sharing tab lists every share, filters, narrows and revokes; a private session is not named', async () => {
    const inv = route({
      list_orgs: [
        {
          ...acme,
          shares: [
            { id: 7, session: 'bob-1', session_id: 11, owner: 'bob', shared_with: 'jane', level: 'drive', since: 1 },
            { id: 8, owner: 'bob', shared_with: 'eve', level: 'answer', since: 1 },
            { id: 9, session: 'ann-1', session_id: 12, owner: 'ann', shared_with: 'everyone in Company A', level: 'watch', since: 1 },
          ],
        },
      ],
    });
    show();
    await openRecordTab('Sharing');
    const rows = await screen.findAllByTestId('item-shares');
    expect(rows.map((r) => r.querySelector('td')!.textContent!.trim())).toEqual(['bob-1 · bob', 'a private session · bob', 'ann-1 · ann']);
    // A watch share has nothing to narrow.
    expect(within(rows[2]).queryByTestId('share-narrow')).toBeNull();
    await fireEvent.change(screen.getByTestId('shares-level'), { target: { value: 'answer' } });
    expect(screen.getAllByTestId('item-shares')).toHaveLength(1);
    await fireEvent.change(screen.getByTestId('shares-level'), { target: { value: '' } });
    await fireEvent.change(screen.getByTestId('shares-owner'), { target: { value: 'ann' } });
    expect(screen.getAllByTestId('item-shares')).toHaveLength(1);
    await fireEvent.change(screen.getByTestId('shares-owner'), { target: { value: '' } });

    await fireEvent.click(within(screen.getAllByTestId('item-shares')[0]).getByTestId('share-narrow'));
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('no longer answer or drive');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'narrow_org_share')).toEqual({ org_id: 1, grant_id: 7 }));
    await fireEvent.click(within(screen.getAllByTestId('item-shares')[1]).getByTestId('share-revoke'));
    await fireEvent.click(await screen.findByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'revoke_org_share')).toEqual({ org_id: 1, grant_id: 8 }));
  });

  it('M15 G4.7: the team panel, an inline role, a removed member whose shares are taken back, and accounts', async () => {
    const inv = route({
      list_orgs: [
        {
          ...acme,
          members: [
            { person_id: 2, name: 'jane', role: 'admin' },
            { person_id: 3, name: 'bob', role: 'member' },
          ],
          team: [
            { person_id: 2, name: 'jane', role: 'admin', sessions: [], private: 0 },
            { person_id: 3, name: 'bob', role: 'member', sessions: [{ id: 11, name: 'bob-1', state: 'needs you' }], private: 2 },
          ],
          removed_members: [
            { person_id: 4, name: 'carl', removed_at: 1_000_000 - 3 * 86_400, grants: 2 },
            { person_id: 5, name: 'dana', removed_at: 1_000_000 - 9 * 86_400, grants: 0 },
          ],
          accounts: [{ name: 'ops@acme.dev', seat_tier: 'max', hosts: ['hetzner-a'] }],
        },
      ],
    });
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 * 1000);
    show();
    await openRecordTab('Sharing');
    const team = await screen.findAllByTestId('item-team');
    expect(team[0].textContent).toContain('nothing live');
    expect(within(team[1]).getByTestId('team-session').textContent).toBe('bob-1 needs you');
    expect(within(team[1]).getByTestId('team-rest').textContent).toBe('2 private');

    await openRecordTab('Members');

    await fireEvent.change(screen.getByTestId('member-role-3'), { target: { value: 'viewer' } });
    await waitFor(() => expect(argsOf(inv, 'set_org_member')).toEqual({ org_id: 1, person: 'bob', role: 'viewer' }));

    const gone = screen.getAllByTestId('item-removed_members');
    expect(gone.map((g) => g.textContent!.replace(/\s+/g, ' ').trim())).toEqual([
      'carl removed 3 d ago · holds 2 shares Take back their shares',
      'dana removed 9 d ago · holds no share',
    ]);
    await fireEvent.click(within(gone[0]).getByTestId('item-action-org.revoke_former_grants'));
    await fireEvent.click(await screen.findByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'revoke_org_member_grants')).toEqual({ org_id: 1, person_id: 4 }));

    await openRecordTab('Overview');
    expect((await screen.findByTestId('item-accounts')).textContent).toBe('ops@acme.dev (max) · on hetzner-a');
    vi.restoreAllMocks();
  });

  it('a failed command is a toast, and the list is still re-read', async () => {
    const inv = route({ remove_org: Object.assign(new Error('x'), { code: 'E_INVALID', message: 'org 1 still has hosts' }) });
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(1));
    const before = calls(inv, 'list_orgs').length;
    await fireEvent.click(screen.getByTestId('record-delete'));
    await fireEvent.click(await screen.findByTestId('record-confirm'));
    await waitFor(() =>
      expect(get(toasts).map((t) => t.message)).toEqual([expect.stringMatching(/^Remove organisation failed: org 1 still has hosts/)]),
    );
    await waitFor(() => expect(calls(inv, 'list_orgs').length).toBeGreaterThan(before));
  });

  it('the host select offers only hosts outside the org', async () => {
    hosts.set([host('hetzner-a', { org_id: 1 }), host('hetzner-b'), host('local', { org_id: 2 })]);
    const inv = route();
    show();
    const sel = (await screen.findByTestId('param-org.assign_host-host')) as HTMLSelectElement;
    expect(Array.from(sel.options, (o) => o.value)).toEqual(['', 'hetzner-b', 'local']);
    expect((screen.getByTestId('run-org.assign_host') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.change(sel, { target: { value: 'hetzner-b' } });
    await fireEvent.click(screen.getByTestId('run-org.assign_host'));
    await waitFor(() => expect(argsOf(inv, 'assign_host_org')).toEqual({ host_alias: 'hetzner-b', org_id: 1 }));
  });

  it('the tracker select sends a numeric id and offers only trackers outside the org', async () => {
    trackers.set([tracker(2, 'Acme Jira', 1), tracker(5, 'Beta Linear', null), tracker(6, 'Other Jira', 3)]);
    const inv = route();
    show();
    const sel = (await screen.findByTestId('param-org.assign_tracker-tracker')) as HTMLSelectElement;
    await waitFor(() => expect(Array.from(sel.options, (o) => o.textContent)).toEqual(['Tracker…', 'Beta Linear', 'Other Jira']));
    await fireEvent.change(sel, { target: { value: '5' } });
    await fireEvent.click(screen.getByTestId('run-org.assign_tracker'));
    await waitFor(() => expect(argsOf(inv, 'assign_tracker_org')).toEqual({ tracker_id: 5, org_id: 1 }));
    expect(typeof argsOf(inv, 'assign_tracker_org').tracker_id).toBe('number');
  });

  it('read-only on a paired desktop: the hub\'s orgs, no controls, and where to change them', async () => {
    const inv = route();
    show(true, 'Organisations belong to the hub. On the hub: fleet-hub org add <name>.');
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(1));
    expect(screen.getByTestId('resource-readonly').textContent).toContain('fleet-hub org add');
    for (const id of ['resource-add', 'record-delete', 'edit-name', 'edit-isolate_sessions', 'org-suggestions']) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    expect(screen.queryByLabelText('Remove rule: acme/*')).toBeNull();
    await openRecordTab('Settings');
    expect(screen.getByTestId('value-isolate_sessions').textContent).toBe('Off');
    expect(calls(inv, 'org_suggestions')).toHaveLength(0);
  });
});

describe('removing an organisation (G1.4, the destructive confirm)', () => {
  it('says what it holds; a small org asks with the red verb alone', async () => {
    const inv = route();
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    await fireEvent.click(screen.getByTestId('record-delete'));
    const dialog = await screen.findByTestId('record-remove');
    expect(dialog.textContent).toContain('Remove organisation "Company A"?');
    expect(dialog.textContent).toContain('Its rules go with it');
    expect(dialog.textContent).toContain('It holds 2 rules · 1 host · 1 tracker.');
    expect(screen.queryByTestId('destructive-typed-name')).toBeNull();
    const go = screen.getByTestId('record-confirm');
    expect(go.textContent).toBe('Remove organisation');
    expect(go.classList.contains('btn--danger')).toBe(true);
    await fireEvent.click(go);
    await waitFor(() => expect(argsOf(inv, 'remove_org')).toEqual({ org_id: 1 }));
  });

  it('a large org asks for its name before Remove organisation turns on', async () => {
    const big: OrgDetail = { ...acme, hosts: ['hetzner-a', 'hetzner-b', 'mac'] };
    const inv = route({ list_orgs: [big] });
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    await fireEvent.click(screen.getByTestId('record-delete'));
    expect((await screen.findByTestId('record-remove')).textContent).toContain('Type the organisation name to confirm');
    const go = screen.getByTestId('record-confirm') as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    await fireEvent.click(go);
    expect(calls(inv, 'remove_org')).toHaveLength(0);
    await fireEvent.input(screen.getByTestId('destructive-typed-name'), { target: { value: 'Company A' } });
    await fireEvent.click(go);
    await waitFor(() => expect(argsOf(inv, 'remove_org')).toEqual({ org_id: 1 }));
  });

  it('Cancel removes nothing', async () => {
    const inv = route();
    show();
    await fireEvent.click((await screen.findAllByTestId('resource-row'))[0]);
    await fireEvent.click(screen.getByTestId('record-delete'));
    await fireEvent.click(within(await screen.findByTestId('record-remove')).getByTestId('sheet-cancel'));
    await waitFor(() => expect(screen.queryByTestId('record-remove')).toBeNull());
    expect(calls(inv, 'remove_org')).toHaveLength(0);
  });
});
