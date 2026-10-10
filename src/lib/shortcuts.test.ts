import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import {
  MATCHED_SCOPES,
  SHORTCUTS,
  SCOPE_SOURCES,
  bind,
  findConflicts,
  formatBinding,
  matchShortcut,
  shortcutLabel,
  viewKey,
  type KeyEventLike,
  type Scope,
  type Shortcut,
} from './shortcuts';
import {
  appChord,
  agentChordLabel,
  hostsChordLabel,
  scopeChordLabel,
  sessionViewChordLabel,
  todayChordLabel,
  workViewChordLabel,
} from './app_views';
import { chordLabel, isNewSessionChord, isSwitcherChord } from './quick_switcher';

// The shortcut freeze (redesign step 0.1, ground rule 3). The functions below
// are the 0.5.4 matchers, copied verbatim as they shipped: the registry-backed
// ones must answer exactly as they did for every key and modifier
// combination, on the Mac and off it. Do not edit these copies to make a
// test pass; a chord that changes on purpose changes in the plan first.
type Ev = KeyEventLike;

function appChord054(e: Ev, isMac: boolean): string | null {
  if (e.altKey) return null;
  const k = e.key.toLowerCase();
  if (k === 'o' && e.shiftKey && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) return 'scope';
  if (k === 't' && e.shiftKey && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) return 'today';
  if (k === 'w' && e.shiftKey && (isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) return 'work-view';
  if (e.metaKey && !e.ctrlKey && !e.shiftKey) {
    if (k === 'i') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === 'e') return 'agent';
    if (k === ',') return 'settings';
    return null;
  }
  if (!isMac && e.ctrlKey && e.shiftKey && !e.metaKey) {
    if (k === 'h') return 'hosts';
    if (k === 'j') return 'session-view';
    if (k === 'e') return 'agent';
  }
  return null;
}

function isSwitcherChord054(e: Ev, isMac: boolean): boolean {
  const k = e.key.toLowerCase();
  if (k !== 'k' && k !== 'p') return false;
  if (e.altKey) return false;
  if (isMac) return e.metaKey && !e.ctrlKey && !e.shiftKey;
  return e.ctrlKey && e.shiftKey && !e.metaKey;
}

function isNewSessionChord054(e: Ev, isMac: boolean): boolean {
  if (e.key.toLowerCase() !== 'n' || e.altKey) return false;
  if (isMac) return e.metaKey && !e.ctrlKey && !e.shiftKey;
  return e.ctrlKey && e.shiftKey && !e.metaKey;
}

const LABELS_054 = {
  mac: { switcher: '⌘K', hosts: '⌘I', sessionView: '⌘J', scope: '⌘⇧O', today: '⌘⇧T', agent: '⌘E', workView: '⌘⇧W' },
  other: {
    switcher: 'Ctrl+Shift+K', hosts: 'Ctrl+Shift+H', sessionView: 'Ctrl+Shift+J', scope: 'Ctrl+Shift+O',
    today: 'Ctrl+Shift+T', agent: 'Ctrl+Shift+E', workView: 'Ctrl+Shift+W',
  },
};

// Every printable key and the named keys a chord could use, each in both
// cases (Shift turns `o` into `O`).
const KEYS = [
  ...'abcdefghijklmnopqrstuvwxyz0123456789'.split('').flatMap((c) => [c, c.toUpperCase()]),
  ',', '<', '.', '/', '?', '`', '~', '[', ']', '\\', ';', "'", '-', '=',
  'Enter', 'Escape', 'Backspace', 'Tab', ' ', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'F10',
];

function* allEvents(): Generator<Ev> {
  for (const key of KEYS) {
    for (let m = 0; m < 16; m++) {
      yield { key, metaKey: !!(m & 1), ctrlKey: !!(m & 2), altKey: !!(m & 4), shiftKey: !!(m & 8) };
    }
  }
}

