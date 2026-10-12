import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts } from './hosts';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { allDescriptors, bundle } from './pages/testing';
import { settingProposals, settingsWritable } from './pages/review';

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
  unavailable: null,
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
      case 'list_pages':
        return bundle;
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
    // The way to pair, and nothing that claims a state it is not in.
    expect(screen.getByTestId('hub-link').textContent).toContain('Link to a hub');
    // The link is a wizard (step 10.12): the two fields a pairing needs.
    await fireEvent.click(screen.getByTestId('hub-link'));
    expect(await screen.findByTestId('form-field-url')).toBeInTheDocument();
    expect(screen.getByTestId('form-field-code')).toBeInTheDocument();
  });

  it('tells the operator where the code comes from', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    const section = await screen.findByTestId('hub-section');
    expect(section.textContent).toContain('fleet-hub pair');
  });

  it('will not pair on an empty form', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('hub-link'));
    const pair = (await screen.findByTestId('form-submit')) as HTMLButtonElement;
    expect(pair).toBeDisabled();
  });
});

describe('pairing', () => {
  it('sends the URL and the code, and then asks for a restart', async () => {
    const inv = route({
      hub_pair: { ...remote, remote: false, restart_required: true },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('hub-link'));
    await fireEvent.input(await screen.findByTestId('form-field-url'), {
      target: { value: 'https://fleet.example.com' },
    });
    await fireEvent.input(screen.getByTestId('form-field-code'), { target: { value: 'ABCD1234' } });
    await fireEvent.click(screen.getByTestId('form-submit'));

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
    // The code worked once: the wizard closes rather than invite a retry.
    await waitFor(() => expect(screen.queryByTestId('wizard-link_hub')).toBeNull());
  });

  it('shows the hub’s own refusal rather than a generic failure', async () => {
    route({ hub_pair: ipcError('E_INVALID', 'that pairing code is unknown, used or expired') });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('hub-link'));
    await fireEvent.input(await screen.findByTestId('form-field-url'), {
      target: { value: 'https://fleet.example.com' },
    });
    await fireEvent.input(screen.getByTestId('form-field-code'), { target: { value: 'BADCODE1' } });
    await fireEvent.click(screen.getByTestId('form-submit'));
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
    await fireEvent.click(await screen.findByTestId('hub-link'));
    await fireEvent.input(await screen.findByTestId('form-field-url'), {
      target: { value: 'http://10.0.0.5:8787' },
    });
    await fireEvent.input(screen.getByTestId('form-field-code'), { target: { value: 'ABCD1234' } });

    // Before the refusal there is nothing to click: the opt-in is not a
    // checkbox someone can tick past without reading.
    expect(screen.queryByTestId('hub-allow-plaintext')).toBeNull();

    await fireEvent.click(screen.getByTestId('form-submit'));

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
    await fireEvent.click(screen.getByTestId('form-submit'));
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

  it('heads the section with last sync, the other devices and Unpair… (M15 G7.13)', async () => {
    const now = Math.floor(Date.now() / 1000);
    const inv = route({
      list_devices: [
        { name: 'laptop', mode: 'full', trusted: true, created_at: 1, catalogs: [], person_id: 1, last_seen_at: now - 4, this_device: true },
        { name: 'phone', mode: 'full', trusted: true, created_at: 1, catalogs: [], person_id: 1 },
        { name: 'tablet', mode: 'answer', trusted: true, created_at: 1, catalogs: [], person_id: 1 },
      ],
      hub_disconnect: { ...STANDALONE, restart_required: true },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    const head = await screen.findByTestId('hub-status-header');
    expect(head.textContent).toContain('Paired with https://fleet.example.com');
    await waitFor(() => expect(screen.getByTestId('hub-last-sync').textContent).toMatch(/^last sync \d+ s ago$/));
    expect(screen.getByTestId('hub-device-count').textContent).toBe('2 more of your devices');
    // Unpair… asks first; Cancel sends nothing, Unpair disconnects.
    await fireEvent.click(screen.getByTestId('hub-unpair'));
    await fireEvent.click(screen.getByTestId('hub-unpair-cancel'));
    expect(inv.mock.calls.some((c) => c[0] === 'hub_disconnect')).toBe(false);
    await fireEvent.click(screen.getByTestId('hub-unpair'));
    await fireEvent.click(screen.getByTestId('hub-unpair-confirm-btn'));
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'hub_disconnect')).toBe(true));
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

  // Requirement (a): the local-only ones are not called unprompted; the
  // fleet's settings are the hub's since declarative pages P6, so those are.
  it('does not call the local-only commands on mount, and reads the settings from the hub', async () => {
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-section');
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'describe_fleet_settings')).toBe(true));
    expect(inv.mock.calls.some((c) => c[0] === 'mcp_status')).toBe(false);
    for (const cmd of ['get_fleet_settings', 'setting_proposals']) {
      expect(inv.mock.calls.some((c) => c[0] === cmd), cmd).toBe(true);
    }
  });

  it('the control API panel is still the reason: it is the hub’s', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect((await screen.findByTestId('mcp-remote')).textContent).toContain('fleet.example.com');
    expect(screen.queryByTestId('mcp-enable')).toBeNull();
  });

  it('still renders Diagnostics, which is about THIS process either way', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect(await screen.findByTestId('copy-diagnostics')).toBeInTheDocument();
  });

  it('About, beside Diagnostics, opens with the Wordmark reveal (redesign 3.13)', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    const about = await screen.findByTestId('about-section');
    const mark = await screen.findByTestId('about-wordmark');
    expect(about.contains(mark)).toBe(true);
    expect(mark.dataset.loader).toBe('wordmark-reveal');
    expect(mark).toHaveAttribute('aria-label', 'Orbit Fleet');
    expect(screen.getByTestId('about-version')).toHaveTextContent('Orbit Fleet');
  });

  it('standalone is untouched: every panel is still there', async () => {
    hubStatus.set({ ...STANDALONE });
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'mcp_status')).toBe(true));
    expect(inv.mock.calls.some((c) => c[0] === 'get_fleet_settings')).toBe(true);
    expect(screen.getByTestId('projects-section')).toBeInTheDocument();
    expect(inv.mock.calls.some((c) => c[0] === 'describe_fleet_settings')).toBe(true);
    expect(screen.queryByTestId('projects-remote')).toBeNull();
    // Updates are a generated page since declarative pages P3/P6, not a
    // hand-written panel here: the note that this desktop installs nothing
    // yet is `crates/fleet-core/pages/settings.updates.json`'s notice, and
    // the tracks offered are pinned in the settings registry
    // (`nightly_is_a_track_once_it_is_published`).
    expect(screen.queryByTestId('update-section')).toBeNull();
    expect(screen.queryByTestId('update-remote')).toBeNull();
  });
});

