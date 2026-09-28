import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WorkSettings from './WorkSettings.svelte';
import { get } from 'svelte/store';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { trackers, type TrackerRow, type TrackerProposals } from './trackers';
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
  it('shows the Usage section, which reads the counts on mount (M13.2)', async () => {
    const inv = route([]);
    render(WorkSettings);
    await waitFor(() => expect(screen.getByTestId('work-usage')).toBeInTheDocument());
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'work_usage')).toBe(true));
  });

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
    // The Organisations section (work graph M5) reads its own lists on mount,
    // the sync metrics (M11.4) are read on mount and after a test, and the
    // Usage section (M13.2) reads its counts on mount.
    const cmds = inv.mock.calls
      .map((c) => c[0])
      .filter(
        (c) =>
          !['list_trackers', 'list_orgs', 'org_suggestions', 'tracker_sync_metrics', 'work_usage'].includes(
            c as string,
          ),
      );
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
        .filter((c) => !['list_orgs', 'org_suggestions', 'tracker_sync_metrics', 'work_usage'].includes(c as string)),
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

  it('shows no Usage section and never asks for usage (M13.2: work_admin is the hub master’s)', async () => {
    hubStatus.set(remote);
    const inv = route([row()]);
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    expect(screen.queryByTestId('work-usage')).toBeNull();
    expect(inv.mock.calls.some((c) => c[0] === 'work_usage')).toBe(false);
  });

  it('offers no PR write-back toggle, even on a Jira tracker that has it on (M13.4e)', async () => {
    hubStatus.set(remote);
    const inv = route([row({ settings: { write_back: { pr_remote_link: true } } })]);
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    expect(screen.queryByTestId('tracker-write-back-pr')).toBeNull();
    expect(inv.mock.calls.some((c) => c[0] === 'update_tracker')).toBe(false);
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
      .filter((c) => !['list_trackers', 'list_orgs', 'org_suggestions', 'tracker_sync_metrics', 'work_usage'].includes(c as string));
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

  it('turns a Jira tracker’s PR write-back on without touching its other settings (M13.4e)', async () => {
    const dc = row({
      id: 9,
      provider: 'jira_dc',
      name: 'corp',
      site_url: 'https://jira.corp.example',
      state: 'ok',
      has_credential: true,
      settings: { extra_ca: 'PEM' },
    });
    const inv = route([dc], {
      update_tracker: { ...dc, settings: { extra_ca: 'PEM', write_back: { pr_remote_link: true } } },
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    const box = screen.getByTestId('tracker-write-back-pr') as HTMLInputElement;
    expect(box.checked).toBe(false);
    await fireEvent.click(box);
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'update_tracker')).toBe(true));
    const up = inv.mock.calls.find((c) => c[0] === 'update_tracker')!;
    expect((up[1] as { args: Record<string, unknown> }).args).toEqual({
      tracker_id: 9,
      settings: { extra_ca: 'PEM', write_back: { pr_remote_link: true } },
    });
  });

  it('offers PR write-back only for Jira trackers', async () => {
    route([row({ id: 3, provider: 'github', name: 'gh', site_url: 'https://github.com', state: 'ok' })], {});
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    expect(screen.queryByTestId('tracker-write-back-pr')).toBeNull();
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

describe('Settings → Work, GitHub Enterprise Server and sync metrics (work graph M11.4)', () => {
  const ghe = (over: Partial<TrackerRow> = {}): TrackerRow =>
    row({
      id: 9,
      provider: 'github',
      name: 'acme (ghe.corp.example)',
      site_url: 'https://ghe.corp.example/acme',
      transport: 'via_cli:devbox',
      has_credential: false,
      settings: { hostname: 'ghe.corp.example:8443' },
      ...over,
    });

  it('offers a pasted enterprise issue URL as GitHub with the hostname prefilled', async () => {
    const { hosts } = await import('./hosts');
    hosts.set([{ alias: 'devbox', ssh_alias: null, reachable: true } as never]);
    const inv = route([], {
      add_tracker: ghe({ state: 'unconfigured' }),
      test_tracker: { tracker: ghe(), ok: true, views: ['My issues'] },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://ghe.corp.example:8443/Acme/api/issues/42' },
    });
    await tick();
    expect(screen.getByTestId('connect-site').textContent).toContain('https://ghe.corp.example/acme');
    expect(screen.getByTestId('connect-site').textContent).toContain('ghe.corp.example/acme/api#42');
    const field = screen.getByTestId('connect-ghes-hostname') as HTMLInputElement;
    expect(field.value).toBe('ghe.corp.example:8443');
    expect(screen.queryByTestId('connect-token')).toBeNull();
    await fireEvent.change(screen.getByTestId('connect-gh-host'), { target: { value: 'devbox' } });
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'test_tracker')).toBe(true));
    const add = inv.mock.calls.find((c) => c[0] === 'add_tracker')!;
    expect((add[1] as { args: Record<string, unknown> }).args).toMatchObject({
      url: 'https://ghe.corp.example:8443/Acme/api/issues/42',
      provider: 'github',
      transport: 'via_cli:devbox',
      settings: { hostname: 'ghe.corp.example:8443' },
    });
    expect(inv.mock.calls.some((c) => c[0] === 'set_tracker_credential')).toBe(false);
  });

  it('sends the hostname as edited, and cannot connect without one', async () => {
    const { hosts } = await import('./hosts');
    hosts.set([{ alias: 'devbox', ssh_alias: null, reachable: true } as never]);
    const inv = route([], {
      add_tracker: ghe({ state: 'unconfigured' }),
      test_tracker: { tracker: ghe(), ok: true, views: ['My issues'] },
    });
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    await fireEvent.input(screen.getByTestId('connect-url'), {
      target: { value: 'https://ghe.corp.example/acme/api/issues/42' },
    });
    await tick();
    await fireEvent.change(screen.getByTestId('connect-gh-host'), { target: { value: 'devbox' } });
    const field = screen.getByTestId('connect-ghes-hostname');
    expect((field as HTMLInputElement).value).toBe('ghe.corp.example');
    await fireEvent.input(field, { target: { value: '' } });
    await tick();
    expect((screen.getByTestId('connect-submit') as HTMLButtonElement).disabled).toBe(true);
    await fireEvent.input(field, { target: { value: 'ghe.corp.example:9443' } });
    await tick();
    await fireEvent.click(screen.getByTestId('connect-submit'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'add_tracker')).toBe(true));
    const add = inv.mock.calls.find((c) => c[0] === 'add_tracker')!;
    expect((add[1] as { args: { settings: unknown } }).args.settings).toEqual({
      hostname: 'ghe.corp.example:9443',
    });
  });

  it('a URL on an unknown host that is not GitHub-shaped is still not recognised', async () => {
    route([]);
    render(WorkSettings);
    await fireEvent.click(await screen.findByTestId('connect-jira'));
    for (const value of [
      'https://intranet.corp.example/wiki/page',
      'https://127.0.0.1/acme/api/issues/1',
      'https://localhost/acme/api/issues/1',
    ]) {
      await fireEvent.input(screen.getByTestId('connect-url'), { target: { value } });
      await tick();
      expect(screen.getByTestId('connect-url-error'), value).toBeInTheDocument();
      expect(screen.queryByTestId('connect-ghes-hostname')).toBeNull();
    }
  });

  it('shows the enterprise host badge and each tracker’s last sync pass', async () => {
    const inv = route([ghe(), row()], {
      tracker_sync_metrics: [
        {
          tracker_id: 9,
          last_pass_at: 1000,
          duration_ms: 1234,
          items_listed: 40,
          items_changed: 3,
          frames_emitted: 12,
          last_error: null,
        },
        { tracker_id: 4, last_pass_at: null },
      ],
    });
    render(WorkSettings, { props: { now: () => 1000 } });
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(2));
    expect(screen.getByTestId('tracker-ghes-host').textContent).toBe('GHES ghe.corp.example:8443');
    await waitFor(() => expect(screen.getAllByTestId('tracker-metrics')).toHaveLength(1));
    expect(screen.getByTestId('tracker-metrics').textContent).toContain(
      'last pass 1.2 s · 40 listed · 3 changed · 12 frames',
    );
    expect(inv.mock.calls.some((c) => c[0] === 'tracker_sync_metrics')).toBe(true);
  });

  it('shows the items a pass skipped and why (M13.1)', async () => {
    route([ghe()], {
      tracker_sync_metrics: [
        {
          tracker_id: 9,
          last_pass_at: 1000,
          duration_ms: 80,
          items_failed: 1,
          consecutive_partial: 2,
          last_item_error: 'UNIQUE constraint failed',
          last_error: null,
        },
      ],
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getByTestId('tracker-metrics-skipped')).toBeInTheDocument());
    expect(screen.getByTestId('tracker-metrics').textContent).toContain('1 skipped (2 passes in a row)');
    expect(screen.getByTestId('tracker-metrics-skipped').textContent).toContain('UNIQUE constraint failed');
    expect(screen.queryByTestId('tracker-metrics-error')).toBeNull();
  });

  it('shows a failed pass’s error, and asks for no metrics on a paired desktop', async () => {
    route([ghe()], {
      tracker_sync_metrics: [
        { tracker_id: 9, last_pass_at: 1000, duration_ms: 80, last_error: 'the tracker could not be reached' },
      ],
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getByTestId('tracker-metrics-error')).toBeInTheDocument());
    expect(screen.getByTestId('tracker-metrics').textContent).toContain('last pass 80 ms');
    expect(screen.getByTestId('tracker-metrics-error').textContent).toContain('could not be reached');

    hubStatus.set(remote);
    trackers.set([]);
    const inv = route([ghe()]);
    render(WorkSettings);
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'list_trackers')).toBe(true));
    expect(inv.mock.calls.some((c) => c[0] === 'tracker_sync_metrics')).toBe(false);
  });
});

