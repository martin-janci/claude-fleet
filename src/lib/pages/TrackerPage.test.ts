// Declarative pages P4b: the Trackers page — a master_detail page over the
// `tracker` resource with the connect flow — carries what the hand-written
// WorkSettings tracker list did (work graph M3, M6, M13.4e), through the
// same commands with the same arguments.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ResourcePage from './ResourcePage.svelte';
import { hubStatus, STANDALONE } from '../hub';
import { trackers, type TrackerRow } from '../trackers';
import { toasts } from '../toasts';
import { bundle } from './testing';
import type { Page } from './pages';

const page = bundle.pages.find((p) => p.id === 'settings.trackers') as Page;
const trackerResource = bundle.resources.find((r) => r.id === 'tracker')!;

const row = (over: Partial<TrackerRow> = {}): TrackerRow => ({
  id: 4,
  provider: 'jira',
  name: 'acme',
  site_url: 'https://acme.atlassian.net',
  transport: 'direct',
  state: 'ok',
  created_at: 1,
  last_sync_at: 1000,
  has_credential: true,
  username: 'me@acme.com',
  credential_hint: '…oken',
  config: { key_prefixes: ['ABC'] },
  settings: { section_map: { doing: 'in_progress' }, hostname: null },
  ...over,
});

function route(listed: TrackerRow[], extra: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in extra) {
      const v = extra[cmd];
      if (v instanceof Error) throw v;
      return v;
    }
    if (cmd === 'list_trackers') return listed;
    if (cmd === 'list_orgs' || cmd === 'tracker_sync_metrics' || cmd === 'status_map_proposals') return [];
    return null;
  });
  return inv;
}

function show(readonly = false, reason: string | null = null) {
  render(ResourcePage, { props: { page, resource: trackerResource, readonly, reason } });
}

const calls = (inv: ReturnType<typeof vi.fn>, cmd: string) => inv.mock.calls.filter((c) => c[0] === cmd);
const argsOf = (inv: ReturnType<typeof vi.fn>, cmd: string) =>
  (calls(inv, cmd).at(-1)![1] as { args: Record<string, unknown> }).args;

beforeEach(() => {
  hubStatus.set({ ...STANDALONE });
  trackers.set([]);
  toasts.set([]);
});

