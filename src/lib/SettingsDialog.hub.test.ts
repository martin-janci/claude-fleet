import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

const mcpStatusObj = {
  enabled: false,
  running: false,
  port: 4180,
  token: 'test-token',
  url: 'http://127.0.0.1:4180/mcp',
  bind_error: null,
  confirm_destructive: false,
};

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

/** Route invoke by command, with per-test overrides. */
function route(overrides: Record<string, unknown> = {}) {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    if (cmd in overrides) {
      const v = overrides[cmd];
      if (v instanceof Error) throw v;
      return v;
    }
    switch (cmd) {
      case 'mcp_status':
      case 'mcp_configure':
        return mcpStatusObj;
      case 'hub_status':
        return get(hubStatus);
      case 'list_host_tokens':
        return [];
      case 'get_fleet_settings':
        return {};
      default:
        return null;
    }
  });
  return inv;
}

function ipcError(code: string, message: string): Error {
  return Object.assign(new Error(message), { code, message });
}

beforeEach(() => {
  hosts.set([]);
  hubStatus.set({ ...STANDALONE });
  route();
});

describe('the Hub section, standalone', () => {
  it('shows an empty state rather than pretending a hub is configured', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const section = await screen.findByTestId('hub-section');
    expect(section.textContent).toContain('Hub');
    expect(screen.getByTestId('hub-empty')).toBeInTheDocument();
    expect(screen.queryByTestId('hub-disconnect')).toBeNull();
    // The two fields a pairing needs, and nothing that claims a state it is
    // not in.
    expect(screen.getByTestId('hub-url')).toBeInTheDocument();
    expect(screen.getByTestId('hub-code')).toBeInTheDocument();
  });

  it('tells the operator where the code comes from', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const section = await screen.findByTestId('hub-section');
    expect(section.textContent).toContain('fleet-hub pair');
  });

  it('will not pair on an empty form', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const pair = (await screen.findByTestId('hub-pair')) as HTMLButtonElement;
    expect(pair).toBeDisabled();
  });
});

