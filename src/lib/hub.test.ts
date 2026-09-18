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
  HUB_ACTIONS,
  type HubStatus,
} from './hub';

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

  // PARITY OR REFUSAL, the rule Task 3 made and the ledger asks to write down:
  // new_session and repair_session REFUSE rather than route, because routing
  // would have succeeded while silently dropping the user's label.
  it('new_session and repair_session say why they refuse rather than routing', () => {
    expect(hubBlock('new_session', remote)).toMatch(/label|name/i);
    expect(hubBlock('repair_session', remote)).toBeTruthy();
  });

  // The terminal is the spec's named non-goal, and the hint has to be more
  // than "no".
  it('the terminal reason offers the shell command instead', () => {
    expect(hubBlock('terminal', remote)!.toLowerCase()).toContain('tmux attach');
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
