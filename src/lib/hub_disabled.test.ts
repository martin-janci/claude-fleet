// The other half of a hub client's honesty: a control that cannot work is
// disabled with the reason BEFORE the click, rather than failing at it.
//
// The backend already refuses each of these with `E_LOCAL_ONLY` and a message
// naming where the operation does work (`backend.local_only`). That is the
// safety net. What these tests pin is the part a person actually experiences.
import { render, screen, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import HostDetail from './HostDetail.svelte';
import AssetsPanel from './AssetsPanel.svelte';
import OnboardingCard from './OnboardingCard.svelte';
import HostsView from './HostsView.svelte';
import Sidebar from './Sidebar.svelte';
import SessionDetails from './SessionDetails.svelte';
import { sharedWith } from './hosts_view';
import { ADMIN, GMAIL, NOW, fleetHosts, fleetSessions, fleetUsage, host } from './hosts_fixture';
import { hosts } from './hosts';

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

const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  inv().mockReset();
  // `null` for everything except the one list the setup checklist iterates
  // unguarded: a standalone OnboardingCard (also mounted inside Sidebar) does
  // `tunnels.some(...)`, and the real command never answers null.
  inv().mockImplementation(async (cmd: string) => (cmd === 'tunnel_status' ? [] : null));
  hubStatus.set({ ...STANDALONE });
  hosts.set([]);
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
});

function mountHostDetail(alias: string) {
  const all = fleetHosts();
  const h = all.find((x) => x.alias === alias) ?? host(alias);
  const acct = h.account_uuid === ADMIN.uuid ? ADMIN : h.account_uuid === GMAIL.uuid ? GMAIL : null;
  render(HostDetail, {
    props: {
      host: h,
      account: acct,
      snapshot: h.account_uuid ? fleetUsage()[h.account_uuid] : null,
      sharedWith: sharedWith(h, all),
      hostSessions: fleetSessions().filter((s) => s.host_alias === alias),
      token: { host_alias: alias, mode: 'full', created_at: 1 },
      tokensLoaded: true,
      hook: { state: 'seen' as const, lastAt: NOW - 300 },
      attention: null,
      now: NOW,
      locale: 'en-GB',
      timeZone: 'UTC',
      editingNickname: false,
      oneditstart: vi.fn(),
      oneditdone: vi.fn(),
      onreprobe: vi.fn(),
      onrefreshusage: vi.fn(),
    },
  });
}

describe('fleet administration on a hub client', () => {
  // The plan's constraint, verbatim: the desktop "never calls a tool a client
  // is refused (provision_hosts, add_host, remove_host, hide_host, apply_sync,
  // set_secret, pair_client, list_clients, revoke_client). Those are disabled
  // in the UI with the reason."
  it('Hide, Remove and Rotate token are disabled, each saying why', async () => {
    hubStatus.set(remote);
    mountHostDetail('mefistos');
    await tick();
    for (const testid of ['detail-hide', 'detail-remove', 'detail-rotate']) {
      const btn = screen.getByTestId(testid) as HTMLButtonElement;
      expect(btn, testid).toBeDisabled();
      expect(btn.title, testid).toContain('fleet.example.com');
      expect(btn.title.toLowerCase(), testid).toContain('client');
    }
  });

  it('the per-host token mode is read-only, because those tokens are not this app’s', async () => {
    hubStatus.set(remote);
    mountHostDetail('mefistos');
    await tick();
    const select = screen.getByTestId('detail-token-mode') as HTMLSelectElement;
    expect(select).toBeDisabled();
    expect(select.title).toContain('fleet.example.com');
  });

  it('standalone is untouched: all three still work', async () => {
    mountHostDetail('mefistos');
    await tick();
    for (const testid of ['detail-hide', 'detail-remove', 'detail-rotate']) {
      expect(screen.getByTestId(testid), testid).not.toBeDisabled();
    }
    expect(screen.getByTestId('detail-token-mode')).not.toBeDisabled();
  });
});

