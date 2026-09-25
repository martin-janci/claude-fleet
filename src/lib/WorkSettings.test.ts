import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WorkSettings from './WorkSettings.svelte';
import { get } from 'svelte/store';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { trackers, type TrackerRow } from './trackers';
import { toasts } from './toasts';

const TOKEN = 'ATATT3xFfGF0-ui-test-token';

const row = (over: Partial<TrackerRow> = {}): TrackerRow => ({
  id: 4,
  provider: 'jira',
  name: 'acme',
  site_url: 'https://acme.atlassian.net',
  state: 'ok',
  created_at: 1,
  last_sync_at: 1000,
  has_credential: true,
  username: 'me@acme.com',
  credential_hint: '…oken',
  config: { key_prefixes: ['ABC'] },
  ...over,
});

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

function route(listed: TrackerRow[], extra: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in extra) return extra[cmd];
    if (cmd === 'list_trackers') return listed;
    return null;
  });
  return inv;
}

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  trackers.set([]);
  toasts.set([]);
});

describe('Settings → Work, standalone', () => {
  it('lists trackers with a state badge, a hint and Test / Remove', async () => {
    route([row(), row({ id: 5, name: 'other', site_url: 'https://other.atlassian.net', state: 'auth_failed' })]);
    render(WorkSettings, { props: { now: () => 1000 + 240 } });
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(2));
    const states = screen.getAllByTestId('tracker-state').map((e) => e.textContent);
    expect(states).toEqual(['ok', 'token expired or wrong']);
    expect(screen.getByTestId('tracker-expired').textContent).toMatch(/expire within a year/);
    expect(screen.getAllByTestId('tracker-row')[0].textContent).toContain('synced 4 min ago');
    expect(screen.getAllByTestId('tracker-test')).toHaveLength(2);
  });

  it('Test and Remove act on that tracker and re-read the list; a failed test is a toast', async () => {
    const inv = route([row()], {
      test_tracker: { tracker: row({ state: 'auth_failed' }), ok: false, error: 'the tracker refused the credential' },
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    const listed = () => inv.mock.calls.filter((c) => c[0] === 'list_trackers').length;
    const before = listed();
    await fireEvent.click(screen.getByTestId('tracker-test'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('test_tracker', { args: { tracker_id: 4 } }));
    await waitFor(() =>
      expect(get(toasts).map((t) => [t.kind, t.message])).toEqual([['error', 'the tracker refused the credential']]),
    );
    await waitFor(() => expect(listed()).toBe(before + 1));
    expect(screen.getByTestId('tracker-test')).toHaveTextContent('Test');
    await fireEvent.click(screen.getByTestId('tracker-remove'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('remove_tracker', { args: { tracker_id: 4 } }));
    await waitFor(() => expect(listed()).toBe(before + 2));
    // Neither ever carries a credential.
    for (const [cmd] of inv.mock.calls) expect(cmd).not.toBe('set_tracker_credential');
  });

  it('connects Jira from a pasted ticket URL, an email and a token — in that order', async () => {
    const inv = route([], {
      add_tracker: row({ state: 'unconfigured', has_credential: false }),
      set_tracker_credential: row({ state: 'unconfigured' }),
      test_tracker: { tracker: row(), ok: true, views: ['My work'] },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://acme.atlassian.net/browse/ABC-123' },
    });
    await tick();
    expect(screen.getByTestId('connect-site').textContent).toBe('https://acme.atlassian.net · ABC-123');
    await fireEvent.input(screen.getByTestId('connect-email'), { target: { value: 'me@acme.com' } });
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(screen.queryByTestId('connect-form')).toBeNull());
    // The Organisations section (work graph M5) reads its own lists on mount.
    const cmds = inv.mock.calls
      .map((c) => c[0])
      .filter((c) => !['list_trackers', 'list_orgs', 'org_suggestions'].includes(c as string));
    expect(cmds).toEqual(['add_tracker', 'set_tracker_credential', 'test_tracker']);
    const cred = inv.mock.calls.find((c) => c[0] === 'set_tracker_credential')![1] as {
      args: { tracker_id: number; username: string; secret: string };
    };
    expect(cred.args).toEqual({ tracker_id: 4, username: 'me@acme.com', secret: TOKEN });
    // The only command that ever carried the token.
    for (const [cmd, args] of inv.mock.calls) {
      if (cmd !== 'set_tracker_credential') expect(JSON.stringify(args ?? {})).not.toContain(TOKEN);
    }
  });

  it('refuses a URL that is not Jira Cloud before sending anything', async () => {
    const inv = route([]);
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://intranet.corp/browse/ABC-1' },
    });
    await tick();
    expect(screen.getByTestId('connect-url-error')).toBeInTheDocument();
    expect((screen.getByTestId('connect-submit') as HTMLButtonElement).disabled).toBe(true);
    expect(
      inv.mock.calls
        .map((c) => c[0])
        .filter((c) => !['list_orgs', 'org_suggestions'].includes(c as string)),
    ).toEqual(['list_trackers']);
  });

  it('forgets the token and email on Cancel and on a failed add, so nothing is pre-filled next time', async () => {
    const inv = route([]);
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), { target: { value: 'https://acme.atlassian.net' } });
    await fireEvent.input(screen.getByTestId('connect-email'), { target: { value: 'me@acme.com' } });
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('connect-cancel'));
    await tick();
    expect(screen.queryByTestId('connect-form')).toBeNull();
    await fireEvent.click(screen.getByTestId('connect-jira'));
    await tick();
    expect((screen.getByTestId('connect-token') as HTMLInputElement).value).toBe('');
    expect((screen.getByTestId('connect-email') as HTMLInputElement).value).toBe('');
    expect((screen.getByTestId('connect-submit') as HTMLButtonElement).disabled).toBe(true);
    // A failed add_tracker keeps the form open with the error, but not the secret.
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_tracker') throw { code: 'E_INVALID', message: 'not a tracker site' };
      if (cmd === 'list_trackers') return [];
      return null;
    });
    await fireEvent.input(screen.getByTestId('connect-email'), { target: { value: 'me@acme.com' } });
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(screen.getByTestId('connect-error').textContent).toMatch(/not a tracker site/));
    expect((screen.getByTestId('connect-token') as HTMLInputElement).value).toBe('');
    expect((screen.getByTestId('connect-email') as HTMLInputElement).value).toBe('');
  });

  it('shows a failed test in the form', async () => {
    route([], {
      add_tracker: row({ state: 'unconfigured' }),
      set_tracker_credential: row({ state: 'unconfigured' }),
      test_tracker: { tracker: row({ state: 'auth_failed' }), ok: false, error: 'the tracker refused the credential' },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), { target: { value: 'https://acme.atlassian.net' } });
    await fireEvent.input(screen.getByTestId('connect-email'), { target: { value: 'me@acme.com' } });
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(screen.getByTestId('connect-error').textContent).toMatch(/refused/));
    expect((screen.getByTestId('connect-token') as HTMLInputElement).value).toBe('');
  });
});

