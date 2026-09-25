// Work graph M5.4: Settings → Work → Organisations, standalone and on a
// paired desktop (read-only, "configure on the hub").
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import OrgSettings from './OrgSettings.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { orgs, type OrgDetail } from './orgs';
import { hosts } from './hosts';
import { trackers } from './trackers';
import { toasts } from './toasts';
import { get } from 'svelte/store';

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

const remote: HubStatus = {
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  client_mode: null,
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

function route(extra: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in extra) return extra[cmd];
    if (cmd === 'list_orgs') return [acme];
    if (cmd === 'org_suggestions') return [{ name: 'beta', owner: 'beta', sessions: 2, reason: '2 live sessions under beta/*' }];
    return null;
  });
  return inv;
}

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  orgs.set([]);
  hosts.set([]);
  trackers.set([]);
  toasts.set([]);
});

describe('Settings → Organisations', () => {
  it('lists orgs with their rules, hosts and trackers as chips', async () => {
    route();
    render(OrgSettings);
    await waitFor(() => expect(screen.getAllByTestId('org-row')).toHaveLength(1));
    const chips = screen.getAllByTestId('org-rule').map((c) => c.textContent?.replace('×', '').trim());
    expect(chips).toEqual(['acme/*', 'path: /w/acme']);
    expect(screen.getByTestId('org-host').textContent).toContain('hetzner-a');
    expect(screen.getByTestId('org-tracker').textContent).toContain('Acme Jira');
  });

  it('standalone: a suggestion is one click — the org, then its rule', async () => {
    const inv = route({ add_org: { id: 9, name: 'beta', created_at: 1 }, add_org_rule: { id: 1, org_id: 9, owner: 'beta' } });
    render(OrgSettings);
    const btn = await screen.findByTestId('org-suggestion');
    expect(btn.textContent).toContain('Create org beta from beta/*');
    await fireEvent.click(btn);
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'add_org_rule')).toBe(true));
    const add = inv.mock.calls.find((c) => c[0] === 'add_org')![1] as { args: { name: string } };
    expect(add.args.name).toBe('beta');
    const rule = inv.mock.calls.find((c) => c[0] === 'add_org_rule')![1] as { args: { org_id: number; owner: string } };
    expect(rule.args).toEqual({ org_id: 9, owner: 'beta' });
  });

  it('standalone: isolation is a toggle with a plain warning', async () => {
    const inv = route({ update_org: { ...acme, isolate_sessions: true } });
    render(OrgSettings);
    const box = (await screen.findByTestId('org-isolate')) as HTMLInputElement;
    await fireEvent.click(box);
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'update_org')).toBe(true));
    const upd = inv.mock.calls.find((c) => c[0] === 'update_org')![1] as { args: { org_id: number; isolate_sessions: boolean } };
    expect(upd.args).toEqual({ org_id: 1, isolate_sessions: true });
  });

  it('standalone: per-org auto-tidy is on / off / inherit (work graph M7)', async () => {
    const inv = route({ update_org: { ...acme, auto_tidy: true } });
    render(OrgSettings);
    const sel = (await screen.findByTestId('org-auto-tidy')) as HTMLSelectElement;
    expect(sel.value).toBe('inherit');
    await fireEvent.change(sel, { target: { value: 'on' } });
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'update_org')).toBe(true));
    const upd = inv.mock.calls.find((c) => c[0] === 'update_org')![1] as { args: unknown };
    expect(upd.args).toEqual({ org_id: 1, auto_tidy: 'on' });
  });

  it('standalone: adds an org by name, adds an owner/repo rule, removes a rule and the org', async () => {
    const inv = route({ add_org: { id: 9, name: 'Company B', created_at: 1 } });
    render(OrgSettings);
    await waitFor(() => expect(screen.getAllByTestId('org-row')).toHaveLength(1));
    const add = screen.getByTestId('org-add') as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('org-add-name'), { target: { value: ' Company B ' } });
    expect(add.disabled).toBe(false);
    await fireEvent.click(add);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('add_org', {
        args: { name: 'Company B', color: '#3b82f6', isolate_sessions: false },
      }),
    );
    // The list is re-read and the name box cleared for the next one.
    await waitFor(() => expect((screen.getByTestId('org-add-name') as HTMLInputElement).value).toBe(''));
    const listed = inv.mock.calls.filter((c) => c[0] === 'list_orgs').length;
    expect(listed).toBeGreaterThanOrEqual(2);

    await fireEvent.input(screen.getByTestId('org-rule-input'), { target: { value: 'acme/api' } });
    await fireEvent.click(screen.getByTestId('org-rule-add'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('add_org_rule', { args: { org_id: 1, owner: 'acme', repo: 'api' } }),
    );
    await waitFor(() => expect((screen.getByTestId('org-rule-input') as HTMLInputElement).value).toBe(''));

    await fireEvent.click(screen.getByLabelText('Remove rule acme/*'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('remove_org_rule', { args: { rule_id: 3 } }));
    await fireEvent.click(screen.getByLabelText('Take hetzner-a out of Company A'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('assign_host_org', { args: { host_alias: 'hetzner-a', org_id: null } }),
    );
    await fireEvent.click(screen.getByTestId('org-remove'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('remove_org', { args: { org_id: 1 } }));
  });

  it('standalone: a failed admin command is a toast and the list is still re-read', async () => {
    const inv = route();
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'remove_org') throw { code: 'E_INVALID', message: 'org 1 still has hosts' };
      if (cmd === 'list_orgs') return [acme];
      if (cmd === 'org_suggestions') return [];
      return null;
    });
    render(OrgSettings);
    await waitFor(() => expect(screen.getAllByTestId('org-row')).toHaveLength(1));
    const before = inv.mock.calls.filter((c) => c[0] === 'list_orgs').length;
    await fireEvent.click(screen.getByTestId('org-remove'));
    await waitFor(() =>
      expect(get(toasts).map((t) => t.message)).toEqual([
        expect.stringMatching(/^Remove org failed: org 1 still has hosts/),
      ]),
    );
    await waitFor(() => expect(inv.mock.calls.filter((c) => c[0] === 'list_orgs').length).toBe(before + 1));
    expect(screen.getAllByTestId('org-row')).toHaveLength(1);
  });

  it('on a paired desktop it is read-only and names the hub CLI', async () => {
    hubStatus.set(remote);
    route();
    render(OrgSettings);
    await waitFor(() => expect(screen.getAllByTestId('org-row')).toHaveLength(1));
    expect(screen.queryByTestId('org-add-form')).toBeNull();
    expect(screen.queryByTestId('org-isolate')).toBeNull();
    expect(screen.queryByTestId('org-auto-tidy')).toBeNull();
    expect(screen.queryByTestId('org-remove')).toBeNull();
    expect(screen.queryByTestId('org-suggestions')).toBeNull();
    expect(screen.getByTestId('org-remote').textContent).toContain('fleet-hub org add');
  });
});
