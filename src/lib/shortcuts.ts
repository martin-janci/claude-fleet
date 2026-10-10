/**
 * The shortcut registry (Orbit Fleet redesign step 0.1): every chord the app
 * answers, in one table, per platform.
 *
 * - `global` chords are matched here: `appChord`, `isSwitcherChord` and
 *   `isNewSessionChord` read this table, so a chord can only change by
 *   changing a row below, and `shortcuts.test.ts` freezes every 0.5.4 chord
 *   against a copy of the matchers as they shipped.
 * - The other scopes are the per-view tables (Hosts, Assets, the task list,
 *   the review sheets, the board, a session row) and the contextual chords of
 *   the terminal, the conversation, the switcher and the New session dialog.
 *   Every scope in `MATCHED_SCOPES` asks the table (`viewKey` /
 *   `matchShortcut`) which action a key is, so its rows ARE its handler's
 *   keys; `shortcuts.test.ts` replays every key and modifier combination
 *   through the table against a copy of each handler's 0.5.4 key logic. The
 *   three in `SCOPE_SOURCES` (the switcher's two and the New session dialog)
 *   still read `e.key`: their handlers take a key under any mix of ⌘ and Ctrl
 *   by design, so the rows there are the inventory the freeze test checks
 *   against their source. All of them are what the `?` sheet
 *   (`ShortcutSheet`, 3.8) lists and what ⌘K commands (3.9) read.
 * - `planned` rows are the design manual's new chords (`keyboard.md`): they
 *   match nothing yet, but they take part in the conflict check, so a chord
 *   is known to be free on both platforms before its step wires it.
 *
 * A binding names `KeyboardEvent.key` (compared case-insensitively) and the
 * exact modifiers held; `anyShift` ignores Shift, for symbols such as `?`
 * that need it on some layouts and for handlers that never looked at it.
 * `exactCase` compares the key as typed (a view's `j` is not Caps Lock's
 * `J`), and `anyMods` ignores every modifier (the board's Escape cancels a
 * drag whatever is held).
 */

export type Mod = 'meta' | 'ctrl' | 'alt' | 'shift';

export interface Binding {
  readonly key: string;
  readonly mods: readonly Mod[];
  readonly anyShift?: boolean;
  readonly exactCase?: boolean;
  readonly anyMods?: boolean;
}

export type Scope =
  | 'global'
  | 'terminal'
  | 'conversation'
  | 'switcher'
  | 'switcher-new'
  | 'new-session-dialog'
  | 'hosts'
  | 'assets'
  | 'task-list'
  | 'work-review'
  | 'link-review'
  | 'tidy-review'
  | 'work-board'
  | 'session-row'
  | 'session-list'
  | 'question-card'
  | 'form-card'
  | 'form';

/** Each scope's heading, in the order the lists show them: Settings →
 *  Shortcuts and the `?` sheet both read it, so they name a scope alike. */
export const SCOPE_TITLES: Record<Scope, string> = {
  global: 'Everywhere',
  'session-list': 'Session list',
  'session-row': 'Session row',
  'question-card': 'Question card',
  'form-card': 'Chat form',
  form: 'Dialogs and forms',
  conversation: 'Conversation',
  terminal: 'Terminal',
  switcher: 'Quick switcher',
  'switcher-new': 'Quick switcher, New session',
  'new-session-dialog': 'New session',
  hosts: 'Accounts & hosts',
  assets: 'Assets',
  'task-list': 'Task list',
  'work-review': 'Work review',
  'link-review': 'Link review',
  'tidy-review': 'Tidy up',
  'work-board': 'Work board',
};

export interface Shortcut {
  readonly id: string;
  readonly scope: Scope;
  readonly action: string;
  readonly mac: readonly Binding[];
  readonly other: readonly Binding[];
  /** `live` chords work today; `planned` ones are reserved for their step. */
  readonly status: 'live' | 'planned';
  /** The plan step that wires a planned chord. */
  readonly step?: string;
  /** Global chords this one deliberately takes over inside its scope. */
  readonly shadows?: readonly string[];
}

/** `'Meta+Shift+O'` → a binding. Trailing flags: `~` any Shift, `=` the
 *  key's exact case, `*` any modifiers (`'j~='`, `'Escape*'`). */