// Chords added on purpose since 0.5.4, each by its plan step. The freeze
// allows exactly these, on exactly this platform, and nothing else; every
// chord 0.5.4 answered still answers the same.
const ADDED_SINCE_054: readonly { key: string; mods: Partial<Ev>; mac: boolean; action: string; step: string }[] = [
  { key: ',', mods: { ctrlKey: true }, mac: false, action: 'settings', step: '1.9' },
  ...(['b', 'B'] as const).flatMap((key) => [
    { key, mods: { metaKey: true, altKey: true }, mac: true, action: 'inspector', step: '3.5' },
    { key, mods: { ctrlKey: true, altKey: true }, mac: false, action: 'inspector', step: '3.5' },
  ]),
  ...(['e', 'E'] as const).flatMap((key) => [
    { key, mods: { metaKey: true, shiftKey: true }, mac: true, action: 'open-in-editor', step: '5.5' },
    { key, mods: { ctrlKey: true, altKey: true }, mac: false, action: 'open-in-editor', step: '5.5' },
  ]),
];

const isAddition = (e: Ev, isMac: boolean): string | null => {
  for (const a of ADDED_SINCE_054) {
    if (a.mac !== isMac || a.key !== e.key) continue;
    const want = { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...a.mods };
    if (e.metaKey === want.metaKey && e.ctrlKey === want.ctrlKey && e.altKey === want.altKey && e.shiftKey === want.shiftKey) {
      return a.action;
    }
  }
  return null;
};

const show = (e: Ev) =>
  `${e.metaKey ? 'Meta+' : ''}${e.ctrlKey ? 'Ctrl+' : ''}${e.altKey ? 'Alt+' : ''}${e.shiftKey ? 'Shift+' : ''}${e.key}`;

