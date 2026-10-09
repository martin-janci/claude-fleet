import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts, type HostRow } from './hosts';
import { toolkitTab } from './toolkit_skills';
import { destination } from './destination';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { registryRouter } from './pages/testing';
import { settingsSection } from './app_views';

/** On screen: present, and not in a hidden Settings panel (step 7.1 keeps
 *  every hand-written panel mounted and hides the ones another leaf is on). */
function shown(testid: string): boolean {
  const el = screen.queryByTestId(testid);
  return el !== null && el.closest('[hidden]') === null;
}

const sample: HostRow[] = [
  { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
  { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
];

const mcpStatusObj = {
  enabled: false,
  running: false,
  port: 4180,
  token: 'test-token',
  url: 'http://127.0.0.1:4180/mcp',
  bind_error: null,
  confirm_destructive: false,
};

const hostTokens = [
  { host_alias: 'mefistos', mode: 'full', created_at: 1 },
];

// Route invoke() by command name. The dialog calls mcp_status on mount, so an
// ordered mockResolvedValueOnce chain would be consumed by the wrong call —
// a routed implementation keeps each command's response stable.
beforeEach(() => {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'mcp_status':
      case 'mcp_configure':
        return mcpStatusObj;
      case 'discover_hosts':
        return [];
      case 'probe_host':
        return sample[1];
      case 'list_hosts':
        return sample;
      case 'list_host_tokens':
        return hostTokens;
      case 'set_host_token_mode':
        return { host_alias: 'mefistos', mode: 'readonly', created_at: 1 };
      case 'rotate_host_token':
        return { host_alias: 'mefistos', mode: 'full', created_at: 2 };
      default:
        return null;
    }
  });
  hosts.set(sample);
});

describe('SettingsDialog', () => {
  it('no longer renders the hosts table; one line summarises the hosts instead', async () => {
    hosts.set([
      ...sample,
      { alias: 'nas', ssh_alias: 'nas', reachable: false, claude_version: null, tmux_version: null, hidden: false, last_pinged_at: 1, account_uuid: null, provisioned: false, transport: 'ssh' },
    ]);
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    expect(screen.queryByTestId('hosts-table')).toBeNull();
    expect(document.querySelector('.hosts-table')).toBeNull();
    expect(screen.queryByTestId('settings-add-host')).toBeNull();
    const line = screen.getByTestId('settings-hosts-line');
    expect(line.textContent).toContain('Hosts');
    expect(screen.getByTestId('settings-hosts-summary').textContent).toBe('3 configured · 1 offline');
    expect(screen.getByTestId('settings-open-hosts').textContent).toMatch(/^Open Hosts (⌘I|Ctrl\+Shift\+H)$/);
  });

  it('Open Hosts closes Settings and requests the Hosts view', async () => {
    const { hostsViewRequest } = await import('./app_views');
    hostsViewRequest.set(null);
    const onClose = vi.fn();
    render(SettingsDialog, { props: { onClose } });
    await tick();
    await fireEvent.click(screen.getByTestId('settings-open-hosts'));
    expect(onClose).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(get(hostsViewRequest)).toEqual({ host: null }));
    hostsViewRequest.set(null);
  });

  it('provisioning refreshes the host-token cache the Hosts view reads', async () => {
    const { hostTokens: tokenCache } = await import('./host_actions');
    tokenCache.set(new Map());
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const routed = inv.getMockImplementation() as (cmd: string, ...rest: unknown[]) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, ...rest: unknown[]) => {
      if (cmd === 'mcp_status') return { ...mcpStatusObj, enabled: true, running: true };
      if (cmd === 'provision_hosts') return [];
      return routed(cmd, ...rest);
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    expect(inv.mock.calls.some((c) => c[0] === 'list_host_tokens')).toBe(false);
    const provision = (await screen.findByTestId('provision-hosts')) as HTMLButtonElement;
    await waitFor(() => expect(provision).not.toBeDisabled());
    await fireEvent.click(provision);
    await waitFor(() => expect(inv.mock.calls.some((c) => c[0] === 'list_host_tokens')).toBe(true));
    await waitFor(() => expect(get(tokenCache).get('mefistos')?.mode).toBe('full'));
  });

  it('renders the Control API section and toggling enable calls mcp_configure', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const section = await screen.findByTestId('mcp-section');
    expect(section.textContent).toContain('Control API');
    const toggle = screen.getByTestId('mcp-enable') as HTMLInputElement;
    await fireEvent.click(toggle);
    await tick();
    expect(
      (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
        (c) => c[0] === 'mcp_configure',
      ),
    ).toBe(true);
  });

  it('toggling destructive-call confirmation calls mcp_configure with confirm_destructive', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const toggle = (await screen.findByTestId('mcp-confirm-destructive')) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    await fireEvent.click(toggle);
    await tick();
    const call = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find(
      (c) => c[0] === 'mcp_configure',
    );
    expect(call).toBeDefined();
    expect((call![1] as { args: { confirm_destructive: boolean } }).args.confirm_destructive).toBe(true);
  });
});