export function bind(spec: string): Binding {
  let body = spec;
  const flags = new Set<string>();
  while (body.length > 1 && '~=*'.includes(body[body.length - 1])) {
    flags.add(body[body.length - 1]);
    body = body.slice(0, -1);
  }
  const anyShift = flags.has('~');
  // The key may itself be `+` only as the whole spec; none of ours is.
  const parts = body.split('+');
  const key = parts.pop() ?? '';
  const mods = parts.map((p) => {
    const m = p.toLowerCase();
    if (m !== 'meta' && m !== 'ctrl' && m !== 'alt' && m !== 'shift') {
      throw new Error(`shortcuts: unknown modifier "${p}" in "${spec}"`);
    }
    return m as Mod;
  });
  return {
    key: key === 'Space' ? ' ' : key,
    mods,
    ...(anyShift ? { anyShift } : {}),
    ...(flags.has('=') ? { exactCase: true } : {}),
    ...(flags.has('*') ? { anyMods: true } : {}),
  };
}

/** A view's single keys as its handler has always read them: the key as
 *  typed (`=`), Shift ignored (`~`), and never with ⌘, Ctrl or Alt. */
const view = (...specs: string[]) => both(...specs.map((x) => (x.length === 1 && /[a-z]/i.test(x) ? `${x}~=` : `${x}~`)));

const both = (...specs: string[]) => ({ mac: specs.map(bind), other: specs.map(bind) });
const split = (mac: string[], other: string[]) => ({ mac: mac.map(bind), other: other.map(bind) });
const keys = (...specs: string[]) => both(...specs);

function row(
  scope: Scope,
  id: string,
  action: string,
  b: { mac: Binding[]; other: Binding[] },
  extra: Partial<Pick<Shortcut, 'status' | 'step' | 'shadows'>> = {},
): Shortcut {
  return { id, scope, action, ...b, status: extra.status ?? 'live', ...extra };
}

const digits = (prefix: string) => Array.from({ length: 9 }, (_, i) => `${prefix}${i + 1}`);

