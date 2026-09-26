// The other half of a hub client's honesty: a control that cannot work is
// disabled with the reason BEFORE the click, rather than failing at it.
//
// The backend already refuses each of these with `E_LOCAL_ONLY` and a message
// naming where the operation does work (`backend.local_only`). That is the
// safety net. What these tests pin is the part a person actually experiences.
import { render, screen, waitFor, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import HostDetail from './HostDetail.svelte';
import AssetsPanel from './AssetsPanel.svelte';
import OnboardingCard from './OnboardingCard.svelte';
import HostsView from './HostsView.svelte';
import Sidebar from './Sidebar.svelte';
import SessionDetails from './SessionDetails.svelte';
import FileList from './FileList.svelte';
import RemoteToolbar from './RemoteToolbar.svelte';
import BranchList from './BranchList.svelte';
import CommitGraph from './CommitGraph.svelte';
import { sharedWith } from './hosts_view';
import {
  ADMIN,
  GMAIL,
  NOW,
  fleetHosts,
  fleetSessions,
  fleetUsage,
  host,
  session as sessionFixture,
} from './hosts_fixture';
import { hosts } from './hosts';
import { catalog } from './assets';
import { projects, type ProjectTreeRow } from './projects';
import { sessions as sessionsStore, type SessionRow } from './sessions';

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
  hubConnection.set({ state: 'standalone' });
  hosts.set([]);
  projects.set([]);
  sessionsStore.set([]);
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
  projects.set([]);
  sessionsStore.set([]);
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

  // The list and the scan route (the hub's list_assets / scan_assets), so
  // the panel is a read-only overview of the hub's catalog rather than a
  // dead end.
  const hubListing = {
    head: 'abcdef1234567890', loaded_at: 1, problems: [],
    unmanaged: [{ host_alias: 'nas', harness: 'claude', kind: 'skill', name: 'extra', state: 'unmanaged', catalog_hash: null, host_hash: null, scanned_at: 1, managed: false }],
    assets: [
      { kind: 'skill', name: 'worktree', version: '1.2', description: 'd', tags: [], hosts: [
        { host_alias: 'nas', harness: 'claude', state: 'in_sync' },
        { host_alias: 'mac', harness: 'claude', state: 'drifted' },
      ] },
    ],
  };

  it('shows the hub’s catalog read-only: where each asset is and in what state', async () => {
    catalog.set(null);
    hubStatus.set(remote);
    inv().mockImplementation(async (cmd: string) => (cmd === 'catalog_list_assets' ? hubListing : null));
    render(AssetsPanel, { props: { visible: true } });
    const row = await screen.findByTestId('asset-row-skill-worktree');
    expect(row.tagName).toBe('DIV');
    expect(row.textContent).toContain('1 in sync');
    expect(row.textContent).toContain('1 drifted');
    expect(row.getAttribute('title')).toContain('mac: drifted');
    expect(screen.getByTestId('assets-head').textContent).toContain('abcdef1');
    // Unmanaged rows are listed, with nothing to import them into.
    expect(screen.getByTestId('unmanaged-row-nas-claude-skill-extra').textContent).not.toContain('Import');
    expect(screen.getByTestId('assets-remote-note').textContent).toContain('Read-only');
  });

  it('scans the hosts through the hub and re-reads the overview', async () => {
    catalog.set(null);
    hubStatus.set(remote);
    inv().mockImplementation(async (cmd: string) =>
      cmd === 'catalog_list_assets'
        ? hubListing
        : cmd === 'assets_scan_hosts'
          ? [{ host: 'nas', status: 'scanned', detail: null, rows: 3 }]
          : null,
    );
    render(AssetsPanel, { props: { visible: true } });
    await screen.findByTestId('asset-row-skill-worktree');
    await fireEvent.click(screen.getByTestId('assets-scan'));
    expect((await screen.findByTestId('assets-scan-result')).textContent).toContain('nas: scanned');
    expect(inv().mock.calls.filter((c) => c[0] === 'catalog_list_assets')).toHaveLength(2);
  });

  it('says so when the hub has no catalog yet', async () => {
    catalog.set(null);
    hubStatus.set(remote);
    inv().mockImplementation(async (cmd: string) => {
      if (cmd === 'catalog_list_assets') throw { code: 'E_CATALOG_NOT_CONFIGURED', message: 'catalog not loaded' };
      return null;
    });
    render(AssetsPanel, { props: { visible: true } });
    expect((await screen.findByTestId('assets-hub-failed')).textContent).toContain('no asset catalog yet');
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

// #146: `new_session` and `repair_session` (explicit, the only mode
// the desktop's buttons ever send) now map one-to-one onto their hub tools
// and route, so — unlike the fleet-administration and this-machine-only
// controls above — these two stay enabled on a hub client.
describe('new_session and repair_session route now, so their buttons stay enabled', () => {
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
    model: null, context_tokens: null, context_window: null, context_source: null,
    context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
  };

  it('+ New session is enabled on a hub client', async () => {
    hubStatus.set(remote);
    render(Sidebar, { props: {} as never });
    expect(await screen.findByTestId('new-session-footer')).not.toBeDisabled();
  });

  it('Repair workspace is enabled on a hub client', async () => {
    hubStatus.set(remote);
    render(SessionDetails, { props: { session } });
    expect(await screen.findByTestId('repair-from-details')).not.toBeDisabled();
  });
});

// #147: the git-write panel. None of the ten git-write commands
// (checkout/branch/stage/commit/fetch/pull/push) has a hub tool
// (`commands/mutate.rs`) — a remote client must not stage or commit under a
// running agent. `FilesPanel` computes one reason (`hubBlock('repo_write',
// …)`) and fans it out to every control below; these are the components that
// actually render them.
describe('the git-write panel on a hub client', () => {
  const change = { path: 'src/lib.rs', status: 'modified' as const, staged: false, orig_path: null };
  const branch = {
    name: 'feature/x',
    isCurrent: false,
    isRemote: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tipHash: 'abc123',
  };
  const commit = {
    hash: 'abc123',
    shortHash: 'abc123',
    parents: [],
    refs: [],
    author: 'me',
    date: new Date().toISOString(),
    subject: 'a commit',
  };
  const REASON = 'no git-write tool';

  it('FileList disables staging and committing, with the reason', async () => {
    render(FileList, {
      props: {
        mode: 'changes',
        changes: [change],
        tree: null,
        loading: false,
        error: null,
        selectedPath: null,
        onSelect: vi.fn(),
        onStageToggle: vi.fn(),
        onCommit: vi.fn(),
        enableStaging: true,
        writeBlocked: REASON,
      },
    });
    const checkbox = document.querySelector('.stage') as HTMLInputElement;
    expect(checkbox).toBeDisabled();
    expect(checkbox.title).toBe(REASON);
    const commitBtn = screen.getByText(/Commit \d+ file/);
    expect(commitBtn).toBeDisabled();
  });

  it('FileList staging and committing are enabled with writeBlocked=null', () => {
    render(FileList, {
      props: {
        mode: 'changes',
        changes: [{ ...change, staged: true }],
        tree: null,
        loading: false,
        error: null,
        selectedPath: null,
        onSelect: vi.fn(),
        onStageToggle: vi.fn(),
        onCommit: vi.fn(),
        enableStaging: true,
        writeBlocked: null,
      },
    });
    expect(document.querySelector('.stage')).not.toBeDisabled();
  });

  it('RemoteToolbar disables Fetch/Pull/Push, with the reason', () => {
    render(RemoteToolbar, {
      props: { session: { id: 1 } as SessionRow, ondone: vi.fn(), writeBlocked: REASON },
    });
    for (const label of ['Fetch', 'Pull', 'Push']) {
      const btn = screen.getByText(label);
      expect(btn, label).toBeDisabled();
      expect(btn.title, label).toBe(REASON);
    }
  });

  it('BranchList disables Checkout/Delete/+ New branch, with the reason', () => {
    render(BranchList, {
      props: {
        branches: [branch],
        loading: false,
        error: null,
        onCheckout: vi.fn(),
        onDelete: vi.fn(),
        onNew: vi.fn(),
        writeBlocked: REASON,
      },
    });
    expect(screen.getByText('+ New branch')).toBeDisabled();
    expect(screen.getByText('Checkout')).toBeDisabled();
    expect(screen.getByText('Delete')).toBeDisabled();
  });

  it('CommitGraph disables its create-branch and checkout-commit actions, with the reason', () => {
    render(CommitGraph, {
      props: {
        commits: [commit],
        selected: null,
        onSelect: vi.fn(),
        onCreateBranch: vi.fn(),
        onCheckoutCommit: vi.fn(),
        writeBlocked: REASON,
      },
    });
    const buttons = screen.getAllByRole('button');
    // The two per-row action buttons (⎇ create branch, ⤓ checkout commit).
    const actionButtons = buttons.filter((b) => b.title === REASON);
    expect(actionButtons).toHaveLength(2);
    for (const b of actionButtons) expect(b).toBeDisabled();
  });
});

// #147: the two swept gaps that live in SessionDetails — the pre-flight
// safe-kill inspection and the one-step discard-and-kill, neither of which
// has a hub tool (`commands/sessions.rs`).
describe('safe remove and discard-kill on a hub client', () => {
  const session: SessionRow = sessionFixture('mefistos', 'dev-foo', {
    project_id: 3, claude_status: null, turn_seq: 0, last_stop_at: null,
  });

  it('Safe remove is disabled, with the reason', async () => {
    hubStatus.set(remote);
    render(SessionDetails, { props: { session } });
    const btn = await screen.findByTestId('safe-kill-from-details');
    expect(btn).toBeDisabled();
    expect((btn as HTMLButtonElement).title).toContain('fleet.example.com');
    // A disabled button fires no click in a real browser; jsdom does not
    // enforce that, so this pins the handler never having been reached the
    // way the rendered `disabled` attribute promises.
    expect(inv().mock.calls.some((c) => c[0] === 'inspect_safe_kill')).toBe(false);
  });

  it('standalone is untouched: Safe remove still opens the dialog', async () => {
    inv().mockImplementation(async (cmd: string) =>
      cmd === 'inspect_safe_kill'
        ? { safe_to_remove: true, has_worktree: true, branch: 'main', upstream: 'origin/main', unpushed_commits: 0, dirty_files: [] }
        : cmd === 'tunnel_status' ? [] : null,
    );
    render(SessionDetails, { props: { session } });
    const btn = await screen.findByTestId('safe-kill-from-details');
    expect(btn).not.toBeDisabled();
    await fireEvent.click(btn);
    await tick();
    expect(await screen.findByTestId('confirm-safe-kill-direct')).not.toBeDisabled();
  });

  it("an inactive agent's Remove from list is disabled, with the reason", async () => {
    hubStatus.set(remote);
    render(SessionDetails, { props: { session: { ...session, kind: 'bg', claude_status: 'stopped' } } });
    const btn = await screen.findByTestId('remove-from-list-details');
    expect(btn).toBeDisabled();
    expect((btn as HTMLButtonElement).title.toLowerCase()).toContain('kill');
  });
});

// #147: Add project and Purge project. Neither has a hub tool
// (`commands/projects.rs`, `commands/sessions.rs`): both act over this
// machine's SSH (and, for Add project, GitHub credentials).
describe('add and purge project on a hub client', () => {
  const project: ProjectTreeRow = {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 1, adopted: false, system: false },
    worktrees: [{ id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' }],
  };
  const projectSession: SessionRow = sessionFixture('local', 'dev-cf', {
    project_id: 1, worktree_id: 11, worktree_key: 'main',
    claude_status: null, turn_seq: 0, last_stop_at: null,
  });

  it('+ Add project… is disabled with the reason', async () => {
    hubStatus.set(remote);
    projects.set([project]);
    sessionsStore.set([projectSession]);
    render(Sidebar, { props: {} as never });
    await fireEvent.click(await screen.findByTestId('new-session-footer'));
    const btn = await screen.findByTestId('add-project-row');
    expect(btn).toBeDisabled();
    expect((btn as HTMLButtonElement).title).toContain('fleet.example.com');
  });

  it('standalone is untouched: + Add project… still opens the dialog', async () => {
    projects.set([project]);
    sessionsStore.set([projectSession]);
    render(Sidebar, { props: {} as never });
    await fireEvent.click(await screen.findByTestId('new-session-footer'));
    expect(await screen.findByTestId('add-project-row')).not.toBeDisabled();
  });

  it('Purge project is disabled with the reason', async () => {
    hubStatus.set(remote);
    projects.set([project]);
    sessionsStore.set([projectSession]);
    render(Sidebar, { props: {} as never });
    const btn = await screen.findByTestId('purge-project');
    expect(btn).toBeDisabled();
    expect((btn as HTMLButtonElement).title).toContain('fleet.example.com');
  });

  it('standalone is untouched: Purge project still opens the confirm dialog', async () => {
    projects.set([project]);
    sessionsStore.set([projectSession]);
    render(Sidebar, { props: {} as never });
    expect(await screen.findByTestId('purge-project')).not.toBeDisabled();
  });
});

// #147, requirement C: offline gating for routed mutations. `kill_session`
// stands in for the family (send prompt, kill/restart/rename/move,
// new_session, repair, task cancel, …), all driven by the one derived helper
// (`hubActionBlocked`).
describe('a routed mutation control while the hub connection is not up', () => {
  const session: SessionRow = sessionFixture('mefistos', 'dev-foo', {
    project_id: 3, claude_status: null, turn_seq: 0, last_stop_at: null,
  });

  it('Kill session is disabled while reconnecting, naming the hub as unreachable', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 3, reason: 'closed' });
    render(SessionDetails, { props: { session } });
    const btn = await screen.findByTestId('kill-from-details');
    expect(btn).toBeDisabled();
    expect((btn as HTMLButtonElement).title.toLowerCase()).toContain('unreachable');
  });

  it('Kill session is enabled once connected', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    render(SessionDetails, { props: { session } });
    expect(await screen.findByTestId('kill-from-details')).not.toBeDisabled();
  });

  it('standalone is untouched: Kill session is enabled whatever the (irrelevant) connection store holds', async () => {
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 3, reason: 'closed' });
    render(SessionDetails, { props: { session } });
    expect(await screen.findByTestId('kill-from-details')).not.toBeDisabled();
  });
});
