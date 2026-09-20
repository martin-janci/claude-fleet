import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  STANDALONE,
  hubStatus,
  loadHubStatus,
  hubPair,
  hubDisconnect,
  hubBlock,
  hubNextStep,
  hubActionBlocked,
  HUB_ACTIONS,
  ROUTED_ACTIONS,
  type HubStatus,
} from './hub';
import type { HubConnection } from './hub_connection';

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

beforeEach(() => {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockResolvedValue(STANDALONE);
  hubStatus.set(STANDALONE);
});

describe('hubStatus', () => {
  it('starts standalone, so nothing is disabled before the backend answers', () => {
    expect(get(hubStatus).remote).toBe(false);
    for (const action of HUB_ACTIONS) {
      expect(hubBlock(action, get(hubStatus))).toBeNull();
    }
  });

  it('loadHubStatus stores what the backend answered', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(remote);
    const r = await loadHubStatus();
    expect(r.ok).toBe(true);
    expect(get(hubStatus).url).toBe('https://fleet.example.com');
    expect(mockedInvoke).toHaveBeenCalledWith('hub_status', undefined);
  });

  // A failed hub_status must not be read as "standalone": that would quietly
  // re-enable every fleet-administration button on a paired desktop.
  it('a failed hub_status leaves the last known mode alone', async () => {
    hubStatus.set(remote);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValue({
      code: 'E_LOCK',
      message: 'poisoned',
    });
    const r = await loadHubStatus();
    expect(r.ok).toBe(false);
    expect(get(hubStatus).remote).toBe(true);
  });
});

describe('hubBlock', () => {
  it('blocks nothing at all in standalone mode', () => {
    for (const action of HUB_ACTIONS) {
      expect(hubBlock(action, STANDALONE)).toBeNull();
    }
  });

  it('names the hub in every reason, so the sentence is actionable', () => {
    for (const action of HUB_ACTIONS) {
      const why = hubBlock(action, remote);
      expect(why, action).not.toBeNull();
      expect(why, action).toContain('fleet.example.com');
    }
  });

  // The design's rule: the honest answer is never "you cannot do this", it is
  // "not from here".
  it('every reason says where the action does work', () => {
    for (const action of HUB_ACTIONS) {
      expect(hubBlock(action, remote)!.toLowerCase(), action).toMatch(
        /on the hub|from the hub|standalone|disconnect/,
      );
    }
  });

  it('the fleet-administration actions say a client is not the administrator', () => {
    for (const action of ['add_host', 'remove_host', 'hide_host', 'provision_hosts', 'apply_sync', 'set_secret'] as const) {
      expect(hubBlock(action, remote)!.toLowerCase(), action).toContain('client');
    }
  });

  // The terminal used to be the spec's named non-goal, with a reason that
  // offered the shell command instead. It is not blocked any more: `pty_open`
  // is this machine's own ssh/tmux either way, so a paired desktop attaches.
  // The key is gone, and nothing may quietly reintroduce it — a reason ending
  // "Do it on the hub" would be false for the only case left (an agent host,
  // which the hub cannot attach either).
  it('has no terminal reason: a hub client attaches its own PTY', () => {
    expect(HUB_ACTIONS as readonly string[]).not.toContain('terminal');
  });

  // The audit's finding H, the frontend half: the hub DOES serve the asset
  // list (`list_assets`, read-only, open to any paired client). What it does
  // not serve is the catalog's configuration and checkout, which the panel is
  // built on — so that is the reason, and it must not deny a tool that exists.
  it('the asset catalog reason does not deny the tool the hub has', () => {
    const said = hubBlock('catalog_config', remote)!;
    expect(said).not.toMatch(/no authoring tool|exposes no tool/);
    expect(said).toContain('list_assets');
  });
});

// F1: a hub is configured but this launch could not use it. The backend owns
// nothing in that state and refuses what these controls would do, so they are
// disabled — with the real reason, not a claim that some hub owns the fleet.
describe('hubBlock, configured hub unavailable', () => {
  const unavailable: HubStatus = {
    ...STANDALONE,
    configured_url: 'https://fleet.example.com',
    unavailable: 'https://fleet.example.com is configured but no client token is stored',
  };

  it('blocks every action, naming the reason and Settings', () => {
    for (const action of HUB_ACTIONS) {
      const why = hubBlock(action, unavailable);
      expect(why, action).not.toBeNull();
      expect(why, action).toContain('no client token is stored');
      expect(why!.toLowerCase(), action).toContain('settings');
    }
  });
});