describe('shortcut freeze: every 0.5.4 global chord resolves to the same action', () => {
  for (const isMac of [true, false]) {
    const platform = isMac ? 'macOS' : 'Linux/Windows';

    it(`${platform}: app chords`, () => {
      const drift: string[] = [];
      for (const e of allEvents()) {
        const was = appChord054(e, isMac);
        const now = appChord(e, isMac);
        const added = isAddition(e, isMac);
        // An addition only ever fills a chord 0.5.4 left free.
        if (added !== null && was === null && now === added) continue;
        if (was !== now) drift.push(`${show(e)}: ${was} → ${now}`);
      }
      expect(drift).toEqual([]);
    });

    it(`${platform}: quick switcher and New session chords`, () => {
      const drift: string[] = [];
      for (const e of allEvents()) {
        if (isSwitcherChord054(e, isMac) !== isSwitcherChord(e, isMac)) drift.push(`switcher ${show(e)}`);
        if (isNewSessionChord054(e, isMac) !== isNewSessionChord(e, isMac)) drift.push(`new ${show(e)}`);
      }
      expect(drift).toEqual([]);
    });

    it(`${platform}: the advertised labels are unchanged`, () => {
      const want = isMac ? LABELS_054.mac : LABELS_054.other;
      expect({
        switcher: chordLabel(isMac),
        hosts: hostsChordLabel(isMac),
        sessionView: sessionViewChordLabel(isMac),
        scope: scopeChordLabel(isMac),
        today: todayChordLabel(isMac),
        agent: agentChordLabel(isMac),
        workView: workViewChordLabel(isMac),
      }).toEqual(want);
    });
  }

  it('the chords the plan names still answer', () => {
    const ev = (key: string, mods: Partial<Ev> = {}): Ev => ({
      key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods,
    });
    expect(appChord(ev(',', { metaKey: true }), true)).toBe('settings');
    // Step 1.9: Ctrl+, opens Settings on Linux and Windows; the Super form
    // still does, and on the Mac Ctrl+, stays free.
    expect(appChord(ev(',', { ctrlKey: true }), false)).toBe('settings');
    expect(appChord(ev(',', { metaKey: true }), false)).toBe('settings');
    expect(appChord(ev(',', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('W', { metaKey: true, shiftKey: true }), true)).toBe('work-view');
    expect(appChord(ev('E', { ctrlKey: true, shiftKey: true }), false)).toBe('agent');
    expect(isSwitcherChord(ev('P', { ctrlKey: true, shiftKey: true }), false)).toBe(true);
    // Plain Ctrl chords stay with the terminal off the Mac.
    expect(isSwitcherChord(ev('k', { ctrlKey: true }), false)).toBe(false);
    expect(isNewSessionChord(ev('n', { ctrlKey: true }), false)).toBe(false);
  });
});

describe('shortcut registry', () => {
  it('has unique ids', () => {
    const ids = SHORTCUTS.map((s) => s.id);
    expect(ids.filter((id, i) => ids.indexOf(id) !== i)).toEqual([]);
  });

  it('has no chord that triggers two actions, live or planned, on either platform', () => {
    expect(findConflicts()).toEqual([]);
  });

  it('⌘N is free for New task in Work: it shadows New session there and clashes with nothing (G2.1)', () => {
    const row = SHORTCUTS.find((s) => s.id === 'work.new-task');
    expect(row?.shadows).toEqual(['new-session']);
    const cmdN: KeyEventLike = { key: 'n', metaKey: true, ctrlKey: false, altKey: false, shiftKey: false };
    const ctrlShiftN: KeyEventLike = { key: 'N', metaKey: false, ctrlKey: true, altKey: false, shiftKey: true };
    expect(matchShortcut('work', cmdN, true)).toBe('work.new-task');
    expect(matchShortcut('work', ctrlShiftN, false)).toBe('work.new-task');
    expect(matchShortcut('global', cmdN, true)).toBe('new-session');
    // Without the shadow the two would clash; with it, nothing does.
    expect(findConflicts(SHORTCUTS.map((s) => (s.id === 'work.new-task' ? { ...s, shadows: [] } : s)))).toHaveLength(2);
    expect(findConflicts()).toEqual([]);
  });

  it('a duplicate chord fails the check', () => {
    const dup: Shortcut = {
      id: 'test.dup', scope: 'global', action: 'Clash with Today', status: 'planned',
      mac: [bind('Meta+Shift+T')], other: [bind('Ctrl+Shift+T')],
    };
    expect(findConflicts([...SHORTCUTS, dup])).toEqual([
      { a: 'today', b: 'test.dup', platform: 'mac', chord: '⌘⇧T' },
      { a: 'today', b: 'test.dup', platform: 'other', chord: 'Ctrl+Shift+T' },
    ]);
    // A view key that a global chord would also fire is a clash too.
    const viewDup: Shortcut = {
      id: 'test.view', scope: 'hosts', action: 'x', status: 'live',
      mac: [bind('Meta+K')], other: [bind('Ctrl+Shift+K')],
    };
    expect(findConflicts([...SHORTCUTS, viewDup]).map((c) => c.b)).toEqual(['test.view', 'test.view']);
  });

  it('every new chord from the design manual is registered and free', () => {
    const md = readFileSync('docs/ux/2026-10-08-orbit-fleet-redesign/design-system/keyboard.md', 'utf8');
    const mac = (id: string) => shortcutLabel(id, true);
    const other = (id: string) => shortcutLabel(id, false);
    expect(md).toContain(`| ${mac('open-in-editor')} | Open in VS Code (${other('open-in-editor')})`);
    expect(md).toContain(`| ${mac('inspector')} | Inspector (${other('inspector')})`);
    expect(md).toContain(`| ${mac('new-terminal')} | New terminal (${other('new-terminal')})`);
    expect(md).toContain(`| ${mac('next-terminal')} | Next terminal`);
    expect(md).toContain(`| ${mac('go-to-file')} | Go to file, Files tab only (${other('go-to-file')})`);
    // Wired by step 5.3 (TerminalView).
    for (const id of ['new-terminal', 'next-terminal']) {
      expect(SHORTCUTS.find((s) => s.id === id)?.status).toBe('live');
    }
    // Step 3.5 wired the inspector, 5.5 Open in VS Code.
    expect(SHORTCUTS.find((s) => s.id === 'inspector')?.status).toBe('live');
    expect(SHORTCUTS.find((s) => s.id === 'open-in-editor')?.status).toBe('live');
    // 5.6 Go to file, handled by FilesPanel.
    expect(SHORTCUTS.find((s) => s.id === 'go-to-file')?.status).toBe('live');
  });

  it('an Alt chord on a letter matches by physical key, as macOS Option rewrites the key', () => {
    const ev = { key: '∫', code: 'KeyB', metaKey: true, ctrlKey: false, altKey: true, shiftKey: false };
    expect(matchShortcut('global', ev, true)).toBe('inspector');
    expect(appChord(ev, true)).toBe('inspector');
    // Without Alt the code is not consulted: the key decides.
    expect(matchShortcut('global', { ...ev, key: 'x', altKey: false }, true)).not.toBe('inspector');
  });

  it('AltGr text on Windows never fires a Ctrl+Alt chord (r18-W1)', () => {
    // WebView2 reports AltGr as Ctrl+Alt; `key` is the character typed.
    const brace = { key: '{', code: 'KeyB', metaKey: false, ctrlKey: true, altKey: true, shiftKey: false };
    const euro = { ...brace, key: '€', code: 'KeyE' };
    for (const ev of [brace, euro]) {
      expect(matchShortcut('global', ev, false)).toBeNull();
      expect(appChord(ev, false)).toBeNull();
    }
    // Named AltGr is text on every platform.
    const named = { ...brace, key: 'b', getModifierState: (k: string) => k === 'AltGraph' };
    expect(matchShortcut('global', named, false)).toBeNull();
    // The real chord still works.
    expect(matchShortcut('global', { ...brace, key: 'b' }, false)).toBe('inspector');
  });

  it('planned chords match nothing until their step wires them', () => {
    const planned = SHORTCUTS.filter((s) => s.status === 'planned');
    // Step 5.3 wired the last planned rows; the guard stays for the next ones.
    for (const s of planned) {
      expect(s.step, s.id).toBeTruthy();
      for (const isMac of [true, false]) {
        for (const b of isMac ? s.mac : s.other) {
          const e: Ev = {
            key: b.key, metaKey: b.mods.includes('meta'), ctrlKey: b.mods.includes('ctrl'),
            altKey: b.mods.includes('alt'), shiftKey: b.mods.includes('shift'),
          };
          expect(matchShortcut(s.scope, e, isMac), `${s.id} ${formatBinding(b, isMac)}`).toBeNull();
        }
      }
    }
  });

  it('formats bindings the way the app labels them', () => {
    expect(formatBinding(bind('Alt+Meta+B'), true)).toBe('⌥⌘B');
    expect(formatBinding(bind('Ctrl+Alt+B'), false)).toBe('Ctrl+Alt+B');
    expect(formatBinding(bind('Meta+`'), true)).toBe('⌘`');
    expect(formatBinding(bind('Space'), false)).toBe('Space');
  });
});

// The scopes whose handlers still read `e.key` (SCOPE_SOURCES): each key the
// registry lists for one must still be handled in that view's source, so
// dropping one from a handler (or the registry) fails here.
describe('shortcut freeze: per-view tables match their handlers', () => {
  for (const [scope, file] of Object.entries(SCOPE_SOURCES)) {
    it(`${scope} (${file})`, () => {
      const src = readFileSync(file, 'utf8');
      const missing: string[] = [];
      for (const s of SHORTCUTS.filter((x) => x.scope === scope && x.status === 'live')) {
        for (const b of [...s.mac, ...s.other]) {
          const k = b.key;
          const forms = [`'${k}'`, `'${k.toLowerCase()}'`, `"${k}"`];
          // Digit rows are matched with a pattern in the handler.
          if (/^[1-9]$/.test(k)) forms.push('[1-9]');
          if (!forms.some((f) => src.includes(f))) missing.push(`${s.id}: ${k}`);
        }
      }
      expect(missing).toEqual([]);
    });
  }
});

// Step 0.1's freeze for the views moved onto the registry: each function
// below is that view's 0.5.4 key logic, copied as it shipped and reduced to
// the action it took. Every key and modifier combination is replayed as a
// real KeyboardEvent through the registry (`viewKey`, what the handler now
// asks) and must name the same action, on the Mac and off it. Do not edit
// these copies to make a test pass.
type Frozen = (e: Ev, isMac: boolean) => string | null;
const noMod = (e: Ev) => !e.metaKey && !e.ctrlKey && !e.altKey;
const FROZEN_054: Record<string, Frozen> = {
  terminal: (e, isMac) => {
    const k = e.key.toLowerCase();
    const cmdChord = e.metaKey && !e.altKey && !e.ctrlKey;
    const ctrlShiftChord = !isMac && e.ctrlKey && e.shiftKey && !e.altKey && !e.metaKey;
    if (!(cmdChord || ctrlShiftChord)) return null;
    return k === 'v' ? 'terminal.paste' : k === 'c' ? 'terminal.copy' : k === 'a' ? 'terminal.select-all' : null;
  },
  conversation: (e, isMac) => {
    if ((e.key === '[' || e.key === ']') && noMod(e)) return e.key === '[' ? 'conversation.prev-turn' : 'conversation.next-turn';
    const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
    if (!mod || e.altKey || e.shiftKey || e.key.toLowerCase() !== 'f') return null;
    return 'conversation.find';
  },
  hosts: (e) => {
    if (!noMod(e)) return null;
    const m: Record<string, string> = {
      ArrowDown: 'hosts.down', j: 'hosts.down', ArrowUp: 'hosts.up', k: 'hosts.up', Home: 'hosts.first',
      End: 'hosts.last', Enter: 'hosts.detail', ArrowRight: 'hosts.detail', ArrowLeft: 'hosts.list',
      Escape: 'hosts.close', r: 'hosts.reprobe', u: 'hosts.usage', s: 'hosts.filter-sidebar',
      n: 'hosts.new-session', e: 'hosts.edit', '/': 'hosts.search', '?': 'hosts.legend',
    };
    return m[e.key] ?? null;
  },
  assets: (e) => {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key === 'Enter') return 'assets.primary';
    if (!noMod(e)) return null;
    const m: Record<string, string> = {
      j: 'assets.down', ArrowDown: 'assets.down', k: 'assets.up', ArrowUp: 'assets.up', '/': 'assets.search',
      a: 'assets.adopt', s: 'assets.sync', e: 'assets.edit', i: 'assets.ignore',
    };
    return m[e.key] ?? null;
  },
  'task-list': (e) => {
    if (!noMod(e)) return null;
    if (e.key === 'j' || e.key === 'k') return e.key === 'j' ? 'task-list.down' : 'task-list.up';
    if (e.key === 's' || e.key === 'S') return e.shiftKey ? 'task-list.work-ask' : 'task-list.work';
    return null;
  },
  'work-review': (e) => {
    if (!noMod(e)) return null;
    const m: Record<string, string> = {
      j: 'work-review.down', ArrowDown: 'work-review.down', k: 'work-review.up', ArrowUp: 'work-review.up',
      y: 'work-review.yes', n: 'work-review.no', x: 'work-review.pick',
    };
    return m[e.key] ?? null;
  },
  'link-review': (e) => {
    if (!noMod(e)) return null;
    const m: Record<string, string> = {
      Escape: 'link-review.close', j: 'link-review.down', ArrowDown: 'link-review.down', k: 'link-review.up',
      ArrowUp: 'link-review.up', y: 'link-review.yes', Enter: 'link-review.yes', n: 'link-review.no',
      Backspace: 'link-review.no',
    };
    return m[e.key] ?? null;
  },
  'tidy-review': (e) => {
    if (!noMod(e)) return null;
    const m: Record<string, string> = {
      Escape: 'tidy-review.close', j: 'tidy-review.down', ArrowDown: 'tidy-review.down', k: 'tidy-review.up',
      ArrowUp: 'tidy-review.up', ' ': 'tidy-review.toggle', Enter: 'tidy-review.apply',
    };
    return m[e.key] ?? null;
  },
  // Two handlers: the window's Escape (while a drag runs, whatever is held)
  // and the card's keys.
  'work-board': (e) => {
    if (e.key === 'Escape') return 'work-board.cancel-drag';
    if (e.metaKey || e.ctrlKey || e.altKey || e.shiftKey) return null;
    if (e.key === 'e' || e.key === 'E') return 'work-board.edit';
    if (e.key === 'ArrowLeft') return 'work-board.left';
    if (e.key === 'ArrowRight') return 'work-board.right';
    return null;
  },
  'session-row': (e) => {
    if (!noMod(e)) return null;
    return e.key === 'y' ? 'session-row.yes' : e.key === 'n' ? 'session-row.no' : e.key === 'l' ? 'session-row.link' : null;
  },
};