export const SHORTCUTS: readonly Shortcut[] = [
  // ── Global (app_views.appChord, quick_switcher) ──────────────────────
  // Plain Ctrl chords stay with the terminal on Linux/Windows (Ctrl+K is
  // kill-line, Ctrl+J line-feed), so the app takes Ctrl+Shift there. The
  // Super/Windows-key forms of ⌘I/⌘J/⌘E/⌘, are what appChord has always
  // accepted off the Mac; they are kept, not advertised.
  row('global', 'switcher', 'Quick switcher and commands',
    split(['Meta+K', 'Meta+P'], ['Ctrl+Shift+K', 'Ctrl+Shift+P'])),
  row('global', 'new-session', 'New session', split(['Meta+N'], ['Ctrl+Shift+N'])),
  row('global', 'hosts', 'Accounts and hosts', split(['Meta+I'], ['Ctrl+Shift+H', 'Meta+I'])),
  row('global', 'session-view', 'Flip the Session view (agent tab)',
    split(['Meta+J'], ['Ctrl+Shift+J', 'Meta+J'])),
  row('global', 'agent', 'The fleet agent (Control)', split(['Meta+E'], ['Ctrl+Shift+E', 'Meta+E'])),
  // Ctrl+, off the Mac is new in 1.9 (keyboard.md); the Super form stays.
  row('global', 'settings', 'Settings', split(['Meta+,'], ['Ctrl+,', 'Meta+,'])),
  row('global', 'work-view', 'Switch Sessions and Work', split(['Meta+Shift+W'], ['Ctrl+Shift+W'])),
  row('global', 'scope', 'Organisation scope', split(['Meta+Shift+O'], ['Ctrl+Shift+O'])),
  row('global', 'today', 'Today', split(['Meta+Shift+T'], ['Ctrl+Shift+T'])),
  // The design manual's new chords (keyboard.md); ⌥⌘ is Ctrl+Alt elsewhere.
  row('global', 'open-in-editor', 'Open in VS Code', split(['Meta+Shift+E'], ['Ctrl+Alt+E'])),
  row('global', 'inspector', 'Inspector', split(['Alt+Meta+B'], ['Ctrl+Alt+B'])),
  // Step 5.3: matched by TerminalView , where the strip is.
  row('global', 'new-terminal', 'New terminal', split(['Alt+Meta+T'], ['Ctrl+Alt+T']), { step: '5.3' }),
  row('global', 'next-terminal', 'Next terminal', split(['Meta+`'], ['Ctrl+`']), { step: '5.3' }),
  row('global', 'go-to-file', 'Go to file (Files tab only)',
    split(['Alt+Meta+P'], ['Ctrl+Alt+P'])),
  // Step 3.8: the list keys that work from anywhere outside a text field
  // (Sidebar, ShortcutSheet). ⌘1–9 is Mac-only: off the Mac Ctrl+digit and
  // Alt+digit belong to the terminal and the window manager.
  row('global', 'next-needs-you', 'Next session that needs you', split(['Alt+Meta+N'], ['Ctrl+Alt+N'])),
  row('global', 'jump-n', 'Open the 1st–9th session in the list', split(digits('Meta+'), [])),
  row('global', 'shortcut-sheet', 'Keyboard shortcuts', keys('?~')),

  // ── Terminal (TerminalView) ──────────────────────────────────────────
  // Cmd is never sent to the pty; Ctrl+Shift is the copy/paste chord off
  // the Mac so plain Ctrl+C / Ctrl+V still reach the program.
  row('terminal', 'terminal.paste', 'Paste', split(['Meta+V~'], ['Meta+V~', 'Ctrl+Shift+V'])),
  row('terminal', 'terminal.copy', 'Copy the selection', split(['Meta+C~'], ['Meta+C~', 'Ctrl+Shift+C'])),
  row('terminal', 'terminal.select-all', 'Select the viewport',
    split(['Meta+A~'], ['Meta+A~', 'Ctrl+Shift+A'])),

  // ── Conversation (ConversationPanel) ─────────────────────────────────
  row('conversation', 'conversation.find', 'Find in the conversation', split(['Meta+F'], ['Ctrl+F'])),
  row('conversation', 'conversation.prev-turn', 'Previous turn', keys('[~')),
  row('conversation', 'conversation.next-turn', 'Next turn', keys(']~')),

  // ── Quick switcher (QuickSwitcher, while open) ───────────────────────
  row('switcher', 'switcher.down', 'Next result', keys('ArrowDown', 'Ctrl+N')),
  row('switcher', 'switcher.up', 'Previous result', keys('ArrowUp')),
  row('switcher', 'switcher.open', 'Open the result', keys('Enter')),
  row('switcher', 'switcher.new-with-query', 'New session from the query (or start the ticket)',
    keys('Meta+Enter~', 'Ctrl+Enter~')),
  row('switcher-new', 'switcher-new.close-menu', 'Close the menu or clear the query', keys('Escape')),
  row('switcher-new', 'switcher-new.back', 'Back to the switcher (empty query)', keys('Backspace')),
  row('switcher-new', 'switcher-new.pin', 'Pin the project', keys('Meta+P~', 'Ctrl+P~'),
    { shadows: ['switcher'] }),
  row('switcher-new', 'switcher-new.hide', 'Hide the project', keys('Meta+Backspace~', 'Ctrl+Backspace~')),
  row('switcher-new', 'switcher-new.groups', 'Groups menu', keys('Meta+G~', 'Ctrl+G~')),
  row('switcher-new', 'switcher-new.undo', 'Undo the last pin or hide', keys('Meta+Z~', 'Ctrl+Z~')),
  row('switcher-new', 'switcher-new.menu', 'Project menu', keys('Shift+F10', 'ContextMenu~')),
  row('switcher-new', 'switcher-new.pick-n', 'Pick the numbered project',
    keys(...digits('Meta+').map((s) => `${s}~`), ...digits('Ctrl+').map((s) => `${s}~`)),
    { shadows: ['jump-n'] }),
  row('switcher-new', 'switcher-new.unfold', 'Unfold a group', keys('ArrowRight')),
  row('switcher-new', 'switcher-new.fold', 'Fold the group', keys('ArrowLeft')),

  // ── New session dialog ───────────────────────────────────────────────
  row('new-session-dialog', 'new-session.reroll', 'Re-roll the name', keys('Meta+R~', 'Ctrl+R~')),
  row('new-session-dialog', 'new-session.create', 'Create', keys('Enter')),

  // ── Per-view tables: single keys, never with Cmd/Ctrl/Alt ────────────
  // HostsView
  row('hosts', 'hosts.down', 'Next host', view('j', 'ArrowDown')),
  row('hosts', 'hosts.up', 'Previous host', view('k', 'ArrowUp')),
  row('hosts', 'hosts.first', 'First host', view('Home')),
  row('hosts', 'hosts.last', 'Last host', view('End')),
  row('hosts', 'hosts.detail', 'Into the detail', view('Enter', 'ArrowRight')),
  row('hosts', 'hosts.list', 'Back to the list', view('ArrowLeft')),
  row('hosts', 'hosts.close', 'Close the legend, the detail or Hosts', view('Escape')),
  row('hosts', 'hosts.reprobe', 'Re-probe the host', view('r')),
  row('hosts', 'hosts.usage', 'Refresh usage', view('u')),
  row('hosts', 'hosts.filter-sidebar', 'Filter the sidebar to the host', view('s')),
  row('hosts', 'hosts.new-session', 'New session on the host', view('n')),
  row('hosts', 'hosts.edit', 'Edit the account', view('e')),
  row('hosts', 'hosts.search', 'Search hosts', view('/')),
  row('hosts', 'hosts.legend', 'Legend', view('?~'), { shadows: ['shortcut-sheet'] }),
  // AssetsWorkspace
  row('assets', 'assets.primary', 'Run the primary (apply the card or Sync fleet)',
    keys('Meta+Enter', 'Ctrl+Enter', 'Meta+Ctrl+Enter')),
  row('assets', 'assets.down', 'Next row', view('j', 'ArrowDown')),
  row('assets', 'assets.up', 'Previous row', view('k', 'ArrowUp')),
  row('assets', 'assets.search', 'Search assets', view('/')),
  row('assets', 'assets.adopt', 'Adopt (import or apply the New card)', view('a')),
  row('assets', 'assets.sync', 'Sync the asset', view('s')),
  row('assets', 'assets.edit', 'Edit the asset', view('e')),
  row('assets', 'assets.ignore', 'Ignore the card', view('i')),
  // TaskList
  row('task-list', 'task-list.down', 'Next task', view('j')),
  row('task-list', 'task-list.up', 'Previous task', view('k')),
  row('task-list', 'task-list.work', 'Work on the task', keys('s')),
  row('task-list', 'task-list.work-ask', 'Start options for the task', keys('Shift+S')),
  // WorkReview
  row('work-review', 'work-review.down', 'Next item', view('j', 'ArrowDown')),
  row('work-review', 'work-review.up', 'Previous item', view('k', 'ArrowUp')),
  row('work-review', 'work-review.yes', 'Confirm or keep', view('y')),
  row('work-review', 'work-review.no', 'Reject the suggestion', view('n')),
  row('work-review', 'work-review.pick', 'Pick for a bulk action', view('x')),
  // LinkReview
  row('link-review', 'link-review.down', 'Next link', view('j', 'ArrowDown')),
  row('link-review', 'link-review.up', 'Previous link', view('k', 'ArrowUp')),
  row('link-review', 'link-review.yes', 'Confirm the link', view('y', 'Enter')),
  row('link-review', 'link-review.no', 'Not this', view('n', 'Backspace')),
  row('link-review', 'link-review.close', 'Close the sheet', view('Escape')),
  // TidyReview
  row('tidy-review', 'tidy-review.down', 'Next session', view('j', 'ArrowDown')),
  row('tidy-review', 'tidy-review.up', 'Previous session', view('k', 'ArrowUp')),
  row('tidy-review', 'tidy-review.toggle', 'Toggle the session', view('Space')),
  row('tidy-review', 'tidy-review.apply', 'Apply the tidy', view('Enter')),
  row('tidy-review', 'tidy-review.close', 'Close the sheet', view('Escape')),
  // WorkBoard
  row('work-board', 'work-board.edit', 'Edit the card', keys('e')),
  row('work-board', 'work-board.left', 'Move the card a column left', keys('ArrowLeft')),
  row('work-board', 'work-board.right', 'Move the card a column right', keys('ArrowRight')),
  row('work-board', 'work-board.cancel-drag', 'Cancel the drag', keys('Escape*')),
  // SessionRowItem
  row('session-row', 'session-row.yes', 'Confirm the suggested link', view('y')),
  row('session-row', 'session-row.no', 'Reject the suggested link', view('n')),
  row('session-row', 'session-row.link', 'Link or pick work', view('l')),

  // Sidebar session list (3.8): the row has focus.
  row('session-list', 'session-list.down', 'Next session', keys('j', 'ArrowDown')),
  row('session-list', 'session-list.up', 'Previous session', keys('k', 'ArrowUp')),
  row('session-list', 'session-list.open', 'Open the session', keys('Enter', 'Space')),
  row('session-list', 'session-list.pick', 'Select for a bulk action', keys('x')),

  // ── Question card (3.8): 1–9 answer when no text field has focus ─────
  row('question-card', 'question-card.answer', 'Answer with option 1–9', keys(...digits(''))),

  // ── Chat form (10.1): 1–9 pick the step's only numbered choice ───────
  row('form-card', 'form-card.option', 'Pick option 1–9', keys(...digits(''))),

  // ── Dialogs and forms (G1.2, FormsAnatomy): DialogSheet, WizardDialog
  // and FormWizard ask `submitKey` (forms/form_frame.ts). Esc cancels
  // through the native <dialog> (Modal), as it always has.
  row('form', 'form.submit', 'Submit the form', split(['Meta+Enter'], ['Ctrl+Enter'])),
  row('form', 'form.submit-one', 'Submit a one-field form', keys('Enter')),
];

