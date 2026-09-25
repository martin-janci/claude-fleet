import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsDialog from './SettingsDialog.svelte';
import { hosts, type HostRow } from './hosts';
import { composerPresets, resetComposerPresets, DEFAULT_PRESETS } from './composer_presets';

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

describe('SettingsDialog automation + notifications (W2 Track D)', () => {
  it('renders the playbook and GC controls off by default', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    expect(screen.getByTestId('automation-section')).toBeInTheDocument();
    expect(screen.getByTestId('playbook-press-enter')).not.toBeChecked();
    expect(screen.getByTestId('playbook-oom-recreate')).not.toBeChecked();
    expect(screen.getByTestId('gc-enabled')).not.toBeChecked();
    expect((screen.getByTestId('gc-bg-hours') as HTMLInputElement).value).toBe('24');
    expect((screen.getByTestId('gc-shell-hours') as HTMLInputElement).value).toBe('168');
    expect((screen.getByTestId('gc-work-hours') as HTMLInputElement).value).toBe('0');
    expect(mockedInvoke).toHaveBeenCalledWith('get_fleet_settings', undefined);
  });

  it('toggling a playbook writes the setting through set_fleet_setting', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('playbook-press-enter'));
    await tick();
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'playbooks.press_enter', value: 'true' });
    expect(screen.getByTestId('playbook-press-enter')).toBeChecked();
  });

  it('GC TTL inputs convert hours to seconds on the wire', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const input = screen.getByTestId('gc-bg-hours') as HTMLInputElement;
    input.value = '1.5';
    await fireEvent.change(input);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.bg_idle_secs', value: '5400' });
  });

  it('renders the task TTL and move cap rows with the backend defaults and bounds', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    expect(screen.getByTestId('limits-section')).toBeInTheDocument();
    const tasks = screen.getByTestId('tasks-max-age-hours') as HTMLInputElement;
    expect(tasks.value).toBe('24');
    expect(tasks.min).toBe('0');
    expect(tasks.max).toBe('87600'); // settings::MAX_SECS in hours
    const mb = screen.getByTestId('move-max-transcript-mb') as HTMLInputElement;
    expect(mb.value).toBe('200');
    expect(mb.min).toBe('1');
    expect(mb.max).toBe('4096');
  });

  it('writes the task TTL in seconds and the move cap as typed', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const tasks = screen.getByTestId('tasks-max-age-hours') as HTMLInputElement;
    tasks.value = '2';
    await fireEvent.change(tasks);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'tasks.max_age_secs', value: '7200' });
    const mb = screen.getByTestId('move-max-transcript-mb') as HTMLInputElement;
    mb.value = '64';
    await fireEvent.change(mb);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'move.max_transcript_mb', value: '64' });
  });

  it('shows the backend validation error for an out-of-range move cap', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'set_fleet_setting') {
        throw { code: 'E_INVALID', message: 'move.max_transcript_mb must be an integer between 1 and 4096' };
      }
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const mb = screen.getByTestId('move-max-transcript-mb') as HTMLInputElement;
    mb.value = '5000';
    await fireEvent.change(mb);
    // Sent as typed: the backend is the authority on the range.
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'move.max_transcript_mb', value: '5000' });
    await waitFor(() =>
      expect(screen.getByTestId('limits-error').textContent).toContain('between 1 and 4096'),
    );
  });

  it('never sends a limit it cannot represent, and says why', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const tasks = screen.getByLabelText('tasks') as HTMLInputElement;
    const mb = screen.getByLabelText('move') as HTMLInputElement;
    expect(tasks).toBe(screen.getByTestId('tasks-max-age-hours'));
    expect(mb).toBe(screen.getByTestId('move-max-transcript-mb'));
    const cases: [HTMLInputElement, string, RegExp][] = [
      // -1 h must not be clamped to 0 s ("never").
      [tasks, '-1', /Task timeout: hours must be 0 or more/],
      // A positive value that rounds to 0 s would also mean "never".
      [tasks, '0.00001', /Task timeout: too small/],
      [tasks, '', /Task timeout: enter a number of hours/],
      [mb, '', /Move transcript cap: enter a whole number/],
      [mb, '1.5', /Move transcript cap: "1.5" is not a whole number/],
    ];
    for (const [input, value, message] of cases) {
      input.value = value;
      await fireEvent.change(input);
      await tick();
      expect(screen.getByRole('alert').textContent).toMatch(message);
    }
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());
  });

  it('renders the usage rows with the backend defaults', async () => {
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const enabled = screen.getByLabelText('usage') as HTMLInputElement;
    expect(enabled).toBe(screen.getByTestId('usage-enabled'));
    expect(enabled).toBeChecked();
    const interval = screen.getByLabelText('usage every') as HTMLInputElement;
    expect(interval).toBe(screen.getByTestId('usage-interval-secs'));
    expect(interval.value).toBe('300');
    const prices = screen.getByLabelText('prices') as HTMLTextAreaElement;
    expect(prices).toBe(screen.getByTestId('usage-prices-json'));
    expect(prices.value).toBe('{}');
  });

  it('writes the usage settings through set_fleet_setting', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string, args?: { key?: string; value?: string }) => {
      if (cmd === 'set_fleet_setting') return { [args!.key!]: args!.value! };
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    await fireEvent.click(screen.getByTestId('usage-enabled'));
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'usage.enabled', value: 'false' });
    const interval = screen.getByTestId('usage-interval-secs') as HTMLInputElement;
    interval.value = '600';
    await fireEvent.change(interval);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'usage.interval_secs', value: '600' });
    const prices = screen.getByTestId('usage-prices-json') as HTMLTextAreaElement;
    const json = '{"opus-4-1":{"input":15,"output":75,"cache_write":30,"cache_read":1.5}}';
    prices.value = ` ${json} `;
    await fireEvent.change(prices);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'usage.prices_json', value: json });
  });

  it('refuses prices that are not a JSON object and shows the backend error for a bad one', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'set_fleet_setting') {
        throw { code: 'E_INVALID', message: 'usage.prices_json: prices for opus must be between 0 and 10000' };
      }
      if (cmd === 'mcp_status') return mcpStatusObj;
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick(); await tick();
    const prices = screen.getByTestId('usage-prices-json') as HTMLTextAreaElement;
    for (const [value, message] of [
      ['{oops', /Usage prices: not valid JSON/],
      ['[1, 2]', /Usage prices: must be a JSON object/],
    ] as const) {
      prices.value = value;
      await fireEvent.change(prices);
      await tick();
      expect(screen.getByRole('alert').textContent).toMatch(message);
    }
    expect(inv).not.toHaveBeenCalledWith('set_fleet_setting', expect.anything());
    // Object-shaped but invalid: sent as typed, the backend's message shows.
    const bad = '{"opus":{"input":-1,"output":1,"cache_write":1,"cache_read":1}}';
    prices.value = bad;
    await fireEvent.change(prices);
    expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'usage.prices_json', value: bad });
    await waitFor(() =>
      expect(screen.getByTestId('limits-error').textContent).toContain('between 0 and 10000'),
    );
  });

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

  it('lists the composer presets and edits them in place', async () => {
    resetComposerPresets();
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    const section = screen.getByTestId('composer-section');
    expect(section.textContent).toContain('Conversation composer');
    const labels = screen.getAllByTestId('preset-label') as HTMLInputElement[];
    expect(labels).toHaveLength(DEFAULT_PRESETS.length);
    expect(labels[0].value).toBe(DEFAULT_PRESETS[0].label);

    await fireEvent.input(labels[0], { target: { value: 'Wipe' } });
    expect(get(composerPresets)[0].label).toBe('Wipe');
    const texts = screen.getAllByTestId('preset-text') as HTMLTextAreaElement[];
    await fireEvent.input(texts[0], { target: { value: '/clear now' } });
    expect(get(composerPresets)[0].text).toBe('/clear now');

    await fireEvent.click(screen.getByTestId('preset-add'));
    expect(get(composerPresets)).toHaveLength(DEFAULT_PRESETS.length + 1);
    expect(screen.getAllByTestId('preset-label')).toHaveLength(DEFAULT_PRESETS.length + 1);

    await fireEvent.click(screen.getAllByTestId('preset-remove')[0]);
    expect(get(composerPresets)[0].label).toBe(DEFAULT_PRESETS[1].label);

    await fireEvent.click(screen.getByTestId('preset-reset'));
    expect(get(composerPresets)).toEqual(DEFAULT_PRESETS);
  });
});

