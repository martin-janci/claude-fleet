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

  it('offers one row per other theme, by the names Settings uses (review w20-11)', async () => {
    applyTheme('sun');
    const rows = () => paletteCommands({ selected: null, sessionView: 'conversation' }).filter((c) => c.id.startsWith('app.theme.'));
    expect(rows().map((c) => c.label)).toEqual(['Theme: System', 'Theme: Light', 'Theme: Dark']);
    expect(rows()[0].description).toBe('Now Sun');
    // "dark" from Sun is one run, not three.
    const dark = rows().find((c) => c.synonyms.includes('night'))!;
    await runCommand(dark.id, { selected: null, sessionView: 'conversation' });
    expect(get(theme)).toBe('dark');
    expect(rows().map((c) => c.label)).toEqual(['Theme: Sun', 'Theme: System', 'Theme: Light']);
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

  it('New layout: a theme row names its theme and switches to it', async () => {
    applyTheme('auto');
    const ctx = { selected: null, sessionView: 'conversation' as const };
    const ids = () => paletteCommands(ctx).filter((c) => c.id.startsWith('app.theme.')).map((c) => c.id);
    expect(ids()).toEqual(['app.theme.sun', 'app.theme.light', 'app.theme.dark']);
    await runCommand('app.theme.light', ctx);
    expect(get(theme)).toBe('light');
    await runCommand('app.theme.dark', ctx);
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

describe('Stop the turn (review w13-4)', () => {
  beforeEach(() => {
    invoke.mockReset();
    toasts.set([]);
  });
  const ids = (sel: SessionRow) => paletteCommands({ selected: sel, sessionView: 'conversation' }).map((c) => c.id);

  it('is offered only while a turn runs and the session is not stuck', () => {
    expect(ids(session('mac', 'a', { claude_status: 'working' }))).toContain('session.stop');
    expect(ids(session('mac', 'a', { claude_status: 'idle' }))).not.toContain('session.stop');
    expect(ids(session('mac', 'a', { claude_status: 'working', stuck_kind: 'trust_prompt' }))).not.toContain('session.stop');
    expect(ids(session('mac', 'a', { claude_status: 'working', status: 'ghost' }))).not.toContain('session.stop');
  });

  it("presses Escape in the session's pane, as the composer's Stop does", async () => {
    const s = session('mac', 'fix-flake', { claude_status: 'working' });
    invoke.mockResolvedValue(null);
    await runCommand('session.stop', { selected: s, sessionView: 'conversation' });
    expect(invoke).toHaveBeenCalledWith('send_prompt', {
      args: { host_alias: 'mac', tmux_name: s.tmux_name, prompt: '', keys: 'Escape' },
    });
    expect(get(toasts).at(-1)?.message).toBe('Stopped the turn');
    // The turn ended before the pick: nothing is pressed.
    invoke.mockReset();
    await runCommand('session.stop', { selected: { ...s, claude_status: 'idle' }, sessionView: 'conversation' });
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe('every live chord has a palette row (review w18-4)', () => {
  afterEach(() => destination.set('session'));

  // These act on what is under the keyboard, not on the app: the switcher
  // itself, the list's ⌘1–9, the terminal walk and the Files tab's finder.
  const KEYS_ONLY = new Set(['switcher', 'jump-n', 'next-terminal', 'go-to-file']);

  it('each live global chord, but the keys-only ones, is a command carrying its id', async () => {
    const { SHORTCUTS } = await import('./shortcuts');
    const carried = new Set(
      paletteCommands({ selected: session('mac', 'a'), sessionView: 'conversation' }).map((c) => c.shortcut),
    );
    const missing = SHORTCUTS.filter(
      (s) => s.scope === 'global' && s.status === 'live' && !KEYS_ONLY.has(s.id) && !carried.has(s.id),
    ).map((s) => s.id);
    expect(missing).toEqual([]);
  });

  it('the rail items have rows too, and a new routine opens the editor', async () => {
    const ctx = { selected: null, sessionView: 'conversation' as const };
    const ids = paletteCommands(ctx).map((c) => c.id);
    expect(ids).toEqual(expect.arrayContaining(['app.control', 'app.inbox', 'app.toolkit', 'app.new-routine']));
    const { routinesRequest } = await import('./routines');
    await runCommand('app.new-routine', ctx);
    expect(get(destination)).toBe('automation');
    expect(get(routinesRequest)?.template).toBe('blank');
    routinesRequest.set(null);
    const { workspaceLanding } = await import('./workspace');
    await runCommand('app.inbox', ctx);
    expect(get(workspaceLanding)?.rail).toBe('inbox');
  });
});

// Review 113: ⌘K's New terminal from Settings (or a Work task's detail),
// which cover the pane while the destination stays 'session', brings the
// terminal into view instead of opening it unseen.
describe('session.new-terminal from a covered pane', () => {
  it('closes Settings and shows the terminal', async () => {
    const { settingsOpen } = await import('./app_views');
    const { sessionView } = await import('./prefs');
    destination.set('session');
    settingsOpen.set(true);
    await runCommand('session.new-terminal', { selected: session('mac', 'a'), sessionView: 'conversation' });
    expect(get(settingsOpen)).toBe(false);
    expect(get(sessionView)).toBe('terminal');
  });
});

// ⌘K over another fleet's session acts on that session, through its own hub,
// and never on this window's session hidden under it.
describe('another fleet’s session in the palette', () => {
  beforeEach(async () => {
    invoke.mockReset();
    toasts.set([]);
    const { _resetFleetsForTest, _setFleetsForTest, PRIMARY } = await import('./fleets');
    const { fleetInfo } = await import('./fleets_fixture');
    _resetFleetsForTest();
    _setFleetsForTest([fleetInfo(PRIMARY), fleetInfo('company')]);
  });
  afterEach(async () => {
    const { _resetFleetsForTest } = await import('./fleets');
    _resetFleetsForTest();
  });
  const away = (over: Partial<SessionRow> = {}) => ({ ...asking(), fleet: 'company', ...over }) as SessionRow;

  it('offers only Approve and Stop for it, both of which route by fleet', () => {
    const ids = paletteCommands({ selected: away({ claude_status: 'working', pending_input: null }), sessionView: 'conversation' })
      .map((c) => c.id)
      .filter((id) => id.startsWith('session.'));
    expect(ids).toEqual(['session.stop']);
    const asked = paletteCommands({ selected: away(), sessionView: 'conversation' })
      .map((c) => c.id)
      .filter((id) => id.startsWith('session.'));
    expect(asked).toEqual(['session.approve']);
    // The app's own rows are still there.
    expect(paletteCommands({ selected: away(), sessionView: 'conversation' }).map((c) => c.id)).toContain('app.settings');
  });

  it('Stop presses Escape on its own hub', async () => {
    invoke.mockResolvedValue(null);
    await runCommand('session.stop', { selected: away({ claude_status: 'working', pending_input: null }), sessionView: 'conversation' });
    expect(invoke).toHaveBeenCalledWith('send_prompt', expect.objectContaining({ fleet: 'company' }));
  });

  it('Approve and Stop ask that fleet’s link, not this window’s hub', async () => {
    const { _setFleetsForTest, PRIMARY } = await import('./fleets');
    const { fleetInfo } = await import('./fleets_fixture');
    _setFleetsForTest([fleetInfo(PRIMARY), fleetInfo('company', { connection: { state: 'offline' } } as never)]);
    await runCommand('session.approve', { selected: away(), sessionView: 'conversation' });
    await runCommand('session.stop', { selected: away({ claude_status: 'working', pending_input: null }), sessionView: 'conversation' });
    expect(invoke).not.toHaveBeenCalled();
    expect(get(toasts).at(-1)?.message).toMatch(/company is .*nothing can be sent there/);
  });

  it('a primary-only command picked for it does nothing', async () => {
    const { settingsOpen } = await import('./app_views');
    settingsOpen.set(true);
    invoke.mockResolvedValue(null);
    for (const id of ['session.push', 'session.open-in-editor', 'session.new-terminal', 'session.action.rename']) {
      await runCommand(id, { selected: away(), sessionView: 'conversation' });
    }
    expect(invoke).not.toHaveBeenCalled();
    // New terminal did not run: Settings stays open.
    expect(get(settingsOpen)).toBe(true);
    settingsOpen.set(false);
  });
});