describe('Settings → Work, Jev section proposals (status_map assist)', () => {
  const asana = (over: Partial<TrackerRow> = {}): TrackerRow =>
    row({
      id: 8,
      provider: 'asana',
      name: 'Company B',
      site_url: 'https://app.asana.com',
      username: null,
      config: { section_map: { 'in progress': 'in_progress', done: 'done' } },
      settings: { section_map: { 'in progress': 'in_progress', done: 'done' }, section_map_confirmed: true },
      ...over,
    });

  const proposals = (over: Partial<TrackerProposals> = {}): TrackerProposals[] => [
    {
      tracker_id: 8,
      name: 'Company B',
      org_id: 1,
      mode: 'assist',
      proposals: [
        {
          section: 'ideas',
          answer: 'todo',
          applies_as: 'todo',
          confidence: 0.82,
          run_id: 812,
          at: 1,
          top: [
            ['todo', 0.82],
            ['unsure', 0.11],
          ],
        },
        {
          section: 'parked <b>now</b>',
          answer: 'not_planned',
          applies_as: 'done',
          confidence: 0.7,
          run_id: 813,
          at: 1,
          top: [['not_planned', 0.7]],
        },
        { section: 'someday', answer: 'unsure', applies_as: null, confidence: 0.6, run_id: 814, at: 1 },
        // Already the person's: not shown.
        {
          section: 'backlog',
          answer: 'todo',
          applies_as: 'todo',
          run_id: 815,
          at: 1,
          person: 'todo',
          followup: 'confirmed',
        },
      ],
      shadow: [
        { section: 'in progress', rule: 'in_progress', model: 'in_progress', run_id: 700 },
        { section: 'done', rule: 'done', model: 'in_progress', run_id: 701 },
        { section: 'ideas', rule: 'none', model: 'todo', run_id: 702 },
      ],
      unknown_sections: 0,
      rejected: 0,
      ...over,
    },
  ];

  it('shows the pending proposals with their category, confidence and why, as plain text', async () => {
    route([asana()], { status_map_proposals: proposals() });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    expect(screen.getByTestId('jev-proposals').textContent).toContain('Proposed by Jev (assist)');
    const rows = screen.getAllByTestId('jev-proposal');
    expect(rows.map((r) => r.querySelector('[data-testid="jev-proposal-section"]')!.textContent)).toEqual([
      'ideas',
      'parked <b>now</b>',
      'someday',
    ]);
    // Third-party text is never markup.
    expect(rows[1].querySelector('b')).toBeNull();
    expect(rows[0].querySelector('[data-testid="jev-proposal-confidence"]')!.textContent).toBe('0.82');
    expect(rows[0].querySelector('[data-testid="jev-proposal-why"]')!.textContent).toBe(
      'why: to do 0.82 · unsure 0.11',
    );
    expect(rows[1].querySelector('[data-testid="jev-proposal-category"]')!.textContent).toMatch(
      /not planned\s+\(applies as done\)/,
    );
    // Unsure proposes nothing: no Apply, but Apply as… and Not this.
    expect(rows[2].querySelector('[data-testid="jev-apply"]')).toBeNull();
    expect(rows[2].querySelector('[data-testid="jev-apply-as"]')).not.toBeNull();
    expect(rows[2].querySelector('[data-testid="jev-reject"]')).not.toBeNull();
    expect(screen.getByTestId('jev-shadow-agreement').textContent).toMatch(
      /agreed with the keyword rule on 1 of 2/,
    );
  });

  it('Apply, Apply as… and Not this name the run, then re-read the trackers and the proposals', async () => {
    const inv = route([asana()], {
      status_map_proposals: proposals(),
      decide_status_map_proposal: {
        run_id: 812,
        tracker_id: 8,
        section: 'ideas',
        action: 'apply',
        category: 'todo',
        followup: 'confirmed',
      },
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    const count = (cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd).length;
    const before = [count('list_trackers'), count('status_map_proposals')];

    await fireEvent.click(screen.getAllByTestId('jev-apply')[0]);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', { args: { run_id: 812, action: 'apply' } }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 1));
    expect(count('list_trackers')).toBe(before[0] + 1);
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'success' && t.message.includes('ideas'))).toBe(true),
    );

    await fireEvent.change(screen.getAllByTestId('jev-apply-as')[1], { target: { value: 'in_progress' } });
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', {
        args: { run_id: 813, action: 'apply_as', category: 'in_progress' },
      }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 2));

    await fireEvent.click(screen.getAllByTestId('jev-reject')[2]);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('decide_status_map_proposal', { args: { run_id: 814, action: 'reject' } }),
    );
    await waitFor(() => expect(count('status_map_proposals')).toBe(before[1] + 3));
    // Nothing else wrote the tracker.
    expect(count('update_tracker')).toBe(0);
  });

  it('a failed decision is a toast', async () => {
    const inv = route([asana()]);
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_trackers') return [asana()];
      if (cmd === 'status_map_proposals') return proposals();
      if (cmd === 'decide_status_map_proposal')
        throw { code: 'E_INVALID_STATE', message: 'run 812 is not the latest proposal for this section (run 900 is)' };
      return null;
    });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('jev-proposal')).toHaveLength(3));
    await fireEvent.click(screen.getAllByTestId('jev-reject')[0]);
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('not the latest proposal'))).toBe(
        true,
      ),
    );
  });

  it('shows nothing outside assist, or when nothing is pending', async () => {
    route([asana()], { status_map_proposals: proposals({ mode: 'shadow' }) });
    const { unmount } = render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await tick();
    expect(screen.queryByTestId('jev-proposals')).toBeNull();
    unmount();
    trackers.set([]);
    route([asana()], { status_map_proposals: proposals({ proposals: [] }) });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await tick();
    expect(screen.queryByTestId('jev-proposals')).toBeNull();
  });

  it('asks for no proposals without an Asana tracker, nor on a paired desktop', async () => {
    const inv = route([row()], { status_map_proposals: proposals() });
    const { unmount } = render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'status_map_proposals')).toBe(false);
    unmount();

    hubStatus.set(remote);
    trackers.set([]);
    const inv2 = route([asana()], { status_map_proposals: proposals() });
    render(WorkSettings);
    await waitFor(() => expect(screen.getAllByTestId('tracker-row')).toHaveLength(1));
    await tick();
    expect(inv2.mock.calls.some((c) => c[0] === 'status_map_proposals')).toBe(false);
    expect(screen.queryByTestId('jev-proposals')).toBeNull();
    expect(screen.getByTestId('work-remote').textContent).toContain('fleet-hub decide proposals');
  });
});
