import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import {
  SHORTCUTS,
  SCOPE_SOURCES,
  bind,
  findConflicts,
  formatBinding,
  matchShortcut,
  shortcutLabel,
  type KeyEventLike,
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
    for (const id of ['open-in-editor', 'inspector', 'new-terminal', 'next-terminal', 'go-to-file']) {
      expect(SHORTCUTS.find((s) => s.id === id)?.status).toBe('planned');
    }
  });

  it('planned chords match nothing until their step wires them', () => {
    const planned = SHORTCUTS.filter((s) => s.status === 'planned');
    expect(planned.length).toBeGreaterThan(0);
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

// The per-view tables still read `e.key` in their own handlers. Each key the
// registry lists for a view must still be handled in that view's source, so
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