// F1: a configured hub this launch could not use. The Hub section used to say
// "This app runs its own fleet" here — the opposite of the truth, and the
// Disconnect that could clear a leftover pairing was hidden.
describe('the Hub section, configured but unavailable', () => {
  const unavailable: HubStatus = {
    ...STANDALONE,
    configured_url: 'https://fleet.example.com',
    configured_client_name: 'laptop',
    warning: 'https://fleet.example.com is configured but no client token is stored',
    unavailable: 'https://fleet.example.com is configured but no client token is stored',
  };

  beforeEach(() => {
    hubStatus.set(unavailable);
  });

  it('says why, and does not claim this app runs its own fleet', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    const why = await screen.findByTestId('hub-unavailable-reason');
    expect(why.textContent).toContain('no saved pairing for that hub');
    expect(why.textContent).not.toContain('client token');
    expect(why.textContent).toContain('fleet.example.com');
    // The backend's raw reason stays one click away.
    expect(screen.getByTestId('hub-unavailable-detail').textContent).toContain(
      'no client token is stored',
    );
    expect(screen.queryByTestId('hub-empty')).toBeNull();
    expect(screen.queryByTestId('hub-connected')).toBeNull();
  });

  it('offers Disconnect, so a leftover pairing can always be cleared', async () => {
    const inv = route({
      hub_disconnect: { ...unavailable, configured_url: null, warning: null, restart_required: true },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('hub-disconnect'));
    await waitFor(() =>
      expect(inv.mock.calls.some((c) => c[0] === 'hub_disconnect')).toBe(true),
    );
    expect(await screen.findByTestId('hub-restart')).toBeInTheDocument();
  });

  it('offers to pair again, prefilled with the configured URL', async () => {
    route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect((await screen.findByTestId('hub-link')).textContent).toContain('Pair again');
    await fireEvent.click(screen.getByTestId('hub-link'));
    const url = (await screen.findByTestId('form-field-url')) as HTMLInputElement;
    expect(url.value).toBe('https://fleet.example.com');
    expect(screen.getByTestId('form-field-code')).toBeInTheDocument();
  });

  // This process runs no control API and no tick, and the backend refuses
  // the commands behind these panels — same as a working hub client.
  it('does not call the commands the backend refuses in this state', async () => {
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-section');
    await tick();
    await tick();
    for (const cmd of ['mcp_status', 'get_fleet_settings']) {
      expect(inv.mock.calls.some((c) => c[0] === cmd), cmd).toBe(false);
    }
  });
});