/** The view handlers each per-view scope lives in, for the freeze test. */
/** The scopes whose handlers still read `e.key` themselves, for the freeze
 *  test: each takes some key under any mix of ⌘ and Ctrl (Enter, the
 *  arrows), which no row can say without taking chords from the terminal. */
export const SCOPE_SOURCES: Partial<Record<Scope, string>> = {
  switcher: 'src/lib/QuickSwitcher.svelte',
  'switcher-new': 'src/lib/QuickSwitcher.svelte',
  'new-session-dialog': 'src/lib/NewSessionDialog.svelte',
};

/** Scopes whose handler asks the registry (`viewKey` or `matchShortcut`),
 *  as the global chords do: the table is the handler, so there are no keys
 *  in the source to check — the freeze test replays every key instead. */
export const MATCHED_SCOPES: Partial<Record<Scope, string>> = {
  'session-list': 'src/lib/Sidebar.svelte',
  'question-card': 'src/lib/AnswerPrompt.svelte',
  'form-card': 'src/lib/forms/FormWizard.svelte',
  form: 'src/lib/forms/form_frame.ts',
  terminal: 'src/lib/TerminalView.svelte',
  conversation: 'src/lib/ConversationPanel.svelte',
  hosts: 'src/lib/HostsView.svelte',
  assets: 'src/lib/AssetsWorkspace.svelte',
  'task-list': 'src/lib/TaskList.svelte',
  'work-review': 'src/lib/WorkReview.svelte',
  'link-review': 'src/lib/LinkReview.svelte',
  'tidy-review': 'src/lib/TidyReview.svelte',
  'work-board': 'src/lib/WorkBoard.svelte',
  'session-row': 'src/lib/SessionRowItem.svelte',
};

