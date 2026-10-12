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
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { resetAccessForTests, setMyGrants } from './access';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { UNKNOWN_SESSION_REASON } from './share';

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

// The pinned identity a double-click hands over: Sidebar pins `{ id, host_alias,
// tmux_name, mode, original }` and SessionDetails passes the row itself, so the
// id is in hand at both call sites — and since F2e it is what the access half
// asks about, the host and the name being only what the IPC renames.
const target = { id: 1, host_alias: 'trn', tmux_name: 'dev-foo', friendly_name: 'old label' };

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
    // Multi-user M1: a paired desktop also has to know whose row this is. A
    // device the hub has not identified fails closed on every row, which is a
    // different state and is pinned in the access-gate suite below.
    const mine = session('trn', 'dev-foo', { id: 1, visibility: 'private', owner_person_id: 7 });
    sessions.set([mine]);
    setMyGrants(7, []);
    inv().mockResolvedValue({ ...mine, tmux_name: 'new-name' });
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(inv()).toHaveBeenCalledWith('rename_session', expect.anything());
    expect(outcome.kind).toBe('ok');
  });
});

// ── Multi-user M1 (F2a): the double-click path, where no button is consulted ─
//
// This module is the one place both routes to a rename funnel through, which is
// why #147 put the hub's gate here rather than on every control. The access half
// was missing: a watcher could double-click a shared row's name — past a Label
// button that F2 had already disabled — and relabel, or rename the tmux session,
// on somebody else's machine. `set_friendly_name` is `drive` in
// `share.ts::SESSION_TIER` and `rename_session` is `own`.
//
// The row is resolved out of the store by the SESSION ID the pinned identity
// carries (F2e), through `share.ts::sessionIdActionBlocked`: the identity itself
// carries no `owner_person_id`, and a tmux name is not an identity over time.
describe('applySessionRename access gate (multi-user M1)', () => {
  const row = (owner: number) =>
    session('trn', 'dev-foo', {
      id: 1,
      friendly_name: 'old label',
      visibility: 'private',
      owner_person_id: owner,
    });

  beforeEach(() => {
    hubStatus.set(remote);
    hubConnection.set({ state: 'connected' });
    resetAccessForTests();
    // The answer carries the owner: `createRowStore` replaces a held row
    // WHOLESALE on merge, so a mock answer without `owner_person_id` would
    // un-own the row mid-test — the very pipeline fact that made M1 derive
    // access instead of stamping it on the row (`access.ts`, R6-j).
    inv().mockImplementation(async () => row(7));
  });

  afterEach(() => {
    sessions.set([]);
    resetAccessForTests();
  });

  it('the owner renames on a paired desktop — both modes (the positive control)', async () => {
    sessions.set([row(7)]);
    setMyGrants(7, []);
    expect((await applySessionRename(target, 'label', 'new label')).kind).toBe('ok');
    expect(inv()).toHaveBeenCalledWith('set_session_friendly_name', expect.anything());
    inv().mockClear();
    expect((await applySessionRename(target, 'tmux', 'new-name')).kind).toBe('ok');
    expect(inv()).toHaveBeenCalledWith('rename_session', expect.anything());
  });

  it('a watcher cannot relabel it, and the IPC is never called', async () => {
    inv().mockImplementation(async () => row(42));
    sessions.set([row(42)]);
    setMyGrants(7, [{ session_id: 1, level: 'watch' }]);
    const outcome = await applySessionRename(target, 'label', 'new label');
    expect(ipcCalls()).toHaveLength(0);
    expect(outcome.kind).toBe('error');
    if (outcome.kind === 'error') {
      expect(outcome.error.message).toMatch(/needs drive/i);
      // Which half refused, in the code: not `E_LOCAL_ONLY` — the hub would
      // route this fine, it is simply not this client's session.
      expect(outcome.error.code).toBe('E_FORBIDDEN');
    }
  });

  it('a driver may relabel but may not rename the tmux session', async () => {
    // The two tiers, on the same row, through the same function: the label is
    // fleet's own metadata (`drive`), the tmux name is the row's identity
    // (`own`).
    inv().mockImplementation(async () => row(42));
    sessions.set([row(42)]);
    setMyGrants(7, [{ session_id: 1, level: 'drive' }]);
    expect((await applySessionRename(target, 'label', 'new label')).kind).toBe('ok');
    expect(inv()).toHaveBeenCalledWith('set_session_friendly_name', expect.anything());
    inv().mockClear();
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(ipcCalls()).toHaveLength(0);
    expect(outcome.kind).toBe('error');
    if (outcome.kind === 'error') {
      expect(outcome.error.message).toMatch(/only the session’s owner/i);
      expect(outcome.error.code).toBe('E_FORBIDDEN');
    }
  });

  it('standalone is untouched, and so is a rename of a row the store does not hold', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    sessions.set([]);
    expect((await applySessionRename(target, 'label', 'new label')).kind).toBe('ok');
    expect((await applySessionRename(target, 'tmux', 'new-name')).kind).toBe('ok');
  });

  // ── F2e: the two defects `accessBlocked` had ─────────────────────────────
  //
  //  1. a row the store does not hold answered `null` = allowed. On a paired
  //     desktop the hub fences rows this person may not see off the stream, so
  //     that is a rename offered on a session we cannot tell the owner of —
  //     the fail-open F2d deleted from four other surfaces.
  //  2. the row was resolved on `(host_alias, tmux_name)`, which is not a
  //     session identity over time: a lost row's pane name is reusable, so the
  //     answer could be about a NAMESAKE — and the namesake is typically the row
  //     this person just started, so it was wrong in the `own` direction.
  //
  // Both tests fail if the helper goes back to `get(sessions).find` on the name.

  it('a paired desktop refuses a rename of a row it cannot see — both modes', async () => {
    sessions.set([]);
    setMyGrants(7, []);
    const label = await applySessionRename(target, 'label', 'new label');
    expect(ipcCalls()).toHaveLength(0);
    expect(label.kind).toBe('error');
    if (label.kind === 'error') {
      expect(label.error.message).toBe(UNKNOWN_SESSION_REASON);
      // Not `E_LOCAL_ONLY`: the hub would route this fine, we simply cannot
      // tell whose session it is.
      expect(label.error.code).toBe('E_FORBIDDEN');
    }
    const tmux = await applySessionRename(target, 'tmux', 'new-name');
    expect(ipcCalls()).toHaveLength(0);
    expect(tmux.kind).toBe('error');
    if (tmux.kind === 'error') expect(tmux.error.message).toBe(UNKNOWN_SESSION_REASON);
  });

  it('a namesake row this person owns does not license the rename', async () => {
    // Same host, same pane name, DIFFERENT session — the live row that inherited
    // a lost session's name, owned by the person renaming. A lookup by name
    // answered `own` for it and let the rename through on session 1.
    sessions.set([
      session('trn', 'dev-foo', { id: 99, visibility: 'private', owner_person_id: 7 }),
    ]);
    setMyGrants(7, []);
    const outcome = await applySessionRename(target, 'tmux', 'new-name');
    expect(ipcCalls()).toHaveLength(0);
    expect(outcome.kind).toBe('error');
    if (outcome.kind === 'error') expect(outcome.error.message).toBe(UNKNOWN_SESSION_REASON);
  });

  it('and the id still picks the right row out of two that share the pane name', async () => {
    // The positive control for the same defect, the other way round: with the
    // namesake FIRST in the list, `find` by name answered about row 99 — which
    // is not this person's — and refused the owner their own rename.
    sessions.set([
      session('trn', 'dev-foo', { id: 99, visibility: 'private', owner_person_id: 42 }),
      row(7),
    ]);
    setMyGrants(7, []);
    expect((await applySessionRename(target, 'tmux', 'new-name')).kind).toBe('ok');
    expect(inv()).toHaveBeenCalledWith('rename_session', expect.anything());
  });
});
