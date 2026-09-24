import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WorkSettings from './WorkSettings.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { trackers, type TrackerRow } from './trackers';

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
    const cmds = inv.mock.calls.map((c) => c[0]).filter((c) => c !== 'list_trackers');
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
    expect(inv.mock.calls.map((c) => c[0])).toEqual(['list_trackers']);
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
