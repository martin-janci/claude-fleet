import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
import { approvable, commandRows, keepsKind, paletteCommands, runCommand, settingRow, splitPrefix } from './commands';
import { applyTheme, theme } from './theme';
import { allDescriptors } from './pages/testing';
import { session } from './hosts_fixture';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { destination } from './destination';
import type { PendingInput } from './pending_input';
import type { SessionRow } from './sessions';
import { toasts } from './toasts';

// Redesign step 3.9: the ⌘K command registry.

const PERMISSION: PendingInput = {
  kind: 'permission',
  question: 'Push to origin?',
  options: [
    { n: 1, label: 'Yes', selected: true },
    { n: 2, label: "Yes, and don't ask again", selected: false },
    { n: 3, label: 'No', selected: false },
  ],
};
const asking = (over: Partial<SessionRow> = {}) =>
  session('mac', 'fix-flake', { claude_status: 'blocked', pending_input: PERMISSION, ...over });

describe('prefixes', () => {
  it('> commands, # tasks and tickets, @ hosts; anything else is everything', () => {
    expect(splitPrefix('> pause')).toEqual({ mode: 'commands', rest: 'pause' });
    expect(splitPrefix('#PD-12')).toEqual({ mode: 'work', rest: 'PD-12' });
    expect(splitPrefix('@mac')).toEqual({ mode: 'hosts', rest: 'mac' });
    expect(splitPrefix('blue mef')).toEqual({ mode: 'all', rest: 'blue mef' });
    expect(keepsKind('commands', 'setting')).toBe(true);
    expect(keepsKind('commands', 'session')).toBe(false);
    expect(keepsKind('work', 'lookup')).toBe(true);
    expect(keepsKind('hosts', 'host')).toBe(true);
    expect(keepsKind('hosts', 'project')).toBe(false);
  });
});

describe('palette commands', () => {
  it('offers Approve only for a one-key permission dialog, naming what it presses', () => {
    expect(approvable(asking())?.options[0].label).toBe('Yes');
    const cmds = paletteCommands({ selected: asking(), sessionView: 'conversation' });
    const approve = cmds.find((c) => c.id === 'session.approve');
    expect(approve?.label).toBe('Approve: Yes');
    expect(approve?.section).toBe('This session');
    // A question that is not a permission, a multi-select, a ghost: no Approve.
    expect(approvable(asking({ pending_input: { ...PERMISSION, kind: 'question' } as unknown as PendingInput }))).toBeNull();
    expect(approvable(asking({ status: 'ghost' }))).toBeNull();
    expect(approvable(session('mac', 'idle'))).toBeNull();
  });

  it('session commands need an open session; app commands are always there', () => {
    const none = paletteCommands({ selected: null, sessionView: 'conversation' }).map((c) => c.id);
    expect(none).not.toContain('session.flip-view');
    expect(none).toEqual(expect.arrayContaining(['app.settings', 'app.hosts', 'app.pause-all', 'app.shortcuts']));
    const flip = paletteCommands({ selected: session('mac', 'a'), sessionView: 'terminal' }).find(
      (c) => c.id === 'session.flip-view',
    );
    expect(flip?.label).toBe('Show the conversation');
  });

  it('the theme command names the theme it switches to, in the sidebar toggle order', async () => {
    applyTheme('auto');
    const label = () => paletteCommands({ selected: null, sessionView: 'conversation' }).find((c) => c.id === 'app.theme')?.label;
    expect(label()).toBe('Theme: light');
    await runCommand('app.theme', { selected: null, sessionView: 'conversation' });
    expect(label()).toBe('Theme: dark');
    applyTheme('auto');
  });

  it("each row shows its chord from the shortcut registry, in the platform's spelling", () => {
    const cmds = paletteCommands({ selected: null, sessionView: 'conversation' });
    const meta = (isMac: boolean, id: string) => commandRows(cmds, isMac).find((r) => r.action === id)?.meta;
    expect(meta(true, 'app.settings')).toBe('⌘,');
    expect(meta(false, 'app.hosts')).toBe('Ctrl+Shift+H');
    expect(meta(true, 'app.shortcuts')).toBe('?');
    expect(meta(true, 'app.pause-all')).toBe('Commands');
  });
});