export interface KeyEventLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  /** The physical key. On macOS Option rewrites `key` (⌥B types `∫`), so an
   *  Alt chord on a letter also matches by `code`. */
  code?: string;
  /** AltGr reports as Ctrl+Alt on Windows; the browser names it here. */
  getModifierState?: (key: string) => boolean;
}

/**
 * Whether `e` presses `b`. The by-`code` match is for macOS Option only:
 * on Windows AltGr arrives as Ctrl+Alt with `key` the typed character
 * (AltGr+B is `{` on a Slovak keyboard), and that text must never fire a
 * Ctrl+Alt chord (r18-W1).
 */
export function bindingMatches(b: Binding, e: KeyEventLike, isMac = true): boolean {
  if (e.getModifierState?.('AltGraph')) return false;
  if (b.exactCase) {
    if (b.key !== e.key) return false;
  } else {
    const letterByCode =
      isMac && e.altKey && /^[a-z]$/i.test(b.key) && e.code === `Key${b.key.toUpperCase()}`;
    if (b.key.toLowerCase() !== e.key.toLowerCase() && !letterByCode) return false;
  }
  if (b.anyMods) return true;
  const has = (m: Mod) => b.mods.includes(m);
  if (has('meta') !== e.metaKey || has('ctrl') !== e.ctrlKey || has('alt') !== e.altKey) return false;
  return b.anyShift === true || has('shift') === e.shiftKey;
}

export function bindingsFor(s: Shortcut, isMac: boolean): readonly Binding[] {
  return isMac ? s.mac : s.other;
}

/** The live shortcut in `scope` that `e` triggers, or null. */
export function matchShortcut(scope: Scope, e: KeyEventLike, isMac: boolean): string | null {
  for (const s of SHORTCUTS) {
    if (s.scope !== scope || s.status !== 'live') continue;
    if (bindingsFor(s, isMac).some((b) => bindingMatches(b, e, isMac))) return s.id;
  }
  return null;
}