describe('Trackers (master_detail over the tracker resource)', () => {
  it('lists trackers with their tracker and state, and says what a refused Jira token means', async () => {
    route([row({ state: 'auth_failed', last_error: '401 from /myself' }), row({ id: 5, provider: 'asana', name: 'B', state: 'ok' })]);
    show();
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(2));
    expect(within(screen.getAllByTestId('resource-row')[0]).getAllByTestId('resource-badge').map((b) => b.textContent)).toEqual([
      'Jira Cloud',
      'token expired or wrong',
    ]);
    expect(screen.getByTestId('value-last_error').textContent).toBe('401 from /myself');
    expect(screen.getByText(/Atlassian API tokens expire within a year/)).toBeInTheDocument();
    expect(screen.getByTestId('value-username').textContent).toBe('me@acme.com');
  });

  it('Test reports the tracker’s answer; a failed test is a toast, and the list is re-read', async () => {
    const inv = route([row()], { test_tracker: { tracker: row(), ok: true, views: ['My work'] } });
    show();
    await fireEvent.click(await screen.findByTestId('record-action-tracker.test'));
    await waitFor(() => expect(argsOf(inv, 'test_tracker')).toEqual({ tracker_id: 4 }));
    await waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Test: ok'));
    const before = calls(inv, 'list_trackers').length;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'test_tracker') return { tracker: row(), ok: false, error: 'Atlassian answered 401' };
      if (cmd === 'list_trackers') return [row()];
      return [];
    });
    await fireEvent.click(screen.getByTestId('record-action-tracker.test'));
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('Atlassian answered 401'))).toBe(true),
    );
    await waitFor(() => expect(calls(inv, 'list_trackers').length).toBeGreaterThan(before));
  });

  it('Remove asks first', async () => {
    const inv = route([row()]);
    show();
    await fireEvent.click(await screen.findByTestId('record-delete'));
    expect((await screen.findByTestId('record-remove')).textContent).toContain('cached tickets');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() => expect(argsOf(inv, 'remove_tracker')).toEqual({ tracker_id: 4 }));
  });

  it('turns a Jira tracker’s PR write-back on without touching its other settings (M13.4e)', async () => {
    const inv = route([row()]);
    show();
    const box = (await screen.findByTestId('edit-pr_remote_link')) as HTMLInputElement;
    expect(box.checked).toBe(false);
    await fireEvent.click(box);
    await fireEvent.click(screen.getByTestId('record-apply'));
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('Fleet will write to this tracker');
    await fireEvent.click(screen.getByTestId('record-confirm'));
    await waitFor(() =>
      expect(argsOf(inv, 'update_tracker')).toEqual({
        tracker_id: 4,
        settings: { section_map: { doing: 'in_progress' }, hostname: null, write_back: { pr_remote_link: true } },
      }),
    );
  });

  it('offers PR write-back only for Jira trackers', async () => {
    route([row({ provider: 'asana', username: null })]);
    show();
    await screen.findByTestId('edit-name');
    expect(screen.queryByTestId('edit-pr_remote_link')).toBeNull();
    expect(screen.queryByTestId('value-username')).toBeNull();
  });

  it('replaces a credential with what that tracker needs: Jira an email and a token, Asana a token', async () => {
    const inv = route([row(), row({ id: 5, provider: 'asana', name: 'B', username: null })]);
    show();
    await fireEvent.click(await screen.findByTestId('record-action-tracker.replace_login'));
    expect(screen.queryByTestId('record-action-tracker.replace_token')).toBeNull();
    const secret = screen.getByTestId('param-tracker.replace_login-token') as HTMLInputElement;
    expect(secret.type).toBe('password');
    await fireEvent.input(screen.getByTestId('param-tracker.replace_login-email'), { target: { value: 'me@acme.com' } });
    await fireEvent.input(secret, { target: { value: 'ATATT-new' } });
    await fireEvent.click(screen.getByTestId('run-tracker.replace_login'));
    await waitFor(() =>
      expect(argsOf(inv, 'set_tracker_credential')).toEqual({ tracker_id: 4, username: 'me@acme.com', secret: 'ATATT-new' }),
    );

    await fireEvent.click(screen.getAllByTestId('resource-row')[1]);
    await fireEvent.click(await screen.findByTestId('record-action-tracker.replace_token'));
    await fireEvent.input(screen.getByTestId('param-tracker.replace_token-token'), { target: { value: 'pat' } });
    await fireEvent.click(screen.getByTestId('run-tracker.replace_token'));
    await waitFor(() => expect(argsOf(inv, 'set_tracker_credential')).toEqual({ tracker_id: 5, secret: 'pat' }));
  });

  it('adds a tracker through the connect flow, then selects it', async () => {
    const inv = route([], {
      flow_start: {
        flow_id: 'f1',
        flow: 'tracker.connect',
        step: 'url',
        title: 'Connect a tracker',
        fields: [{ name: 'url', label: 'URL', type: 'text', placeholder: '', value: '', required: true }],
        submit: 'Next',
        back: false,
      },
      flow_submit: { state: 'done', message: 'Connected https://acme.atlassian.net', record_id: 4 },
    });
    show();
    expect(await screen.findByTestId('resource-empty')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('resource-add'));
    await fireEvent.input(await screen.findByTestId('flow-field-url'), { target: { value: 'https://acme.atlassian.net' } });
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'flow_submit') return { state: 'done', message: 'Connected https://acme.atlassian.net', record_id: 4 };
      if (cmd === 'list_trackers') return [row()];
      return [];
    });
    await fireEvent.click(screen.getByTestId('flow-submit'));
    await waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Connected https://acme.atlassian.net'));
    await waitFor(() =>
      expect(screen.getAllByTestId('resource-row')[0].getAttribute('aria-selected')).toBe('true'),
    );
    expect(screen.queryByTestId('flow-tracker.connect')).toBeNull();
  });

  it('read-only on a paired desktop: the hub’s trackers, no Test, no write-back, no extras, and where to change them', async () => {
    const inv = route([row({ settings: { write_back: { pr_remote_link: true } } })]);
    show(true, 'trackers and their credentials are fleet administration — use `fleet-hub tracker`');
    await waitFor(() => expect(screen.getAllByTestId('resource-row')).toHaveLength(1));
    expect(screen.getByTestId('resource-readonly').textContent).toContain('fleet-hub tracker');
    for (const id of ['resource-add', 'record-delete', 'record-action-tracker.test', 'edit-pr_remote_link', 'tracker-extras']) {
      expect(screen.queryByTestId(id), id).toBeNull();
    }
    expect(screen.getByTestId('value-pr_remote_link').textContent).toBe('On');
    expect(calls(inv, 'tracker_sync_metrics')).toHaveLength(0);
    expect(calls(inv, 'update_tracker')).toHaveLength(0);
  });
});