// A pairing that crashed on the OLD write order (token first, URL last) left a
// fleet-wide client token on this machine with no hub configured. No launch
// reads it — a blank URL is standalone, and resolution never queries the
// keychain there — so this section is the only place it is ever mentioned and
// the only place it can be cleared.
describe('a client token stranded by a half-finished pairing', () => {
  it('is found, explained and clearable from the standalone section', async () => {
    const inv = route({ hub_stranded_token: true, hub_disconnect: { ...STANDALONE } });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-section');

    const warning = await screen.findByTestId('hub-stranded-token');
    expect(warning.textContent).toContain('client token');
    // It must not read as a configured hub: this app still owns its fleet.
    expect(screen.getByTestId('hub-empty')).toBeInTheDocument();
    // And it must not promise a revocation it cannot perform.
    expect(warning.closest('section')!.textContent).toContain('does not revoke');

    await fireEvent.click(screen.getByTestId('hub-disconnect'));
    await waitFor(() =>
      expect(inv.mock.calls.some((c) => c[0] === 'hub_disconnect')).toBe(true),
    );
    // Cleared: the warning and its button go away without a reload.
    await waitFor(() => expect(screen.queryByTestId('hub-stranded-token')).toBeNull());
  });

  it('is not claimed when there is none', async () => {
    route({ hub_stranded_token: false });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-empty');
    await tick();
    await tick();
    expect(screen.queryByTestId('hub-stranded-token')).toBeNull();
    expect(screen.queryByTestId('hub-disconnect')).toBeNull();
  });

  // A keychain that will not open is not evidence of a leftover, and this app
  // is working normally otherwise. An error toast on every Settings open would
  // be noise about a state that almost certainly does not exist.
  it('says nothing, and raises nothing, when the token store cannot be read', async () => {
    route({ hub_stranded_token: ipcError('E_IO', 'keychain locked') });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-empty');
    await tick();
    await tick();
    expect(screen.queryByTestId('hub-stranded-token')).toBeNull();
    expect(screen.queryByTestId('hub-error')).toBeNull();
  });

  // With a hub configured the token belongs to it, Disconnect is already on
  // screen, and asking would only risk a keychain prompt for nothing.
  it('is not asked about at all once a hub is configured', async () => {
    hubStatus.set({ ...remote });
    const inv = route();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await screen.findByTestId('hub-connected');
    await tick();
    await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'hub_stranded_token')).toBe(false);
  });
});