describe('pairing', () => {
  it('sends the URL and the code, and then asks for a restart', async () => {
    const inv = route({
      hub_pair: { ...remote, remote: false, restart_required: true },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.input(await screen.findByTestId('hub-url'), {
      target: { value: 'https://fleet.example.com' },
    });
    await fireEvent.input(screen.getByTestId('hub-code'), { target: { value: 'ABCD1234' } });
    await fireEvent.click(screen.getByTestId('hub-pair'));

    await waitFor(() =>
      expect(inv.mock.calls.some((c) => c[0] === 'hub_pair')).toBe(true),
    );
    const call = inv.mock.calls.find((c) => c[0] === 'hub_pair')!;
    expect(call[1]).toEqual({
      args: {
        url: 'https://fleet.example.com',
        code: 'ABCD1234',
        allow_plaintext: false,
      },
    });
    // The backend resolves the mode once, at startup, on purpose. Saying
    // "paired" and leaving the app behaving exactly as before would be the
    // worst of both.
    const restart = await screen.findByTestId('hub-restart');
    expect(restart.textContent!.toLowerCase()).toContain('restart');
  });

  it('shows the hub’s own refusal rather than a generic failure', async () => {
    route({ hub_pair: ipcError('E_INVALID', 'that pairing code is unknown, used or expired') });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.input(await screen.findByTestId('hub-url'), {
      target: { value: 'https://fleet.example.com' },
    });
    await fireEvent.input(screen.getByTestId('hub-code'), { target: { value: 'BADCODE1' } });
    await fireEvent.click(screen.getByTestId('hub-pair'));
    const err = await screen.findByTestId('hub-error');
    expect(err.textContent).toContain('unknown, used or expired');
  });

  // Requirement (d)(i): the plaintext warning must reach the PERSON, before
  // any token is stored. It used to reach only the log.
  it('refuses plaintext once, in words, and only then offers to do it anyway', async () => {
    const inv = route({
      hub_pair: ipcError(
        'E_HUB_PLAINTEXT',
        'http://10.0.0.5:8787 is plain http to a host that is not loopback, so this app’s client token — a credential for the whole fleet — would cross the network in the clear on every call. Use https://, or pair again with "send it in the clear anyway".',
      ),
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.input(await screen.findByTestId('hub-url'), {
      target: { value: 'http://10.0.0.5:8787' },
    });
    await fireEvent.input(screen.getByTestId('hub-code'), { target: { value: 'ABCD1234' } });

    // Before the refusal there is nothing to click: the opt-in is not a
    // checkbox someone can tick past without reading.
    expect(screen.queryByTestId('hub-allow-plaintext')).toBeNull();

    await fireEvent.click(screen.getByTestId('hub-pair'));

    const err = await screen.findByTestId('hub-error');
    expect(err.textContent).toContain('in the clear');
    const optIn = (await screen.findByTestId('hub-allow-plaintext')) as HTMLInputElement;
    expect(optIn.checked).toBe(false);

    // Ticking it and pairing again sends the decision.
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'hub_pair') return { ...remote, allow_plaintext: true, restart_required: true };
      if (cmd === 'hub_status') return get(hubStatus);
      if (cmd === 'mcp_status') return mcpStatusObj;
      if (cmd === 'get_fleet_settings') return {};
      return null;
    });
    await fireEvent.click(optIn);
    await fireEvent.click(screen.getByTestId('hub-pair'));
    await waitFor(() =>
      expect(inv.mock.calls.some((c) => c[0] === 'hub_pair')).toBe(true),
    );
    const last = inv.mock.calls.filter((c) => c[0] === 'hub_pair').at(-1)!;
    expect((last[1] as { args: { allow_plaintext: boolean } }).args.allow_plaintext).toBe(true);
  });
});

describe('the Hub section, paired', () => {
  beforeEach(() => {
    hubStatus.set(remote);
    route();
  });

  it('names the hub and the client this desktop is paired as', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const section = await screen.findByTestId('hub-section');
    expect(section.textContent).toContain('https://fleet.example.com');
    expect(section.textContent).toContain('laptop');
    expect(screen.queryByTestId('hub-empty')).toBeNull();
  });

  // The spec is explicit: Disconnecting does not revoke the token — that is
  // the operator's, from the hub. A wording that implies otherwise would
  // leave someone believing a stolen laptop had been locked out.
  it('says plainly that Disconnect revokes nothing', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const note = await screen.findByTestId('hub-disconnect-note');
    expect(note.textContent!.toLowerCase()).toContain('does not revoke');
    expect(note.textContent).toContain('fleet-hub');
  });

  it('Disconnect calls the command and asks for a restart', async () => {
    const inv = route({
      hub_disconnect: {
        ...STANDALONE,
        remote: true,
        url: 'https://fleet.example.com',
        client_name: 'laptop',
        restart_required: true,
      },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('hub-disconnect'));
    await waitFor(() =>
      expect(inv.mock.calls.some((c) => c[0] === 'hub_disconnect')).toBe(true),
    );
    expect(await screen.findByTestId('hub-restart')).toBeInTheDocument();
  });

  // Requirement (c). With confirm-destructive on, the hub can refuse a kill
  // or a delete until someone approves it — and this desktop's confirmation
  // dialog answers its OWN queue, which is always empty here. Nothing in the
  // interface said so.
  it('warns that a destructive confirmation has to be approved on the hub', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const note = await screen.findByTestId('hub-confirm-note');
    expect(note.textContent!.toLowerCase()).toContain('on the hub');
    expect(note.textContent).toMatch(/confirm/i);
    const said = note.textContent!.replace(/\s+/g, ' ').toLowerCase();
    expect(said).toContain('approve it on the hub — this window will follow');
    expect(said).not.toContain('refresh');
  });

  // A real, correct difference from standalone that nobody would guess.
  it('says that prompts sent from here reach the agent marked untrusted', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const note = await screen.findByTestId('hub-untrusted-note');
    expect(note.textContent!.toLowerCase()).toContain('untrusted');
  });
});

describe('the panels that do not apply to a hub client', () => {
  beforeEach(() => {
    hubStatus.set(remote);
  });

  // Requirement (a): these six are called UNPROMPTED and now return
  // E_LOCAL_ONLY, so remote mode showed errors where those panels are.
  it('does not call the local-only commands it used to call on mount', async () => {
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-section');
    await tick();
    await tick();
    for (const cmd of ['mcp_status', 'get_fleet_settings']) {
      expect(inv.mock.calls.some((c) => c[0] === cmd), cmd).toBe(false);
    }
  });

  it('replaces each of them with the reason instead of an error', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    for (const testid of ['projects-remote', 'automation-remote', 'limits-remote', 'mcp-remote']) {
      const note = await screen.findByTestId(testid);
      expect(note.textContent, testid).toContain('fleet.example.com');
    }
    // …and the controls are gone rather than present-but-broken.
    expect(screen.queryByTestId('projects-save')).toBeNull();
    expect(screen.queryByTestId('gc-enabled')).toBeNull();
    expect(screen.queryByTestId('mcp-enable')).toBeNull();
  });

  it('still renders Diagnostics, which is about THIS process either way', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect(await screen.findByTestId('copy-diagnostics')).toBeInTheDocument();
  });

  it('standalone is untouched: every panel is still there', async () => {
    hubStatus.set({ ...STANDALONE });
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'mcp_status')).toBe(true));
    expect(inv.mock.calls.some((c) => c[0] === 'get_fleet_settings')).toBe(true);
    expect(screen.getByTestId('projects-section')).toBeInTheDocument();
    expect(screen.getByTestId('automation-section')).toBeInTheDocument();
    expect(screen.queryByTestId('projects-remote')).toBeNull();
  });
});