describe('the asset catalog on a hub client', () => {
  // Requirement (a): `catalog_config` is one of the six the UI calls
  // unprompted. It now answers E_LOCAL_ONLY, so opening the Assets tab in
  // remote mode used to raise an error where the panel should be.
  it('does not ask for a catalog it cannot have', async () => {
    hubStatus.set(remote);
    render(AssetsPanel, { props: { visible: true } });
    await tick();
    await tick();
    expect(inv().mock.calls.some((c) => c[0] === 'catalog_config')).toBe(false);
    expect(inv().mock.calls.some((c) => c[0] === 'catalog_last_sync')).toBe(false);
  });

  it('shows the reason instead, and none of the controls that cannot work', async () => {
    hubStatus.set(remote);
    render(AssetsPanel, { props: { visible: true } });
    const note = await screen.findByTestId('assets-remote');
    expect(note.textContent).toContain('fleet.example.com');
    // Sync (apply_sync) and Secrets (set_secret) are two of the nine a client
    // is refused; they must not be sitting there waiting to fail.
    expect(screen.queryByTestId('assets-sync')).toBeNull();
    expect(screen.queryByTestId('assets-secrets')).toBeNull();
    expect(screen.queryByTestId('assets-setup')).toBeNull();
  });

  it('standalone is untouched: it still loads the catalog', async () => {
    render(AssetsPanel, { props: { visible: true } });
    await waitFor(() =>
      expect(inv().mock.calls.some((c) => c[0] === 'catalog_config')).toBe(true),
    );
    expect(screen.queryByTestId('assets-remote')).toBeNull();
  });
});

describe('the setup checklist on a hub client', () => {
  // Three of the six unprompted commands live here: check_local_prereqs,
  // tunnel_status and mcp_status, all fired from an $effect on mount.
  it('asks for none of the three local-only snapshots', async () => {
    hubStatus.set(remote);
    render(OnboardingCard, { props: { onaddhost: vi.fn(), onnewsession: vi.fn() } });
    await tick();
    await tick();
    for (const cmd of ['check_local_prereqs', 'tunnel_status', 'mcp_status']) {
      expect(inv().mock.calls.some((c) => c[0] === cmd), cmd).toBe(false);
    }
  });

  it('says the checklist is about running a fleet from this machine', async () => {
    hubStatus.set(remote);
    render(OnboardingCard, { props: { onaddhost: vi.fn(), onnewsession: vi.fn() } });
    const note = await screen.findByTestId('onboarding-remote');
    expect(note.textContent).toContain('fleet.example.com');
    expect(screen.queryByTestId('onboarding-steps')).toBeNull();
  });

  it('standalone is untouched: the three snapshots are still fetched', async () => {
    render(OnboardingCard, { props: { onaddhost: vi.fn(), onnewsession: vi.fn() } });
    await waitFor(() =>
      expect(inv().mock.calls.some((c) => c[0] === 'check_local_prereqs')).toBe(true),
    );
    expect(screen.queryByTestId('onboarding-remote')).toBeNull();
  });
});

describe('adding a host on a hub client', () => {
  it('+ Add host is disabled with the reason', async () => {
    hubStatus.set(remote);
    hosts.set(fleetHosts());
    render(HostsView, { props: { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    const btn = (await screen.findByTestId('hosts-add')) as HTMLButtonElement;
    expect(btn).toBeDisabled();
    expect(btn.title).toContain('fleet.example.com');
  });

  it('standalone is untouched', async () => {
    hosts.set(fleetHosts());
    render(HostsView, { props: { onClose: vi.fn(), onFilterSidebar: vi.fn(), onNewSession: vi.fn() } });
    expect(await screen.findByTestId('hosts-add')).not.toBeDisabled();
  });
});

// PARITY OR REFUSAL: these two REFUSE rather than route, because routing them
// would have succeeded while meaning something else. A refusal nobody can see
// coming is only half honest.
describe('the two commands that refuse rather than route', () => {
  const session = {
    id: 1, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 3, worktree_id: null,
    created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null,
    kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null,
    claude_session_id: null, claude_status: null, effort_level: null, pr_url: null,
    current_activity: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
    safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null,
    idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null,
    started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null,
    parent_session_id: null, tags: [],
  };

  it('New session says the label it would drop is why', async () => {
    hubStatus.set(remote);
    render(Sidebar, { props: {} as never });
    const btn = (await screen.findByTestId('new-session-footer')) as HTMLButtonElement;
    expect(btn).toBeDisabled();
    expect(btn.title).toMatch(/label|name/i);
    expect(btn.title).toContain('fleet.example.com');
  });

  it('Repair workspace says why it is not the same operation here', async () => {
    hubStatus.set(remote);
    render(SessionDetails, { props: { session } });
    const btn = (await screen.findByTestId('repair-from-details')) as HTMLButtonElement;
    expect(btn).toBeDisabled();
    expect(btn.title).toContain('fleet.example.com');
  });

  it('standalone is untouched: both still work', async () => {
    render(Sidebar, { props: {} as never });
    expect(await screen.findByTestId('new-session-footer')).not.toBeDisabled();
    render(SessionDetails, { props: { session } });
    expect(await screen.findByTestId('repair-from-details')).not.toBeDisabled();
  });
});