// The named keys the views use beyond the freeze's KEYS.
const VIEW_KEYS = [...KEYS, 'Home', 'End', 'Space'];

function* realEvents(): Generator<KeyboardEvent> {
  for (const key of VIEW_KEYS) {
    for (let m = 0; m < 16; m++) {
      yield new KeyboardEvent('keydown', {
        key, metaKey: !!(m & 1), ctrlKey: !!(m & 2), altKey: !!(m & 4), shiftKey: !!(m & 8), bubbles: true,
      });
    }
  }
}

describe('shortcut freeze: views matched through the registry answer every key as 0.5.4 did', () => {
  it('covers every matched scope that had a 0.5.4 handler', () => {
    // 'form' (G1.2) is new since 0.5.4 too: DialogSheet/WizardDialog/FormWizard.
    // 'work' (G2.1) too: ⌘N makes a task in the Work view.
    const since38 = ['session-list', 'question-card', 'form-card', 'form', 'work'];
    expect(Object.keys(FROZEN_054).sort()).toEqual(Object.keys(MATCHED_SCOPES).filter((k) => !since38.includes(k)).sort());
    // 15 of the 18 scopes ask the registry; the other 3 are SCOPE_SOURCES.
    expect(Object.keys(MATCHED_SCOPES)).toHaveLength(15);
    expect(new Set([...Object.keys(MATCHED_SCOPES), ...Object.keys(SCOPE_SOURCES), 'global']).size).toBe(19);
  });

  for (const [scope, frozen] of Object.entries(FROZEN_054)) {
    for (const isMac of [true, false]) {
      it(`${scope} on ${isMac ? 'macOS' : 'Linux/Windows'}`, () => {
        const drift: string[] = [];
        for (const e of realEvents()) {
          const was = frozen(e, isMac);
          const now = viewKey(scope as Scope, e, isMac);
          if (was !== now) drift.push(`${show(e)}: ${was} → ${now}`);
        }
        expect(drift).toEqual([]);
      });
    }
  }
});

