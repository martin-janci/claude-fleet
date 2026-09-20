import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { tick } from 'svelte';
import HostDetail from './HostDetail.svelte';
import { sharedWith } from './hosts_view';
import { ADMIN, GMAIL, NOW, fleetHosts, fleetSessions, fleetUsage, host } from './hosts_fixture';

function mount(alias: string, over: Record<string, unknown> = {}) {
  const hosts = fleetHosts();
  const h = hosts.find((x) => x.alias === alias) ?? host(alias);
  const acct = h.account_uuid === ADMIN.uuid ? ADMIN : h.account_uuid === GMAIL.uuid ? GMAIL : null;
  const props = {
    host: h,
    account: acct,
    snapshot: h.account_uuid ? fleetUsage()[h.account_uuid] : null,
    sharedWith: sharedWith(h, hosts),
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
    ...over,
  };
  render(HostDetail, { props });
  return props;
}

describe('HostDetail', () => {
  it('shares the account with the other host in the usage block', () => {
    mount('claude-fleet-oci');
    expect(screen.getByTestId('usage-shared').textContent).toBe('· shared with mefistos');
    expect(screen.getByTestId('detail-account').textContent).toContain('admin-janci@users.noreply.github.com');
  });

  it('a host with no account says so and has no refresh', () => {
    mount('nas', { hostSessions: [] });
    expect(screen.getByTestId('usage-block').textContent).toContain('Not logged in to Claude on this host');
    expect(screen.queryByTestId('usage-refresh')).toBeNull();
    expect(screen.queryByTestId('detail-account')).toBeNull();
  });

  it('the remove confirm counts this host’s session rows, with Cancel focused', async () => {
    mount('claude-fleet-trn');
    await fireEvent.click(screen.getByTestId('detail-remove'));
    await tick();
    const dialog = screen.getByTestId('confirm-dialog');
    expect(dialog.textContent).toContain('Fleet deletes its 14 session rows');
    expect(document.activeElement).toBe(within(dialog).getByTestId('confirm-cancel'));
  });

  it('the re-probe button and the usage refresh call their props', async () => {
    const p = mount('mefistos');
    await fireEvent.click(screen.getByTestId('detail-reprobe'));
    expect(p.onreprobe).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('usage-refresh'));
    expect(p.onrefreshusage).toHaveBeenCalled();
  });

  it('no token: no mode control and no Rotate', () => {
    mount('mefistos', { token: null });
    expect(screen.queryByTestId('detail-token-mode')).toBeNull();
    expect(screen.queryByTestId('detail-rotate')).toBeNull();
    expect(screen.getByText('none — provision hosts to mint one')).toBeInTheDocument();
  });

  it('marks an agent-transport host in the facts list; an ssh host stays quiet', () => {
    mount('mefistos', { host: { ...host('mefistos'), transport: 'agent' } });
    expect(screen.getByTestId('detail-transport').textContent).toBe('agent');
  });

  it('an ssh host (the default) shows no transport fact', () => {
    mount('mefistos');
    expect(screen.queryByTestId('detail-transport')).toBeNull();
  });
});
