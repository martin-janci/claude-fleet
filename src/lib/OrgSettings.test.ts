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

  it('on a paired desktop it is read-only and names the hub CLI', async () => {
    hubStatus.set(remote);
    route();
    render(OrgSettings);
    await waitFor(() => expect(screen.getAllByTestId('org-row')).toHaveLength(1));
    expect(screen.queryByTestId('org-add-form')).toBeNull();
    expect(screen.queryByTestId('org-isolate')).toBeNull();
    expect(screen.queryByTestId('org-remove')).toBeNull();
    expect(screen.queryByTestId('org-suggestions')).toBeNull();
    expect(screen.getByTestId('org-remote').textContent).toContain('fleet-hub org add');
  });
});