// Step 3.8's scopes are matched through the registry, so their handlers must
// ask it for their own scope.
describe('shortcut registry: matched scopes ask the registry', () => {
  for (const [scope, file] of Object.entries(MATCHED_SCOPES)) {
    it(`${scope} (${file})`, () => {
      const src = readFileSync(file, 'utf8');
      expect(src.includes(`matchShortcut('${scope}'`) || src.includes(`viewKey('${scope}'`)).toBe(true);
    });
  }

  it("3.8's global chords: ⌥⌘N / Ctrl+Alt+N, ⌘1–9 on the Mac only, ? everywhere", () => {
    expect(shortcutLabel('next-needs-you', true)).toBe('⌥⌘N');
    expect(shortcutLabel('next-needs-you', false)).toBe('Ctrl+Alt+N');
    expect(shortcutLabel('jump-n', true)).toBe('⌘1');
    expect(SHORTCUTS.find((s) => s.id === 'jump-n')?.other).toEqual([]);
    const q: KeyEventLike = { key: '?', metaKey: false, ctrlKey: false, altKey: false, shiftKey: true };
    expect(matchShortcut('global', q, true)).toBe('shortcut-sheet');
    expect(matchShortcut('global', q, false)).toBe('shortcut-sheet');
    const three: KeyEventLike = { key: '3', metaKey: false, ctrlKey: false, altKey: false, shiftKey: false };
    expect(matchShortcut('question-card', three, true)).toBe('question-card.answer');
  });
});
