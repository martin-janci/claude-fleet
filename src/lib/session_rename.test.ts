// #147: the label/tmux-rename editor opens from a
// button (already disabled up front) AND from a double-click on the row
// (SessionRowItem) / the title (SessionDetails) — a path that doesn't
// consult a button's `disabled`. `applySessionRename` is the one place both
// routes funnel through before the IPC call, so it is gated there instead of
// at every way to reach it.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import { applySessionRename } from './session_rename';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';

const inv = () => mockedInvoke as ReturnType<typeof vi.fn>;
// A refusal still goes through `pushError`, which also fires a
// `report_client_error` telemetry call — not the rename IPC under test.
const ipcCalls = () => inv().mock.calls.filter((c) => c[0] !== 'report_client_error');

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

const target = { host_alias: 'trn', tmux_name: 'dev-foo', friendly_name: 'old label' };

beforeEach(() => {
  inv().mockReset();
  inv().mockResolvedValue({ ...target, id: 1 });
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
});

afterEach(() => {
  hubStatus.set({ ...STANDALONE });
  hubConnection.set({ state: 'standalone' });
});

describe('applySessionRename, label mode (set_friendly_name)', () => {
  it('does not call the IPC and returns an error while reconnecting', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'reconnecting', attempt: 1, retry_in_secs: 3, reason: 'closed' });
    const outcome = await applySessionRename(target, 'label', 'new label');
    expect(ipcCalls()).toHaveLength(0);
    expect(outcome.kind).toBe('error');
    if (outcome.kind === 'error') expect(outcome.error.message.toLowerCase()).toContain('unreachable');
  });

  it('standalone is untouched: it still calls set_session_friendly_name', async () => {
    const outcome = await applySessionRename(target, 'label', 'new label');
    expect(inv()).toHaveBeenCalledWith('set_session_friendly_name', {
      args: { host_alias: 'trn', tmux_name: 'dev-foo', friendly_name: 'new label' },
    });
    expect(outcome.kind).toBe('ok');
  });

  it('an unchanged value is a no-op before the gate is even consulted', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'offline', attempt: 1, retry_in_secs: 3, reason: 'refused' });
    const outcome = await applySessionRename(target, 'label', 'old label');
    expect(inv()).not.toHaveBeenCalled();
    expect(outcome.kind).toBe('noop');
  });
});

describe('applySessionRename, tmux mode (rename_session)', () => {
  it('does not call the IPC and returns an error while still connecting', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connecting' });
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(ipcCalls()).toHaveLength(0);
    expect(outcome.kind).toBe('error');
  });

  it('standalone is untouched: it still calls rename_session', async () => {
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(inv()).toHaveBeenCalledWith('rename_session', {
      args: { host_alias: 'trn', old_name: 'dev-foo', new_name: 'new-name' },
    });
    expect(outcome.kind).toBe('ok');
  });

  it('a hub client that is connected (routes fine) is not blocked', async () => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(inv()).toHaveBeenCalledWith('rename_session', expect.anything());
    expect(outcome.kind).toBe('ok');
  });
});