describe('SettingsDialog notifications (W2 Track D)', () => {
  it('renders the notifications section with the toast toggle on and OS off', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    expect(screen.getByTestId('notifications-section')).toBeInTheDocument();
    expect(screen.getByTestId('notify-toast')).toBeChecked();
    expect(screen.getByTestId('notify-os')).not.toBeChecked();
    // jsdom has no Notification API ⇒ the OS toggle is disabled + labelled.
    expect(screen.getByTestId('notify-permission')).toHaveTextContent('unsupported');
    expect(screen.getByTestId('notify-os')).toBeDisabled();
  });
});

describe('SettingsDialog projects (W5 G3)', () => {
  const resolved = JSON.stringify({ local: '/home/u/projects/github.com', mefistos: '~/projects/github.com' });
  function routeProjects(extra: Record<string, unknown> = {}) {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      switch (cmd) {
        case 'mcp_status':
          return mcpStatusObj;
        case 'get_fleet_settings':
          return {
            'projects.base_path': '{}',
            'projects.layout': 'github',
            'projects.resolved_base': resolved,
            'projects.local_env_base': '',
            ...extra,
          };
        case 'set_fleet_setting':
          return { [args!.key!]: args!.value!, 'projects.resolved_base': resolved };
        case 'refresh_projects':
          return [];
        default:
          return null;
      }
    });
    return inv;
  }

  // onMount loads settings, then seeds the drafts; the inputs stay disabled
  // until then, so "enabled" is the deterministic ready signal.
  async function ready() {
    await waitFor(() => expect(screen.getByTestId('projects-base-local')).not.toBeDisabled());
    await tick();
  }

  it('previews the resolved root per host with no setting stored', async () => {
    routeProjects();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    expect(screen.getByTestId('projects-section')).toBeInTheDocument();
    expect(screen.getByTestId('projects-preview-local')).toHaveTextContent('~/projects/github.com/<owner>/<repo>');
    expect(screen.getByTestId('projects-preview-mefistos')).toHaveTextContent('~/projects/github.com/<owner>/<repo>');
    expect(screen.getByTestId('projects-preview-local')).not.toHaveClass('err');
    expect((screen.getByTestId('projects-base-mefistos') as HTMLInputElement).value).toBe('');
  });

  it('saves the per-host map and layout, then rescans projects', async () => {
    const inv = routeProjects();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    await fireEvent.input(screen.getByTestId('projects-base-mefistos'), { target: { value: ' ~/code ' } });
    await fireEvent.change(screen.getByTestId('projects-layout'), { target: { value: 'flat' } });
    await tick();
    expect(screen.getByTestId('projects-preview-mefistos')).toHaveTextContent('~/code/<repo>');
    await fireEvent.click(screen.getByTestId('projects-save'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('refresh_projects', undefined));
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'projects.base_path', value: '{"mefistos":"~/code"}' });
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'projects.layout', value: 'flat' });
    expect(inv).toHaveBeenCalledWith('refresh_projects', undefined);
  });

  it('flags an invalid path and disables Save', async () => {
    const inv = routeProjects();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    await fireEvent.input(screen.getByTestId('projects-base-local'), { target: { value: 'relative/dir' } });
    await tick();
    expect(screen.getByTestId('projects-preview-local')).toHaveTextContent('must be absolute');
    // The error message is styled red through `.project-preview.err`; without
    // that qualifier `.hook-desc`'s muted colour wins the specificity tie.
    expect(screen.getByTestId('projects-preview-local')).toHaveClass('err');
    expect(screen.getByTestId('projects-save')).toBeDisabled();
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.objectContaining({ key: 'projects.base_path' }));
  });

  it('pre-fills a stored per-host path', async () => {
    routeProjects({ 'projects.base_path': '{"mefistos":"/data/git"}', 'projects.layout': 'flat' });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    expect((screen.getByTestId('projects-base-mefistos') as HTMLInputElement).value).toBe('/data/git');
    expect((screen.getByTestId('projects-layout') as HTMLSelectElement).value).toBe('flat');
    expect(screen.getByTestId('projects-preview-mefistos')).toHaveTextContent('/data/git/<repo>');
  });

  it('local preview follows an unsaved layout change (no env var)', async () => {
    routeProjects();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    await fireEvent.change(screen.getByTestId('projects-layout'), { target: { value: 'flat' } });
    await tick();
    expect(screen.getByTestId('projects-preview-local')).toHaveTextContent('~/projects/<repo>');
    expect(screen.getByTestId('projects-preview-local')).not.toHaveTextContent('github.com');
  });

  it('local preview uses the env var before the layout default', async () => {
    routeProjects({ 'projects.local_env_base': '/srv/env' });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await ready();
    expect(screen.getByTestId('projects-preview-local')).toHaveTextContent('/srv/env/<owner>/<repo>');
    await fireEvent.change(screen.getByTestId('projects-layout'), { target: { value: 'flat' } });
    await tick();
    expect(screen.getByTestId('projects-preview-local')).toHaveTextContent('/srv/env/<repo>');
    // remote hosts never see the env var
    expect(screen.getByTestId('projects-preview-mefistos')).toHaveTextContent('~/projects/<repo>');
  });

});