const IS_MAC =
  typeof navigator !== 'undefined' &&
  (/Mac|iPhone|iPad|iPod/.test(navigator.platform ?? '') || /Macintosh/.test(navigator.userAgent ?? ''));

/**
 * The shared matcher every view's keydown uses (step 0.1): the action `e`
 * triggers in `scope` on this platform, or null. A view switches on the
 * returned id, never on `e.key`, so its keys are the rows above.
 */
export function viewKey(scope: Scope, e: KeyEventLike, isMac: boolean = IS_MAC): string | null {
  return matchShortcut(scope, e, isMac);
}

export function shortcutById(id: string): Shortcut | undefined {
  return SHORTCUTS.find((s) => s.id === id);
}

const MAC_GLYPH: Record<Mod, string> = { ctrl: '⌃', alt: '⌥', meta: '⌘', shift: '⇧' };
const MAC_ORDER: Mod[] = ['ctrl', 'alt', 'meta', 'shift'];
const OTHER_NAME: Record<Mod, string> = { ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', meta: 'Super' };
const OTHER_ORDER: Mod[] = ['ctrl', 'alt', 'shift', 'meta'];
const KEY_NAME: Record<string, string> = {
  ' ': 'Space', arrowdown: '↓', arrowup: '↑', arrowleft: '←', arrowright: '→', escape: 'Esc',
};

/** How a binding reads on its platform: `⌘⇧O`, `⌥⌘B`, `Ctrl+Shift+H`. */
export function formatBinding(b: Binding, isMac: boolean): string {
  const k = KEY_NAME[b.key.toLowerCase()] ?? (b.key.length === 1 ? b.key.toUpperCase() : b.key);
  if (isMac) return MAC_ORDER.filter((m) => b.mods.includes(m)).map((m) => MAC_GLYPH[m]).join('') + k;
  return [...OTHER_ORDER.filter((m) => b.mods.includes(m)).map((m) => OTHER_NAME[m]), k].join('+');
}

/** The advertised chord of a shortcut: its first binding on the platform. */
export function shortcutLabel(id: string, isMac: boolean): string {
  const s = shortcutById(id);
  const b = s ? bindingsFor(s, isMac)[0] : undefined;
  if (!b) throw new Error(`shortcuts: no ${isMac ? 'mac' : 'non-mac'} binding for "${id}"`);
  return formatBinding(b, isMac);
}

function overlaps(a: Binding, b: Binding): boolean {
  if (a.key.toLowerCase() !== b.key.toLowerCase()) return false;
  if (a.anyMods || b.anyMods) return true;
  const strip = (x: Binding) => x.mods.filter((m) => !(x.anyShift && m === 'shift'));
  const am = strip(a), bm = strip(b);
  for (const m of ['meta', 'ctrl', 'alt'] as const) if (am.includes(m) !== bm.includes(m)) return false;
  if (a.anyShift || b.anyShift) return true;
  return am.includes('shift') === bm.includes('shift');
}

export interface Conflict {
  a: string;
  b: string;
  platform: 'mac' | 'other';
  chord: string;
}

/**
 * Every pair of shortcuts that one key press would trigger together: two in
 * the same scope, or a global one and one in any scope (a global chord fires
 * everywhere) unless the scoped row declares that it `shadows` it.
 */
export function findConflicts(table: readonly Shortcut[] = SHORTCUTS): Conflict[] {
  const out: Conflict[] = [];
  for (let i = 0; i < table.length; i++) {
    for (let j = i + 1; j < table.length; j++) {
      const a = table[i], b = table[j];
      const sameScope = a.scope === b.scope;
      const crossGlobal = !sameScope && (a.scope === 'global' || b.scope === 'global');
      if (!sameScope && !crossGlobal) continue;
      if (crossGlobal) {
        const [g, s] = a.scope === 'global' ? [a, b] : [b, a];
        if (s.shadows?.includes(g.id)) continue;
      }
      for (const isMac of [true, false]) {
        for (const x of bindingsFor(a, isMac)) {
          const y = bindingsFor(b, isMac).find((yy) => overlaps(x, yy));
          if (y) {
            out.push({ a: a.id, b: b.id, platform: isMac ? 'mac' : 'other', chord: formatBinding(x, isMac) });
            break;
          }
        }
      }
    }
  }
  return out;
}