// Declarative pages P6: a paired desktop's generated pages are the hub's
// settings — one line says so, a trusted device edits them, an untrusted one
// reads them, and a hub that serves none leaves its reason.
describe('the hub’s settings on a paired desktop (P6)', () => {
  const described = allDescriptors.map((d) => ({ ...d, value: d.key === 'playbooks.press_enter' ? 'true' : d.value }));

  beforeEach(() => {
    hubStatus.set(remote);
    settingProposals.set([]);
    settingsWritable.set(true);
  });

  it('a trusted device edits the hub’s values, through the hub', async () => {
    const inv = route({
      describe_fleet_settings: described,
      get_fleet_settings: { 'playbooks.press_enter': 'true' },
      setting_proposals: { can_write: true, proposals: [] },
      set_fleet_setting: { 'playbooks.press_enter': 'false' },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-automation'));
    const box = (await screen.findByTestId('setting-playbooks-press-enter')) as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(screen.getByTestId('hub-scope-note').textContent).toContain('paired as laptop');
    expect(screen.queryByTestId('hub-scope-readonly')).toBeNull();
    await fireEvent.click(box);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'playbooks.press_enter', value: 'false' }),
    );
    // Data items and page actions read this app's store: none on a paired desktop.
    expect(inv.mock.calls.some((c) => c[0] === 'fetch_page_source')).toBe(false);
  });

  it('Projects is the hub’s page, not this machine’s roots editor (7.1)', async () => {
    route({
      describe_fleet_settings: described,
      get_fleet_settings: {},
      setting_proposals: { can_write: true, proposals: [] },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-projects'));
    expect(await screen.findByTestId('page-settings.projects')).toBeInTheDocument();
    expect(screen.queryByTestId('projects-section')).toBeNull();
  });

  it('an untrusted device reads them, and is told how to be trusted', async () => {
    route({
      describe_fleet_settings: described,
      get_fleet_settings: { 'playbooks.press_enter': 'true' },
      setting_proposals: { can_write: false, proposals: [] },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-automation'));
    await screen.findByTestId('hub-scope-note');
    expect(screen.getByTestId('hub-scope-readonly').textContent).toContain('fleet-hub client trust laptop');
    // Shown, not editable: the value in words, no switch.
    expect(screen.getByTestId('setting-playbooks-press-enter').tagName).toBe('SPAN');
    expect(screen.getByTestId('setting-playbooks-press-enter').textContent).toBe('On');
  });

  it('Decisions on an untrusted paired desktop: every use case shown, none editable (7.7)', async () => {
    route({
      describe_fleet_settings: described.map((d) => (d.key === 'decide.jev.enabled' ? { ...d, value: 'true' } : d)),
      get_fleet_settings: { 'decide.jev.enabled': 'true' },
      setting_proposals: { can_write: false, proposals: [] },
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-decisions'));
    await screen.findByTestId('hub-scope-readonly');
    for (const k of ['enabled', 'status-map', 'start-project', 'work-link']) {
      expect((await screen.findByTestId(`setting-decide-jev-${k}`)).tagName, k).toBe('SPAN');
    }
    // The Today record reads this app's store: a paired desktop shows none.
    expect(screen.queryByTestId('data-record-decide.today')).toBeNull();
  });

  it('a hub that serves no settings leaves its reason and its answer', async () => {
    route({
      describe_fleet_settings: ipcError('E_FORBIDDEN', 'describe_fleet_settings: get_settings is not a client-callable tool'),
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-automation'));
    const note = await screen.findByTestId('pages-remote');
    await waitFor(() => expect(note.textContent).toContain('did not serve its settings'));
    expect(screen.getByTestId('pages-remote-error').textContent).toContain('not a client-callable tool');
    expect(screen.queryByTestId('setting-row-playbooks.press_enter')).toBeNull();
  });
});

// Assets M6 (R11): Settings → Catalogs on a paired desktop is the hub's
// catalogs, read-only, with the refusal — add and remove are the master's.
describe('Settings → Catalogs on a paired desktop', () => {
  it('lists the hub’s catalogs with no controls, and says to use the hub', async () => {
    hubStatus.set(remote);
    route({
      catalog_list_catalogs: [
        { id: 2, name: 'acme', org_id: 1, org: 'Acme', repo_path: '/r/acme', remote_url: null, head_commit: 'abc',
          last_loaded_at: 1, state: 'loaded', asset_count: 3, admitted: ['mefistos'], granted: ['laptop'] },
      ],
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-catalogs'));
    const reason = await screen.findByTestId('resource-readonly');
    expect(reason.textContent).toContain('fleet-hub catalog add');
    expect(reason.textContent).toContain('https://fleet.example.com');
    await screen.findByRole('option', { name: /acme/ });
    expect(screen.queryByTestId('resource-add')).toBeNull();
    expect(screen.queryByTestId('item-remove-admitted')).toBeNull();
  });
});