describe('SettingsDialog composer link', () => {
  it('sends the chip editor to Toolkit › Prompts & snippets and closes Settings', async () => {
    destination.set('session');
    toolkitTab.set('skills');
    const onClose = vi.fn();
    render(SettingsDialog, { props: { onClose } });
    await tick();
    expect(screen.queryByTestId('preset-label')).toBeNull();
    await fireEvent.click(screen.getByTestId('composer-open-toolkit'));
    expect(get(toolkitTab)).toBe('prompts');
    expect(get(destination)).toBe('assets');
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

describe('SettingsDialog — generated pages (declarative pages P3)', () => {
  function routeRegistry(initial: Record<string, string> = {}) {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const base = inv.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    const router = registryRouter(initial, (cmd, a) => base(cmd, a));
    inv.mockImplementation(router.impl);
    return inv;
  }
  afterEach(() => fleetSettings.set({ ...SETTING_DEFAULTS }));

  it('opens on Appearance, lists the tree, and renders a page from the registry', async () => {
    const inv = routeRegistry();
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect(await screen.findByTestId('settings-nav-automation')).toBeInTheDocument();
    expect(screen.getByTestId('settings-nav-appearance').getAttribute('aria-current')).toBe('page');
    expect(shown('settings-panel-appearance')).toBe(true);
    expect(shown('hub-section')).toBe(false);
    await fireEvent.click(screen.getByTestId('settings-nav-hub'));
    expect(screen.getByTestId('settings-nav-hub').getAttribute('aria-current')).toBe('page');
    expect(shown('hub-section')).toBe(true);
    // The app-vs-hub versions line is about a pairing; standalone there is
    // one program and the footer already names its version.
    expect(screen.queryByTestId('hub-versions')).toBeNull();
    for (const id of ['advanced', 'limits', 'work', 'decisions', 'usage', 'voice', 'downloads', 'playbooks']) {
      expect(screen.getByTestId(`settings-nav-${id}`)).toBeInTheDocument();
    }
    await waitFor(() => expect(inv).toHaveBeenCalledWith('describe_fleet_settings', undefined));
    await fireEvent.click(screen.getByTestId('settings-nav-automation'));
    expect(await screen.findByTestId('page-settings.automation')).toBeInTheDocument();
    expect(shown('hub-section')).toBe(false);
    const press = screen.getByTestId('setting-playbooks-press-enter') as HTMLInputElement;
    expect(press.checked).toBe(false);
    await fireEvent.click(press);
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'playbooks.press_enter', value: 'true' }),
    );
    // The write's answer refreshes the value the page shows.
    await waitFor(() => expect((screen.getByTestId('setting-playbooks-press-enter') as HTMLInputElement).checked).toBe(true));
  });

  it('Decisions: off by default, says what is sent where, offers no auto mode, and asks before turning on', async () => {
    const inv = routeRegistry();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await fireEvent.click(await screen.findByTestId('settings-nav-decisions'));
    const page = await screen.findByTestId('page-settings.decisions');
    await waitFor(() => expect(screen.getByTestId('setting-decide-jev-enabled')).toBeInTheDocument());
    expect((screen.getByTestId('setting-decide-jev-enabled') as HTMLInputElement).checked).toBe(false);
    expect(page.textContent).toContain('only for organisations that opted in');
    // The feature modes only show once the kill switch is on.
    expect(screen.queryByTestId('setting-row-decide.jev.work_link')).toBeNull();
    await fireEvent.click(screen.getByTestId('setting-decide-jev-enabled'));
    expect((await screen.findByTestId('confirm-dialog')).textContent).toContain('sent to TypeSafe');
    await fireEvent.click(screen.getByTestId('setting-confirm-decide.jev.enabled'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'decide.jev.enabled', value: 'true' }),
    );
    // Work link is shown, not edited: J1 is an offline benchmark until it
    // passes its acceptance lines (D32).
    const wl = await screen.findByTestId('setting-decide-jev-work-link');
    expect(wl.tagName).toBe('SPAN');
    expect(screen.getByTestId('setting-row-decide.jev.work_link').textContent).toContain('offline benchmark only until J1');
    const sm = screen.getByTestId('setting-decide-jev-status-map') as HTMLSelectElement;
    expect(Array.from(sm.querySelectorAll('option'), (o) => o.value)).toEqual(['off', 'shadow', 'assist']);
    // Step 7.7: each use case its own row, today's budget, the breaker, the
    // key and which orgs consent, with the way to change that consent.
    for (const k of ['status_map', 'start_project', 'sibling_repos', 'quick_answer', 'duplicate', 'work_placement', 'related_session', 'work_link']) {
      const sel = screen.queryByTestId(`setting-decide-jev-${k.replace('_', '-')}`) as HTMLSelectElement | null;
      expect(sel, k).not.toBeNull();
      if (sel?.tagName === 'SELECT') expect(Array.from(sel.options, (o) => o.value)).not.toContain('auto');
    }
    expect(screen.getByTestId('section-Use cases').textContent).toContain('There is no auto mode');
    const today = await screen.findByTestId('section-Today');
    await waitFor(() => expect(today.textContent).toContain('closed'));
    expect(today.textContent).toContain('Acme');
    expect(today.textContent).toContain('Daily budget');
    expect(screen.getByTestId('page-link-settings.orgs').textContent).toContain('which organisations allow Jev');
  });

  it('search finds a setting and opens it on its page and tab', async () => {
    routeRegistry();
    render(SettingsDialog, { props: { onClose: () => {} } });
    const search = (await screen.findByTestId('settings-search')) as HTMLInputElement;
    await waitFor(() => expect(screen.getByTestId('settings-nav-work')).toBeInTheDocument());
    await fireEvent.input(search, { target: { value: 'unlinked' } });
    await fireEvent.click(await screen.findByTestId('settings-hit-work.tidy_idle_unlinked_days'));
    const row = await screen.findByTestId('setting-row-work.tidy_idle_unlinked_days');
    await waitFor(() => expect(row.classList.contains('highlighted')).toBe(true));
    expect(screen.getByTestId('page-settings.work-tab-1').getAttribute('aria-selected')).toBe('true');
    // The hits stay up until the search is cleared; then the tree marks the leaf.
    await fireEvent.input(search, { target: { value: '' } });
    expect(screen.getByTestId('settings-nav-work').getAttribute('aria-current')).toBe('page');
  });

  it('a hit in a section with its own leaf opens that leaf, and only that section', async () => {
    routeRegistry();
    render(SettingsDialog, { props: { onClose: () => {} } });
    const search = (await screen.findByTestId('settings-search')) as HTMLInputElement;
    await fireEvent.input(search, { target: { value: 'voice.max_capture_secs' } });
    await fireEvent.click(await screen.findByTestId('settings-hit-voice.max_capture_secs'));
    await fireEvent.input(search, { target: { value: '' } });
    expect(screen.getByTestId('settings-nav-voice').getAttribute('aria-current')).toBe('page');
    const page = await screen.findByTestId('page-settings.limits');
    expect(screen.getByTestId('section-Voice')).toBeInTheDocument();
    expect(screen.queryByTestId('section-Downloads')).toBeNull();
    expect(page.querySelector('h4')?.textContent).toBe('Voice');
    // The whole page is one click away, on the Limits leaf.
    await fireEvent.click(screen.getByTestId('page-whole-link'));
    expect(screen.getByTestId('settings-nav-limits').getAttribute('aria-current')).toBe('page');
    expect(await screen.findByTestId('section-Downloads')).toBeInTheDocument();
  });

  it('each tree leaf shows its own screen, and the old section names still open', async () => {
    routeRegistry();
    settingsSection.set('diagnostics');
    render(SettingsDialog, { props: { onClose: () => {} } });
    await waitFor(() => expect(screen.getByTestId('settings-nav-error-reports').getAttribute('aria-current')).toBe('page'));
    expect(shown('diagnostics-section')).toBe(true);
    expect(await screen.findByTestId('section-Error reports')).toBeInTheDocument();
    for (const [leaf, testid] of [
      ['notifications', 'notifications-section'],
      ['shortcuts', 'shortcuts-section'],
      ['sessions', 'composer-section'],
      ['projects', 'projects-section'],
      ['work', 'work-section'],
      ['control-api', 'mcp-section'],
      ['accounts-hosts', 'settings-hosts-line'],
      ['appearance', 'onboarding-section'],
    ] as const) {
      await fireEvent.click(screen.getByTestId(`settings-nav-${leaf}`));
      expect(shown(testid), leaf).toBe(true);
      expect(shown('diagnostics-section'), leaf).toBe(false);
    }
  });

  it('a deep link to a page opens it', async () => {
    routeRegistry();
    settingsSection.set('settings.limits');
    render(SettingsDialog, { props: { onClose: () => {} } });
    expect(await screen.findByTestId('page-settings.limits')).toBeInTheDocument();
    expect(get(settingsSection)).toBeNull();
  });
});
