import { describe, it, expect } from 'vitest';
import { appChord, hostsChordLabel } from './app_views';

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
});