describe('hubNextStep', () => {
  // Requirement (c), the real trap: with confirm-destructive on, the hub
  // refuses and the desktop's own confirm dialog answers THIS process's
  // queue, not the hub's. Someone would click, see a refusal, and have
  // nowhere to go.
  it('tells a hub client that a confirmation has to be approved on the hub', () => {
    const step = hubNextStep({ code: 'E_CONFIRM_REQUIRED', message: 'confirm' }, remote);
    expect(step).toBeTruthy();
    expect(step!.toLowerCase()).toContain('on the hub');
    expect(step).toContain('fleet.example.com');
    // The ledger's exact copy. Once the operator approves, the change comes
    // back over the event bridge and this window updates itself, so telling
    // anyone to "refresh" would send them to a button that does nothing new.
    expect(step!.toLowerCase()).toContain('approve it on the hub — this window will follow');
    expect(step!.toLowerCase()).not.toContain('refresh');
  });

  it('says nothing about confirmations in standalone mode, where the dialog works', () => {
    expect(hubNextStep({ code: 'E_CONFIRM_REQUIRED', message: 'confirm' }, STANDALONE)).toBeNull();
  });

  it('sends a revoked client back to Settings', () => {
    const step = hubNextStep({ code: 'E_UNAUTHORIZED', message: 'revoked' }, remote);
    expect(step!.toLowerCase()).toContain('settings');
  });

  it('adds nothing to an error that already explains itself', () => {
    expect(hubNextStep({ code: 'E_LOCAL_ONLY', message: '…; do it on the hub' }, remote)).toBeNull();
    expect(hubNextStep({ code: 'E_SSH', message: 'timed out' }, remote)).toBeNull();
  });
});

describe('hubPair', () => {
  it('sends the URL, the code and the plaintext decision', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockResolvedValue({ ...remote, remote: false, restart_required: true });
    const r = await hubPair(' https://fleet.example.com/ ', ' abcd1234 ', false);
    expect(r.ok).toBe(true);
    expect(inv).toHaveBeenCalledWith('hub_pair', {
      args: { url: 'https://fleet.example.com/', code: 'abcd1234', allow_plaintext: false },
    });
    expect(get(hubStatus).restart_required).toBe(true);
  });

  it('carries the plaintext opt-in when the user accepted the risk', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockResolvedValue(remote);
    await hubPair('http://10.0.0.5:8787', 'ABCD1234', true);
    expect(inv.mock.calls[0][1]).toEqual({
      args: { url: 'http://10.0.0.5:8787', code: 'ABCD1234', allow_plaintext: true },
    });
  });

  it('a refused pairing leaves the stored status untouched', async () => {
    hubStatus.set(STANDALONE);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValue({
      code: 'E_INVALID',
      message: 'that code is not valid',
    });
    const r = await hubPair('https://fleet.example.com', 'BADCODE1', false);
    expect(r.ok).toBe(false);
    expect(get(hubStatus)).toEqual(STANDALONE);
  });
});

describe('hubDisconnect', () => {
  it('calls the command and stores the resulting status', async () => {
    hubStatus.set(remote);
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockResolvedValue({ ...remote, configured_url: null, restart_required: true });
    const r = await hubDisconnect();
    expect(r.ok).toBe(true);
    expect(inv).toHaveBeenCalledWith('hub_disconnect', undefined);
    expect(get(hubStatus).configured_url).toBeNull();
    expect(get(hubStatus).restart_required).toBe(true);
  });
});

