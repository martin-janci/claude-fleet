import { describe, it, expect } from 'vitest';
import { appChord, hostsChordLabel, sessionViewChordLabel, agentChordLabel, todayChordLabel } from './app_views';

const ev = (key: string, mods: Partial<{ metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {}) => ({
  key,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe('appChord', () => {
  it('macOS: ⌘I is Hosts and ⌘, is Settings; Ctrl chords stay with the terminal', () => {
    expect(appChord(ev('i', { metaKey: true }), true)).toBe('hosts');
    expect(appChord(ev('I', { metaKey: true }), true)).toBe('hosts');
    expect(appChord(ev(',', { metaKey: true }), true)).toBe('settings');
    expect(appChord(ev('H', { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(appChord(ev('i', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('i'), true)).toBeNull();
  });

  it('modifier variants are not the chord (⌥⌘I is the web inspector, ⇧⌘I something else)', () => {
    expect(appChord(ev('i', { metaKey: true, altKey: true }), true)).toBeNull();
    expect(appChord(ev('i', { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(appChord(ev(',', { metaKey: true, ctrlKey: true }), true)).toBeNull();
  });

  it('Linux/Windows: Ctrl+Shift+H is Hosts; plain Ctrl+H (backspace) and Ctrl+Shift+I (devtools) are not', () => {
    expect(appChord(ev('H', { ctrlKey: true, shiftKey: true }), false)).toBe('hosts');
    expect(appChord(ev('h', { ctrlKey: true }), false)).toBeNull();
    expect(appChord(ev('I', { ctrlKey: true, shiftKey: true }), false)).toBeNull();
    expect(appChord(ev('i', { metaKey: true }), false)).toBe('hosts');
  });

  it('labels the Hosts chord per platform', () => {
    expect(hostsChordLabel(true)).toBe('⌘I');
    expect(hostsChordLabel(false)).toBe('Ctrl+Shift+H');
  });

  it('⌘J / Ctrl+Shift+J flips the Session view; plain Ctrl+J stays with the terminal', () => {
    expect(appChord(ev('j', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('J', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('J', { ctrlKey: true, shiftKey: true }), false)).toBe('session-view');
    // Plain Ctrl+J is line-feed in a terminal — it must reach the PTY.
    expect(appChord(ev('j', { ctrlKey: true }), false)).toBeNull();
    expect(appChord(ev('j', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('j'), true)).toBeNull();
    // Modifier variants are not the chord, same rule as ⌘I.
    expect(appChord(ev('j', { metaKey: true, altKey: true }), true)).toBeNull();
    expect(appChord(ev('j', { metaKey: true, shiftKey: true }), true)).toBeNull();
  });

  it('labels the Session-view chord per platform', () => {
    expect(sessionViewChordLabel(true)).toBe('⌘J');
    expect(sessionViewChordLabel(false)).toBe('Ctrl+Shift+J');
  });

  it('⌘E opens the agent, and does not collide with ⌘J (Session view)', () => {
    expect(appChord(ev('e', { metaKey: true }), true)).toBe('agent');
    expect(appChord(ev('E', { metaKey: true }), true)).toBe('agent');
    expect(appChord(ev('j', { metaKey: true }), true)).toBe('session-view');
    expect(appChord(ev('e', { ctrlKey: true }), true)).toBeNull();
    expect(appChord(ev('E', { ctrlKey: true, shiftKey: true }), false)).toBe('agent');
    expect(appChord(ev('e', { ctrlKey: true }), false)).toBeNull();
  });

  it('labels the agent chord per platform', () => {
    expect(agentChordLabel(true)).toBe('⌘E');
    expect(agentChordLabel(false)).toBe('Ctrl+Shift+E');
  });
});

describe('the Today chord (work graph M9.1)', () => {
  it('⌘⇧T on macOS, Ctrl+Shift+T elsewhere; never without Shift', () => {
    expect(appChord(ev('T', { metaKey: true, shiftKey: true }), true)).toBe('today');
    expect(appChord(ev('t', { ctrlKey: true, shiftKey: true }), false)).toBe('today');
    expect(appChord(ev('t', { metaKey: true }), true)).toBeNull();
    expect(appChord(ev('T', { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(todayChordLabel(true)).toBe('⌘⇧T');
    expect(todayChordLabel(false)).toBe('Ctrl+Shift+T');
  });
});