// Parity P11: the ⌘K theme command is shared by both layouts.
describe('palette commands in the New layout', () => {
  afterEach(() => {
    applyTheme('auto');
  });

  it('New layout: the theme command names the theme it switches to, and switches it', async () => {
    applyTheme('auto');
    const ctx = { selected: null, sessionView: 'conversation' as const };
    const label = () => paletteCommands(ctx).find((c) => c.id === 'app.theme')?.label;
    expect(label()).toBe('Theme: light');
    await runCommand('app.theme', ctx);
    expect(get(theme)).toBe('light');
    expect(label()).toBe('Theme: dark');
    await runCommand('app.theme', ctx);
    expect(get(theme)).toBe('dark');
  });
});

describe('settings in plain words', () => {
  it('a change this client may write becomes one row; a search does not', () => {
    const row = settingRow('set recent work to 3 days', allDescriptors, true);
    expect(row?.kind).toBe('setting');
    expect(row?.setting).toMatchObject({ key: 'work.recent_days', value: '3', confirm: false });
    expect(row?.label).toMatch(/^Set .* to /);
    expect(settingRow('recent work', allDescriptors, true)).toBeNull();
    expect(settingRow('set recent work to 3 days', allDescriptors, false)).toBeNull();
  });
});

describe('Automation commands (8.4)', () => {
  afterEach(() => {
    fleetSettings.set({ ...SETTING_DEFAULTS });
    destination.set('session');
  });

  it('Open Automation goes there; the pause command names what it will do', async () => {
    const ctx = { selected: null, sessionView: 'conversation' as const };
    await runCommand('app.automation', ctx);
    expect(get(destination)).toBe('automation');
    const label = () => paletteCommands(ctx).find((c) => c.id === 'app.automation-pause')?.label;
    expect(label()).toBe('Pause all automation');
    fleetSettings.set({ ...SETTING_DEFAULTS, 'automation.paused': 'true' });
    expect(label()).toBe('Resume automation');
    // Pause all missions stays as it was.
    expect(paletteCommands(ctx).find((c) => c.id === 'app.pause-all')?.label).toBe('Pause all missions');
  });
});

describe('session commands: push and open in editor (3.9, 5.5)', () => {
  beforeEach(() => {
    invoke.mockReset();
    toasts.set([]);
  });

  it('both are offered for an open session, Open in VS Code with its chord', () => {
    const ids = (sel: SessionRow | null) => paletteCommands({ selected: sel, sessionView: 'conversation' }).map((c) => c.id);
    expect(ids(session('mac', 'a'))).toEqual(expect.arrayContaining(['session.push', 'session.open-in-editor']));
    expect(ids(null)).not.toContain('session.push');
    expect(ids(session('mac', 'a', { status: 'ghost' }))).not.toContain('session.open-in-editor');
    const cmds = paletteCommands({ selected: session('mac', 'a'), sessionView: 'conversation' });
    expect(commandRows(cmds, true).find((r) => r.action === 'session.open-in-editor')?.meta).toBe('⌘⇧E');
  });

  it('push runs repo_push on the session; a failure is a toast', async () => {
    const s = session('mac', 'fix-flake');
    invoke.mockResolvedValue(null);
    await runCommand('session.push', { selected: s, sessionView: 'conversation' });
    expect(invoke).toHaveBeenCalledWith('repo_push', { args: { session_id: s.id, set_upstream: false } });
    expect(get(toasts).at(-1)?.message).toContain('Pushed');
    invoke.mockRejectedValue({ code: 'E_GIT', message: 'no upstream' });
    await runCommand('session.push', { selected: s, sessionView: 'conversation' });
    expect(get(toasts).at(-1)?.message).toContain('no upstream');
  });

  it('open in editor asks the backend; a session without a pane says why instead', async () => {
    const s = session('mac', 'fix-flake');
    invoke.mockResolvedValue(null);
    await runCommand('session.open-in-editor', { selected: s, sessionView: 'conversation' });
    expect(invoke).toHaveBeenCalledWith('open_session_in_editor', {
      args: { host_alias: 'mac', tmux_name: s.tmux_name },
    });
    invoke.mockReset();
    await runCommand('session.open-in-editor', { selected: { ...s, kind: 'bg' }, sessionView: 'conversation' });
    expect(invoke).not.toHaveBeenCalled();
    expect(get(toasts).at(-1)?.message).toContain('outside tmux');
  });
});