// #147: the one place a control checks both refusal (hubBlock) and the live
// connection (hubConnection) — offline gating for routed mutations. The
// truth table below is the actual contract: standalone / owning the fleet
// locally / hub connected / each not-connected state, crossed with a refused
// action, a routed action, and one unaffected by either.
describe('hubActionBlocked', () => {
  const unavailable: HubStatus = {
    ...STANDALONE,
    configured_url: 'https://fleet.example.com',
    unavailable: 'https://fleet.example.com is configured but no client token is stored',
  };

  const STANDALONE_CONN: HubConnection = { state: 'standalone' };
  const CONNECTING: HubConnection = { state: 'connecting' };
  const CONNECTED: HubConnection = { state: 'connected' };
  const RECONNECTING: HubConnection = {
    state: 'reconnecting',
    attempt: 2,
    retry_in_secs: 5,
    reason: 'socket closed',
  };
  const OFFLINE: HubConnection = {
    state: 'offline',
    attempt: 4,
    retry_in_secs: 30,
    reason: 'connect refused',
  };
  const TOO_OLD: HubConnection = { state: 'hub_too_old', hub_contract: 1, min_contract: 3 };
  const TOO_NEW: HubConnection = { state: 'hub_too_new', hub_contract: 9, max_contract: 5 };

  // `standalone` (no hub configured) must never disable anything, in either
  // half, whatever the connection store happens to hold.
  it('standalone blocks nothing at all, for a refused action, a routed one, or neither', () => {
    for (const conn of [STANDALONE_CONN, CONNECTING, CONNECTED, RECONNECTING, OFFLINE, TOO_OLD, TOO_NEW]) {
      expect(hubActionBlocked('add_host', STANDALONE, conn)).toBeNull();
      expect(hubActionBlocked('kill_session', STANDALONE, conn)).toBeNull();
    }
  });

  // Refusal wins: a refused action is blocked in remote mode regardless of
  // the connection, and says the REASONS sentence, not an offline one.
  it('a refused action is blocked the same way whatever the connection is doing', () => {
    for (const conn of [CONNECTING, CONNECTED, RECONNECTING, OFFLINE, TOO_OLD, TOO_NEW]) {
      const why = hubActionBlocked('add_host', remote, conn);
      expect(why, conn.state).not.toBeNull();
      expect(why, conn.state).toBe(hubBlock('add_host', remote));
      expect(why!.toLowerCase(), conn.state).toContain('client');
    }
  });

  // A routed action: enabled only once the connection is actually up.
  it('a routed action is enabled while connected, blocked in every other connection state', () => {
    expect(hubActionBlocked('kill_session', remote, CONNECTED)).toBeNull();
    for (const conn of [CONNECTING, RECONNECTING, OFFLINE]) {
      const why = hubActionBlocked('kill_session', remote, conn);
      expect(why, conn.state).not.toBeNull();
      expect(why!.toLowerCase(), conn.state).toMatch(/unreachable|connecting/);
    }
    for (const conn of [TOO_OLD, TOO_NEW]) {
      const why = hubActionBlocked('kill_session', remote, conn);
      expect(why, conn.state).not.toBeNull();
      expect(why!.toLowerCase(), conn.state).toContain('incompatible');
    }
  });

  // The skew states name which side is behind, distinctly.
  it('the skew states say which side is out of date', () => {
    expect(hubActionBlocked('send_prompt', remote, TOO_OLD)!.toLowerCase()).toContain('update the hub');
    expect(hubActionBlocked('send_prompt', remote, TOO_NEW)!.toLowerCase()).toContain('update this app');
  });

  // An action neither refused nor routed (a read, navigation, or one of the
  // handful of commands that run the same in both modes) is never blocked
  // here — this module has nothing to say about it.
  it('an action that is neither refused nor routed is never blocked, connected or not', () => {
    for (const conn of [CONNECTING, CONNECTED, RECONNECTING, OFFLINE, TOO_OLD, TOO_NEW]) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect(hubActionBlocked('hub_status' as any, remote, conn)).toBeNull();
    }
  });

  // #148 finding 9: `action in REASONS` walks the prototype chain, so an
  // action name that only collides with an inherited `Object.prototype`
  // member (never one of this module's own keys) must not be treated as a
  // refused action.
  it('an action name that only collides with Object.prototype is not treated as refused', () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(hubActionBlocked('toString' as any, remote, CONNECTED)).toBeNull();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect(hubActionBlocked('constructor' as any, remote, CONNECTED)).toBeNull();
  });

  // F1: a hub is configured but this launch could not use it — refuses every
  // routed command too (the routing test's `a_configured_but_unavailable_hub_
  // refuses_every_routed_command`), not just the ones with a REASONS entry.
  it('a configured-but-unavailable hub blocks a routed action as well as a refused one', () => {
    const why = hubActionBlocked('kill_session', unavailable, CONNECTED);
    expect(why).not.toBeNull();
    expect(why).toContain('no client token is stored');
    expect(why!.toLowerCase()).toContain('settings');
  });

  // A desktop that owns its fleet locally but isn't the bare STANDALONE
  // constant — e.g. it still has a hub URL saved from before Disconnect,
  // pending a restart — must be treated exactly like STANDALONE: `remote` and
  // `unavailable` are what `ownsTheFleet` actually checks, not object
  // identity with the constant.
  const LOCAL_OWNER: HubStatus = {
    ...STANDALONE,
    configured_url: 'https://fleet.example.com',
    restart_required: true,
  };

  const NOT_CONNECTED: HubConnection[] = [
    STANDALONE_CONN,
    CONNECTING,
    RECONNECTING,
    OFFLINE,
    TOO_OLD,
    TOO_NEW,
  ];
  const EVERY_ACTION = [...HUB_ACTIONS, ...ROUTED_ACTIONS];

  // The exhaustive local-mode sweep: every refused key and every routed key,
  // for both flavours of "owns the fleet locally", across every connection
  // state that isn't `connected` (including a stale `reconnecting` left over
  // from a hub this desktop no longer points at) — none of it may ever
  // block a local desktop.
  it('every refused and routed action is unblocked for a local-owning desktop, in any connection state', () => {
    for (const status of [STANDALONE, LOCAL_OWNER]) {
      for (const conn of NOT_CONNECTED) {
        for (const action of EVERY_ACTION) {
          expect(hubActionBlocked(action, status, conn), `${status === STANDALONE ? 'STANDALONE' : 'LOCAL_OWNER'}/${conn.state}/${action}`).toBeNull();
        }
      }
    }
  });

  // The exhaustive connected-hub-client sweep: every routed key is sendable
  // once the connection is up, and every refused key still says no — refusal
  // never depends on the connection being fine.
  it('on a connected hub client, every routed key is null and every refused key is non-null', () => {
    for (const action of ROUTED_ACTIONS) {
      expect(hubActionBlocked(action, remote, CONNECTED), action).toBeNull();
    }
    for (const action of HUB_ACTIONS) {
      expect(hubActionBlocked(action, remote, CONNECTED), action).not.toBeNull();
    }
  });
});
