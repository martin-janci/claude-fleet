// The ⌘K command registry (Orbit Fleet redesign step 3.9): what the quick
// switcher can DO besides open a row. Commands on the open session, app
// commands (each showing its chord from the shortcut registry), Pause all,
// and settings changed in plain words (`pages/settings_nl.ts`).
//
// Prefixes narrow the switcher to one kind, as on the Palette board:
// `>` commands, `#` tasks and tickets, `@` hosts.
//
// Every command is something the person picks; none runs on its own. A
// settings change applies on Enter exactly as Settings' own plain-words row
// does, and one whose setting asks for confirmation opens Settings instead.
import { get } from 'svelte/store';
import { settingsOpen, requestHostsView, shortcutSheetOpen } from './app_views';
import { goTo } from './destination';
import { pauseAllMissions } from './missions';
import { sessionView } from './prefs';
import type { SessionView } from './session_view';
import { theme, cycleTheme, type Theme } from './theme';
import { openToday } from './control';
import { toggleSidebarView } from './work_view';
import { pendingInputFor, type AnswerView } from './pending_input';
import { sendAnswer } from './answer_send';
import { hubActionBlocked, hubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { sessionActionBlocked } from './share';
import { push, pushError } from './toasts';
import { fleetSettings, setFleetSetting, SETTING_KEYS, settingBool, type SettingKey } from './fleet_settings';
import { setAutomationPaused } from './automation';
import { interpret } from './pages/settings_nl';
import { valueInWords } from './pages/review';
import type { Descriptor } from './pages/pages';
import type { SessionRow } from './sessions';
import { shortcutLabel } from './shortcuts';
import { requestSessionAction, sessionMenuItems, type SessionActionId } from './session_actions';
import type { SwitcherEntry } from './quick_switcher';

export type PrefixMode = 'all' | 'commands' | 'work' | 'hosts';

const PREFIXES: Record<string, PrefixMode> = { '>': 'commands', '#': 'work', '@': 'hosts' };

/** `> pause` → commands, `pause`. A prefix is the query's first character. */
export function splitPrefix(query: string): { mode: PrefixMode; rest: string } {
  const q = query.trimStart();
  const mode = PREFIXES[q.charAt(0)];
  return mode ? { mode, rest: q.slice(1).trim() } : { mode: 'all', rest: query };
}

/** The kinds of row each prefix keeps. */
export function keepsKind(mode: PrefixMode, kind: SwitcherEntry['kind']): boolean {
  switch (mode) {
    case 'all':
      return true;
    case 'commands':
      return kind === 'command' || kind === 'setting';
    case 'work':
      return kind === 'ticket' || kind === 'lookup';
    case 'hosts':
      return kind === 'host';
  }
}

export interface CommandContext {
  selected: SessionRow | null;
  sessionView: SessionView;
}

export interface PaletteCommand {
  id: string;
  label: string;
  /** The group heading: `This session` or `Commands`. */
  section: string;
  description: string;
  synonyms: string[];
  /** A shortcut-registry id whose chord the row shows. */
  shortcut?: string;
}

/** The question the open session is asking, when it is a permission
 *  dialog whose first option is one keystroke: what Approve presses. */
export function approvable(s: SessionRow | null): AnswerView | null {
  if (!s || s.status === 'ghost') return null;
  const v = pendingInputFor({
    rowStatus: s.claude_status,
    rowStuck: s.stuck_kind,
    rowPending: s.pending_input,
    probe: null,
  });
  if (!v || v.kind !== 'permission' || v.multi) return null;
  return v.options[0]?.key ? v : null;
}

/** The commands available now, session commands first. */
export function paletteCommands(ctx: CommandContext): PaletteCommand[] {
  const out: PaletteCommand[] = [];
  const s = ctx.selected;
  if (s && s.status !== 'ghost') {
    const name = s.friendly_name || s.tmux_name;
    const ask = approvable(s);
    if (ask) {
      out.push({
        id: 'session.approve',
        label: `Approve: ${ask.options[0].label}`,
        section: 'This session',
        description: `${name} · presses ${ask.options[0].key} if the dialog is still on screen`,
        synonyms: ['approve', 'allow', 'yes', 'permission'],
      });
    }
    const toTerminal = ctx.sessionView !== 'terminal';
    out.push({
      id: 'session.flip-view',
      label: toTerminal ? 'Show the terminal' : 'Show the conversation',
      section: 'This session',
      description: name,
      synonyms: ['terminal', 'conversation', 'view', 'flip'],
      shortcut: 'session-view',
    });
    out.push({
      id: 'session.files',
      label: 'Open Files',
      section: 'This session',
      description: name,
      synonyms: ['files', 'browse', 'tree'],
    });
    // The session's own actions, from the one registry the row menu and
    // Details read (`session_actions.ts`). Only the ones it may run now: a
    // palette row cannot be disabled, and the menu and Details say why.
    for (const a of get(sessionMenuItems)(s)) {
      if (a.blocked !== null) continue;
      out.push({
        id: `${SESSION_ACTION}${a.id}`,
        label: a.label,
        section: 'This session',
        description: name,
        synonyms: [...(a.synonyms ?? [])],
      });
    }
  }
  out.push(
    { id: 'app.settings', label: 'Open Settings', section: 'Commands', description: 'Settings',
      synonyms: ['settings', 'preferences', 'options'], shortcut: 'settings' },
    { id: 'app.hosts', label: 'Accounts and hosts', section: 'Commands', description: 'Hosts',
      synonyms: ['hosts', 'accounts', 'machines'], shortcut: 'hosts' },
    { id: 'app.today', label: 'Today', section: 'Commands', description: 'What happened today',
      synonyms: ['today', 'digest'], shortcut: 'today' },
    { id: 'app.work-view', label: 'Switch Sessions and Work', section: 'Commands', description: 'Left list',
      synonyms: ['work', 'sessions', 'switch'], shortcut: 'work-view' },
    { id: 'app.theme', label: `Theme: ${themeAfter(get(theme))}`, section: 'Commands',
      description: `Now ${get(theme)}`, synonyms: ['theme', 'dark', 'light', 'auto', 'appearance'] },
    { id: 'app.shortcuts', label: 'Keyboard shortcuts', section: 'Commands', description: 'Every chord',
      synonyms: ['keys', 'keyboard', 'shortcuts', 'help'], shortcut: 'shortcut-sheet' },
    // Pause all missions pauses each active mission, as before; Pause all
    // automation (8.4, the header's Pause all) is `automation.paused`, every
    // loop that acts on its own.
    { id: 'app.pause-all', label: 'Pause all missions', section: 'Commands', description: 'Automation',
      synonyms: ['pause', 'pause all', 'automation', 'stop', 'missions'] },
    settingBool(get(fleetSettings), SETTING_KEYS.automationPaused)
      ? { id: 'app.automation-pause', label: 'Resume automation', section: 'Commands', description: 'Automation',
          synonyms: ['resume', 'automation', 'unpause', 'loops'] }
      : { id: 'app.automation-pause', label: 'Pause all automation', section: 'Commands', description: 'Automation',
          synonyms: ['pause', 'pause all', 'automation', 'stop', 'loops'] },
    { id: 'app.automation', label: 'Open Automation', section: 'Commands', description: 'Routines, runs, agents',
      synonyms: ['automation', 'routines', 'runs', 'agents', 'loops'] },
  );
  return out;
}

const THEME_NEXT: Record<Theme, Theme> = { auto: 'light', light: 'dark', dark: 'auto' };
/** The theme the command switches to: the sidebar toggle's order. */
function themeAfter(t: Theme): Theme {
  return THEME_NEXT[t];
}

/** Switcher rows for `cmds`, each carrying the chord the platform shows. */
export function commandRows(cmds: readonly PaletteCommand[], isMac: boolean): SwitcherEntry[] {
  return cmds.map((c) => ({
    kind: 'command',
    key: `cmd:${c.id}`,
    label: c.label,
    description: c.description,
    meta: c.shortcut ? shortcutLabel(c.shortcut, isMac) : c.section,
    fields: [c.label, ...c.synonyms],
    action: c.id,
    section: c.section,
  }));
}

/** A plain-words settings change as a row, or null. Only a change this
 *  client may write becomes a row; anything else stays a search. */
export function settingRow(rest: string, descs: Iterable<Descriptor>, canWrite: boolean): SwitcherEntry | null {
  if (!canWrite) return null;
  const nl = interpret(rest, descs);
  if (nl?.kind !== 'change') return null;
  const { d, value } = nl;
  const confirm = d.danger.level === 'confirm';
  return {
    kind: 'setting',
    key: `setting:${d.key}=${value}`,
    label: `Set ${d.label} to ${valueInWords(d, value)}`,
    description: confirm ? 'Opens Settings to confirm' : `Settings · ${d.key}`,
    meta: 'Settings',
    // Always matched: the words were already matched against the registry.
    fields: [rest],
    setting: { key: d.key, value, label: d.label, words: valueInWords(d, value), confirm },
    section: 'Settings',
  };
}

/** A palette id that runs a session action: `session.action.<id>`. */
const SESSION_ACTION = 'session.action.';

/** Run a palette command. Returns once the command has been handed off. */
export async function runCommand(id: string, ctx: CommandContext): Promise<void> {
  const s = ctx.selected;
  if (id.startsWith(SESSION_ACTION)) {
    // Details runs it, with its own gate, confirm and dialog, as it does for
    // the row menu; the request opens the inspector or the Details tab.
    if (s && s.status !== 'ghost') requestSessionAction(s, id.slice(SESSION_ACTION.length) as SessionActionId);
    return;
  }
  switch (id) {
    case 'session.approve': {
      const v = approvable(s);
      if (!s || !v) return;
      // Asked here too (the sweep wants the gate where the write is), so a
      // refusal is a toast before the pane is read; sendAnswer asks again.
      const why = hubActionBlocked('send_prompt', get(hubStatus), get(hubConnection)) ?? sessionActionBlocked(s, 'answer_dialog');
      if (why !== null) {
        push({ kind: 'info', message: why });
        return;
      }
      const out = await sendAnswer(s, v, v.options[0].key!);
      if (out.ok) push({ kind: 'success', message: `Sent: ${v.options[0].label}` });
      else push({ kind: 'info', message: 'stale' in out ? out.stale : 'blocked' in out ? out.blocked : out.error });
      return;
    }
    case 'session.flip-view':
      sessionView.update((v) => (v === 'terminal' ? 'conversation' : 'terminal'));
      goTo('session');
      return;
    case 'session.files':
      goTo('files');
      return;
    case 'app.settings':
      settingsOpen.set(true);
      return;
    case 'app.hosts':
      requestHostsView();
      return;
    case 'app.today':
      openToday();
      return;
    case 'app.work-view':
      toggleSidebarView();
      return;
    case 'app.theme':
      cycleTheme();
      return;
    case 'app.shortcuts':
      shortcutSheetOpen.set(true);
      return;
    case 'app.automation':
      goTo('automation');
      return;
    case 'app.automation-pause': {
      const on = !settingBool(get(fleetSettings), SETTING_KEYS.automationPaused);
      const r = await setAutomationPaused(on);
      if (!r.ok) pushError(r.error, on ? 'Pause all failed' : 'Resume failed');
      else push({ kind: 'success', message: on ? 'Automation paused' : 'Automation resumed' });
      return;
    }
    case 'app.pause-all': {
      const r = await pauseAllMissions();
      if (!r.ok) pushError(r.error, 'Pause all failed');
      else push({ kind: 'success', message: `Paused ${r.value.length} mission${r.value.length === 1 ? '' : 's'}` });
      return;
    }
  }
}

/** Apply a settings row: written as the person, as Settings' own row does;
 *  a setting that asks for confirmation opens Settings instead. */
export async function runSetting(setting: NonNullable<SwitcherEntry['setting']>): Promise<void> {
  if (setting.confirm) {
    settingsOpen.set(true);
    return;
  }
  const r = await setFleetSetting(setting.key as SettingKey, setting.value);
  if (!r.ok) pushError(r.error, `${setting.label} not changed`);
  else push({ kind: 'success', message: `${setting.label}: ${setting.words}` });
}
