import { describe, it, expect } from 'vitest';
import { keyToBytes, detectMac, type KeyLike, type KeyOpts } from './terminal_keys';

const base: KeyLike = { key: '', ctrlKey: false, altKey: false, shiftKey: false, metaKey: false };
const k = (key: string, mods: Partial<KeyLike> = {}): KeyLike => ({ ...base, key, ...mods });
const linux: KeyOpts = { appCursor: false, isMac: false };
const mac: KeyOpts = { appCursor: false, isMac: true };
const app: KeyOpts = { appCursor: true, isMac: false };

const ESC = '\x1b';

describe('terminal_keys.keyToBytes — xterm key table', () => {
  const cases: Array<[string, KeyLike, KeyOpts, string | null]> = [
    // Printable
    ['plain letter', k('a'), linux, 'a'],
    ['shifted letter', k('A', { shiftKey: true }), linux, 'A'],
    ['space', k(' '), linux, ' '],
    // Basic editing keys
    ['Enter', k('Enter'), linux, '\r'],
    ['Backspace', k('Backspace'), linux, '\x7f'],
    ['Ctrl+Backspace', k('Backspace', { ctrlKey: true }), linux, '\x08'],
    ['Tab', k('Tab'), linux, '\t'],
    ['Shift+Tab → CBT', k('Tab', { shiftKey: true }), linux, `${ESC}[Z`],
    ['Escape', k('Escape'), linux, ESC],
    // Arrows / Home / End (normal and application cursor mode)
    ['Up', k('ArrowUp'), linux, `${ESC}[A`],
    ['Down', k('ArrowDown'), linux, `${ESC}[B`],
    ['Right', k('ArrowRight'), linux, `${ESC}[C`],
    ['Left', k('ArrowLeft'), linux, `${ESC}[D`],
    ['Home', k('Home'), linux, `${ESC}[H`],
    ['End', k('End'), linux, `${ESC}[F`],
    ['Up (app cursor)', k('ArrowUp'), app, `${ESC}OA`],
    ['Home (app cursor)', k('Home'), app, `${ESC}OH`],
    ['End (app cursor)', k('End'), app, `${ESC}OF`],
    // Modifier-encoded cursor keys: 1;m where m = 1 + shift|alt<<1|ctrl<<2
    ['Shift+Up', k('ArrowUp', { shiftKey: true }), linux, `${ESC}[1;2A`],
    ['Alt+Left', k('ArrowLeft', { altKey: true }), linux, `${ESC}[1;3D`],
    ['Ctrl+Right', k('ArrowRight', { ctrlKey: true }), linux, `${ESC}[1;5C`],
    ['Ctrl+Shift+Left', k('ArrowLeft', { ctrlKey: true, shiftKey: true }), linux, `${ESC}[1;6D`],
    ['Ctrl+Home', k('Home', { ctrlKey: true }), linux, `${ESC}[1;5H`],
    ['Shift+End', k('End', { shiftKey: true }), linux, `${ESC}[1;2F`],
    ['Ctrl+Up (app cursor still CSI with modifier)', k('ArrowUp', { ctrlKey: true }), app, `${ESC}[1;5A`],
    // Tilde family
    ['Insert', k('Insert'), linux, `${ESC}[2~`],
    ['Delete', k('Delete'), linux, `${ESC}[3~`],
    ['PageUp', k('PageUp'), linux, `${ESC}[5~`],
    ['PageDown', k('PageDown'), linux, `${ESC}[6~`],
    ['Shift+Delete', k('Delete', { shiftKey: true }), linux, `${ESC}[3;2~`],
    ['Ctrl+PageUp', k('PageUp', { ctrlKey: true }), linux, `${ESC}[5;5~`],
    ['Alt+PageDown', k('PageDown', { altKey: true }), linux, `${ESC}[6;3~`],
    // Function keys
    ['F1', k('F1'), linux, `${ESC}OP`],
    ['F2', k('F2'), linux, `${ESC}OQ`],
    ['F3', k('F3'), linux, `${ESC}OR`],
    ['F4', k('F4'), linux, `${ESC}OS`],
    ['F5', k('F5'), linux, `${ESC}[15~`],
    ['F6', k('F6'), linux, `${ESC}[17~`],
    ['F7', k('F7'), linux, `${ESC}[18~`],
    ['F8', k('F8'), linux, `${ESC}[19~`],
    ['F9', k('F9'), linux, `${ESC}[20~`],
    ['F10', k('F10'), linux, `${ESC}[21~`],
    ['F11', k('F11'), linux, `${ESC}[23~`],
    ['F12', k('F12'), linux, `${ESC}[24~`],
    ['Shift+F1', k('F1', { shiftKey: true }), linux, `${ESC}[1;2P`],
    ['Ctrl+F4', k('F4', { ctrlKey: true }), linux, `${ESC}[1;5S`],
    ['Shift+F5', k('F5', { shiftKey: true }), linux, `${ESC}[15;2~`],
    ['Ctrl+Alt+F12', k('F12', { ctrlKey: true, altKey: true }), linux, `${ESC}[24;7~`],
    // Ctrl chords → C0 bytes
    ['Ctrl+A', k('a', { ctrlKey: true }), linux, '\x01'],
    ['Ctrl+C', k('c', { ctrlKey: true }), linux, '\x03'],
    ['Ctrl+Z', k('z', { ctrlKey: true }), linux, '\x1a'],
    ['Ctrl+Shift+A (shift ignored)', k('A', { ctrlKey: true, shiftKey: true }), linux, '\x01'],
    ['Ctrl+Space → NUL', k(' ', { ctrlKey: true }), linux, '\x00'],
    ['Ctrl+[ → ESC', k('[', { ctrlKey: true }), linux, ESC],
    ['Ctrl+\\ → FS', k('\\', { ctrlKey: true }), linux, '\x1c'],
    ['Ctrl+] → GS', k(']', { ctrlKey: true }), linux, '\x1d'],
    ['Ctrl+^ → RS', k('^', { ctrlKey: true, shiftKey: true }), linux, '\x1e'],
    ['Ctrl+_ → US', k('_', { ctrlKey: true, shiftKey: true }), linux, '\x1f'],
    ['Ctrl+? → DEL', k('?', { ctrlKey: true, shiftKey: true }), linux, '\x7f'],
    ['Ctrl+1 (unmapped) sends the plain char', k('1', { ctrlKey: true }), linux, '1'],
    // Alt as ESC prefix
    ['Alt+b', k('b', { altKey: true }), linux, `${ESC}b`],
    ['Alt+Shift+B', k('B', { altKey: true, shiftKey: true }), linux, `${ESC}B`],
    ['Alt+.', k('.', { altKey: true }), linux, `${ESC}.`],
    ['Alt+Enter', k('Enter', { altKey: true }), linux, `${ESC}\r`],
    ['Alt+Backspace', k('Backspace', { altKey: true }), linux, `${ESC}\x7f`],
    ['Ctrl+Alt+d', k('d', { ctrlKey: true, altKey: true }), linux, `${ESC}\x04`],
    // macOS: a non-ASCII Option composition is Meta; recover the base from `code`
    ['Option+b on mac (key=∫, code=KeyB)', k('∫', { altKey: true, code: 'KeyB' }), mac, `${ESC}b`],
    ['Option+Shift+B on mac', k('ı', { altKey: true, shiftKey: true, code: 'KeyB' }), mac, `${ESC}B`],
    ['Option+1 on mac (key=¡, code=Digit1)', k('¡', { altKey: true, code: 'Digit1' }), mac, `${ESC}1`],
    // …but an Option composition that IS printable ASCII is text (F8)
    ['Option+` → ~ on mac', k('~', { altKey: true, code: 'Backquote' }), mac, '~'],
    ['German Option+l → @', k('@', { altKey: true, code: 'KeyL' }), mac, '@'],
    ['German Option+5 → [', k('[', { altKey: true, code: 'Digit5' }), mac, '['],
    ['German Shift+Option+7 → \\', k('\\', { altKey: true, shiftKey: true, code: 'Digit7' }), mac, '\\'],
    // Enter: Option/Shift+Enter insert a newline in Claude Code (ESC CR)
    ['Shift+Enter', k('Enter', { shiftKey: true }), linux, `${ESC}\r`],
    ['Option+Enter on mac', k('Enter', { altKey: true }), mac, `${ESC}\r`],
    ['Ctrl+Enter stays a plain CR', k('Enter', { ctrlKey: true }), linux, '\r'],
    // F13–F20 (tilde family, xterm codes 25/26/28/29/31/32/33/34)
    ['F13', k('F13'), linux, `${ESC}[25~`],
    ['F14', k('F14'), linux, `${ESC}[26~`],
    ['F15', k('F15'), linux, `${ESC}[28~`],
    ['F16', k('F16'), linux, `${ESC}[29~`],
    ['F17', k('F17'), linux, `${ESC}[31~`],
    ['F18', k('F18'), linux, `${ESC}[32~`],
    ['F19', k('F19'), linux, `${ESC}[33~`],
    ['F20', k('F20'), linux, `${ESC}[34~`],
    ['Shift+F13', k('F13', { shiftKey: true }), linux, `${ESC}[25;2~`],
    // AltGr on Windows/Linux arrives as Ctrl+Alt and composes text, not a chord
    ['AltGr+v → @ on linux', k('@', { ctrlKey: true, altKey: true, code: 'KeyV' }), linux, '@'],
    ['AltGr+7 → { on linux', k('{', { ctrlKey: true, altKey: true, code: 'Digit7' }), linux, '{'],
    ['AltGr+a → ą on linux', k('ą', { ctrlKey: true, altKey: true, code: 'KeyA' }), linux, 'ą'],
    ['Ctrl+Alt+2 (no composition) is still a chord', k('2', { ctrlKey: true, altKey: true, code: 'Digit2' }), linux, `${ESC}\x00`],
    ['Ctrl+Alt+d (letter) is still a chord', k('d', { ctrlKey: true, altKey: true, code: 'KeyD' }), linux, `${ESC}\x04`],
    ['Ctrl+Alt+@ on mac stays a chord', k('@', { ctrlKey: true, altKey: true, code: 'KeyV' }), mac, `${ESC}\x00`],
    // Not ours
    ['Cmd+c is an app chord', k('c', { metaKey: true }), mac, null],
    ['Cmd+Left is an app chord', k('ArrowLeft', { metaKey: true }), mac, null],
    ['Super+a on linux is not forwarded', k('a', { metaKey: true }), linux, null],
    ['bare Shift', k('Shift', { shiftKey: true }), linux, null],
    ['bare Control', k('Control', { ctrlKey: true }), linux, null],
    ['CapsLock', k('CapsLock'), linux, null],
    ['Dead key', k('Dead'), linux, null],
  ];

  it.each(cases)('%s', (_name, ev, opts, expected) => {
    expect(keyToBytes(ev, opts)).toBe(expected);
  });
});