describe('SettingsDialog — Work lifecycle (work graph M7.3)', () => {
  it('shows the thresholds, auto-tidy off with its warning, and a dry run of the current candidates', async () => {
    const inv = mockedInvoke as ReturnType<typeof vi.fn>;
    const base = inv.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a?: unknown) => {
      if (cmd === 'work_tidy')
        return {
          candidates: [
            { session_id: 1, host_alias: 'h', tmux_name: 'done-one', reason: 'done_idle', action: 'safe_kill', since: 0, idle_secs: 18000, key: 'ABC-1' },
            { session_id: 2, host_alias: 'h', tmux_name: 'dup', reason: 'duplicate_worktree', action: 'kill', since: 0, idle_secs: 90000 },
            { session_id: 3, host_alias: 'h', tmux_name: 'wontdo', reason: 'not_planned', action: 'safe_kill', since: 0, idle_secs: 18000 },
          ],
          auto_tidy: false,
          auto_reasons: ['done_idle', 'pr_merged_idle'],
          done_days: 2,
          idle_hours: 4,
        };
      if (cmd === 'work_reopened') return [];
      return base(cmd, a);
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    await tick();
    expect((screen.getByTestId('work-tidy-done-days') as HTMLInputElement).value).toBe('2');
    expect((screen.getByTestId('work-tidy-idle-hours') as HTMLInputElement).value).toBe('4');
    expect((screen.getByTestId('work-auto-tidy') as HTMLInputElement).checked).toBe(false);
    expect(screen.getByTestId('work-auto-tidy-warning').textContent).toContain('never touched');
    const reason = (r: string) => screen.getByTestId(`work-auto-tidy-reason-${r}`) as HTMLInputElement;
    expect([reason('done_idle').checked, reason('pr_merged_idle').checked, reason('not_planned').checked]).toEqual([
      true,
      true,
      false,
    ]);
    await fireEvent.click(screen.getByTestId('work-auto-tidy-dry-run'));
    const preview = await screen.findByTestId('work-auto-tidy-preview');
    const rows = screen.getAllByTestId('work-auto-tidy-preview-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('done-one');
    expect(preview.textContent).toContain('once turned on');
    // Ticking a reason writes the comma list.
    await fireEvent.click(reason('not_planned'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', {
        key: 'work.auto_tidy_reasons',
        value: 'done_idle,pr_merged_idle,not_planned',
      }),
    );
  });
});