describe('Settings → Work, paired with a hub', () => {
  it('lists the hub trackers read-only and says to configure them on the hub', async () => {
    hubStatus.set(remote);
    route([row()]);
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    expect(screen.queryByTestId('connect-jira')).toBeNull();
    expect(screen.queryByTestId('tracker-test')).toBeNull();
    const note = screen.getByTestId('work-remote').textContent ?? '';
    expect(note).toContain('fleet-hub tracker add');
    expect(note).toContain('Do it on the hub (https://fleet.example.com)');
  });
});

describe('Settings → Work, other providers (work graph M6)', () => {
  it('connects GitHub through a host with gh, and never sends a credential', async () => {
    const { hosts } = await import('./hosts');
    hosts.set([{ alias: 'devbox', ssh_alias: null, reachable: true } as never]);
    const gh = row({
      id: 7,
      provider: 'github',
      name: 'acme (GitHub)',
      site_url: 'https://github.com/acme',
      transport: 'via_cli:devbox',
      has_credential: false,
      state: 'unconfigured',
    });
    const inv = route([], {
      add_tracker: gh,
      test_tracker: { tracker: { ...gh, state: 'ok' }, ok: true, views: ['My issues'] },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://github.com/acme/api/issues/42' },
    });
    await tick();
    expect(screen.getByTestId('connect-site').textContent).toContain('https://github.com/acme');
    expect(screen.queryByTestId('connect-token')).toBeNull();
    await fireEvent.change(screen.getByTestId('connect-gh-host'), { target: { value: 'devbox' } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'test_tracker')).toBe(true));
    const add = inv.mock.calls.find((c) => c[0] === 'add_tracker')!;
    expect((add[1] as { args: Record<string, unknown> }).args).toMatchObject({
      provider: 'github',
      transport: 'via_cli:devbox',
    });
    expect(inv.mock.calls.some((c) => c[0] === 'set_tracker_credential')).toBe(false);
  });

  it('re-connecting an existing Data Center site updates its CA and private-network flag on the row', async () => {
    const dc = row({
      id: 9,
      provider: 'jira_dc',
      name: 'corp',
      site_url: 'https://jira.corp.example',
      state: 'unreachable',
      has_credential: true,
      settings: { allow_private_network: false },
    });
    const inv = route([dc], {
      update_tracker: { ...dc, settings: { extra_ca: 'PEM', allow_private_network: true } },
      set_tracker_credential: dc,
      test_tracker: { tracker: { ...dc, state: 'ok' }, ok: true, views: ['My work'] },
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await fireEvent.click(screen.getByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), { target: { value: 'https://jira.corp.example' } });
    await fireEvent.change(screen.getByTestId('connect-provider'), { target: { value: 'jira_dc' } });
    await tick();
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.input(screen.getByTestId('connect-ca'), { target: { value: 'PEM' } });
    await fireEvent.click(screen.getByTestId('connect-private'));
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(screen.queryByTestId('connect-form')).toBeNull());
    const cmds = inv.mock.calls
      .map((c) => c[0])
      .filter((c) => !['list_trackers', 'list_orgs', 'org_suggestions'].includes(c as string));
    // No second row; the settings reach the existing one before the credential.
    expect(cmds).toEqual(['update_tracker', 'set_tracker_credential', 'test_tracker']);
    const up = inv.mock.calls.find((c) => c[0] === 'update_tracker')!;
    expect((up[1] as { args: Record<string, unknown> }).args).toEqual({
      tracker_id: 9,
      transport: undefined,
      settings: { extra_ca: 'PEM', allow_private_network: true },
    });
    const cred = inv.mock.calls.find((c) => c[0] === 'set_tracker_credential')!;
    expect((cred[1] as { args: Record<string, unknown> }).args).toMatchObject({ tracker_id: 9, secret: TOKEN });
  });

  it('re-connecting an existing GitHub site with another gh host updates its transport', async () => {
    const { hosts } = await import('./hosts');
    hosts.set([
      { alias: 'devbox', ssh_alias: null, reachable: true } as never,
      { alias: 'other', ssh_alias: null, reachable: true } as never,
    ]);
    const gh = row({
      id: 7,
      provider: 'github',
      name: 'acme (GitHub)',
      site_url: 'https://github.com/acme',
      transport: 'via_cli:devbox',
      has_credential: false,
      state: 'unreachable',
    });
    const inv = route([gh], {
      update_tracker: { ...gh, transport: 'via_cli:other' },
      test_tracker: { tracker: { ...gh, transport: 'via_cli:other', state: 'ok' }, ok: true, views: ['My issues'] },
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await fireEvent.click(screen.getByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://github.com/acme/api/issues/42' },
    });
    await tick();
    await fireEvent.change(screen.getByTestId('connect-gh-host'), { target: { value: 'other' } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'test_tracker')).toBe(true));
    expect(inv.mock.calls.some((c) => c[0] === 'add_tracker')).toBe(false);
    const up = inv.mock.calls.find((c) => c[0] === 'update_tracker')!;
    expect((up[1] as { args: Record<string, unknown> }).args).toMatchObject({
      tracker_id: 7,
      transport: 'via_cli:other',
    });
  });

  it('connects Asana with a token and no email', async () => {
    const asana = row({ id: 8, provider: 'asana', name: 'Asana', site_url: 'https://app.asana.com', state: 'unconfigured' });
    const inv = route([], {
      add_tracker: asana,
      set_tracker_credential: asana,
      test_tracker: { tracker: { ...asana, state: 'ok' }, ok: true, views: ['My tasks'] },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://app.asana.com/0/1200000000001001/1207000000000001' },
    });
    await tick();
    expect(screen.queryByTestId('connect-email')).toBeNull();
    await fireEvent.input(screen.getByTestId('connect-token'), { target: { value: TOKEN } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'test_tracker')).toBe(true));
    const cred = inv.mock.calls.find((c) => c[0] === 'set_tracker_credential')!;
    const args = (cred[1] as { args: Record<string, unknown> }).args;
    expect(args.username).toBeUndefined();
    expect(args.secret).toBe(TOKEN);
  });

  it('asks which Asana sections mean in progress, and saves the answer as confirmed', async () => {
    const asana = row({
      id: 8,
      provider: 'asana',
      name: 'Company B',
      site_url: 'https://app.asana.com',
      config: { section_map: { 'in progress': 'in_progress', shipped: 'done' } },
    });
    const inv = route([asana], { update_tracker: asana });
    render(WorkSettings);
    await waitFor(() => expect(screen.getByTestId('asana-sections')).toBeInTheDocument());
    await fireEvent.change(screen.getByTestId('asana-section-shipped'), {
      target: { value: 'in_progress' },
    });
    await fireEvent.click(screen.getByTestId('asana-sections-confirm'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'update_tracker')).toBe(true));
    const up = inv.mock.calls.find((c) => c[0] === 'update_tracker')!;
    expect((up[1] as { args: Record<string, unknown> }).args).toMatchObject({
      tracker_id: 8,
      settings: {
        section_map: { 'in progress': 'in_progress', shipped: 'in_progress' },
        section_map_confirmed: true,
      },
    });
  });
});