// Slovak (and Czech) Mac layouts hide the whole programming punctuation set
// behind Option. Before F8 each of these arrived as a Meta chord (ESC v was
// Claude's "paste image", ESC b jumped a word back) and the character could
// not be typed at all — the owner's call is "text first, Meta fallback".
describe('terminal_keys — Slovak macOS Option layer sends text, not Meta', () => {
  const slovak: Array<[string, string, string]> = [
    // [code, composed key, expected bytes]
    ['KeyV', '@', '@'],
    ['Digit2', '@', '@'],
    ['KeyF', '[', '['],
    ['KeyG', ']', ']'],
    ['KeyB', '{', '{'],
    ['KeyN', '}', '}'],
    ['KeyW', '|', '|'],
    ['KeyQ', '\\', '\\'],
    ['KeyX', '#', '#'],
    ['Backquote', '~', '~'],
  ];
  it.each(slovak)('Option+%s composing %s', (code, key, expected) => {
    expect(keyToBytes(k(key, { altKey: true, code }), mac)).toBe(expected);
  });

  it('still sends Meta for the US layout glyphs that are not ASCII', () => {
    expect(keyToBytes(k('∫', { altKey: true, code: 'KeyB' }), mac)).toBe(`${ESC}b`);
    expect(keyToBytes(k('ƒ', { altKey: true, code: 'KeyF' }), mac)).toBe(`${ESC}f`);
    expect(keyToBytes(k('∂', { altKey: true, code: 'KeyD' }), mac)).toBe(`${ESC}d`);
  });
});

describe('terminal_keys.detectMac', () => {
  it('recognises macOS / iOS platforms and Macintosh UAs', () => {
    expect(detectMac({ platform: 'MacIntel' })).toBe(true);
    expect(detectMac({ platform: '', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X)' })).toBe(true);
    expect(detectMac({ platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; Linux)' })).toBe(false);
    expect(detectMac({ platform: 'Win32' })).toBe(false);
    expect(detectMac(undefined)).toBe(false);
  });
});
