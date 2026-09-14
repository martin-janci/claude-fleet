/**
 * Keyboard → terminal byte mapping, following xterm's key table so remote
 * apps (tmux, vim, claude's TUI) see exactly what they would from a real
 * terminal. Pure: takes a plain key descriptor rather than a KeyboardEvent
 * so it can be table-tested without a DOM.
 *
 * Reference: xterm ctlseqs "PC-Style Function Keys" — modifiers are encoded
 * as `1 + (Shift=1 | Alt=2 | Ctrl=4 | Meta=8)` in the second CSI parameter,
 * e.g. Ctrl+Right → `ESC [ 1 ; 5 C`, Shift+F5 → `ESC [ 15 ; 2 ~`.
 */

export interface KeyLike {
  key: string;
  /** Physical key (`KeyA`, `Digit1`, …). Used on macOS to recover the base
   *  character for Option+key, where `key` is the composed glyph (`å`). */
  code?: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

export interface KeyOpts {
  /** DECSET ?1 — arrows/Home/End as SS3 (`ESC O A`) instead of CSI. */
  appCursor: boolean;
  /** macOS: Option is the ESC-prefix key and Cmd is reserved for the app. */
  isMac: boolean;
}

/** Best-effort platform detection from `navigator`; false in jsdom. */
export function detectMac(nav: { platform?: string; userAgent?: string } | undefined): boolean {
  if (!nav) return false;
  return /Mac|iPhone|iPad|iPod/.test(nav.platform ?? '') || /Macintosh/.test(nav.userAgent ?? '');
}

/** xterm modifier parameter, or 0 when no modifier is held. */
function modParam(ev: KeyLike, includeMeta: boolean): number {
  let m = 0;
  if (ev.shiftKey) m |= 1;
  if (ev.altKey) m |= 2;
  if (ev.ctrlKey) m |= 4;
  if (includeMeta && ev.metaKey) m |= 8;
  return m === 0 ? 0 : m + 1;
}

/** Cursor-key family: CSI/SS3 final byte with optional `1;m` modifier. */
function cursorKey(final: string, mod: number, appCursor: boolean): string {
  if (mod === 0) return (appCursor ? '\x1bO' : '\x1b[') + final;
  return `\x1b[1;${mod}${final}`;
}

/** Tilde family (`CSI n ~`): Insert/Delete/PgUp/PgDn/F5–F12. */
function tildeKey(n: number, mod: number): string {
  return mod === 0 ? `\x1b[${n}~` : `\x1b[${n};${mod}~`;
}

/** SS3 family (F1–F4): `ESC O P` unmodified, `CSI 1 ; m P` with modifiers. */
function ss3Key(final: string, mod: number): string {
  return mod === 0 ? `\x1bO${final}` : `\x1b[1;${mod}${final}`;
}

const CURSOR_FINALS: Record<string, string> = {
  ArrowUp: 'A',
  ArrowDown: 'B',
  ArrowRight: 'C',
  ArrowLeft: 'D',
  Home: 'H',
  End: 'F',
};

const TILDE_CODES: Record<string, number> = {
  Insert: 2,
  Delete: 3,
  PageUp: 5,
  PageDown: 6,
  F5: 15,
  F6: 17,
  F7: 18,
  F8: 19,
  F9: 20,
  F10: 21,
  F11: 23,
  F12: 24,
};

const SS3_FINALS: Record<string, string> = { F1: 'P', F2: 'Q', F3: 'R', F4: 'S' };

/** Ctrl + this printable → C0 byte. Letters are handled arithmetically. */
const CTRL_PUNCT: Record<string, string> = {
  ' ': '\x00',
  '@': '\x00',
  '2': '\x00',
  '[': '\x1b',
  '3': '\x1b',
  '\\': '\x1c',
  '4': '\x1c',
  ']': '\x1d',
  '5': '\x1d',
  '^': '\x1e',
  '6': '\x1e',
  '_': '\x1f',
  '7': '\x1f',
  '/': '\x1f',
  '8': '\x7f',
  '?': '\x7f',
};

/** Recover the base printable for an Alt/Option chord from `code` when the
 *  browser composed `key` into something else (macOS Option+a → `å`). */
function baseChar(ev: KeyLike): string | null {
  const code = ev.code ?? '';
  if (/^Key[A-Z]$/.test(code)) {
    const letter = code.slice(3);
    return ev.shiftKey ? letter : letter.toLowerCase();
  }
  if (/^Digit[0-9]$/.test(code) && !ev.shiftKey) return code.slice(5);
  if (ev.key.length === 1) return ev.key;
  return null;
}

/**
 * Translate one keydown into the bytes a terminal sends, or null when the
 * key is not ours to forward (Cmd/Super chords, unknown special keys, bare
 * modifiers) so the caller leaves the browser default alone.
 */
export function keyToBytes(ev: KeyLike, opts: KeyOpts): string | null {
  const { key } = ev;
  // Cmd (mac) / Super (elsewhere) chords belong to the app, never the PTY.
  if (ev.metaKey) return null;
  // Bare modifier presses carry no bytes.
  if (key === 'Shift' || key === 'Control' || key === 'Alt' || key === 'Meta' || key === 'Dead' || key === 'Unidentified') {
    return null;
  }
  const mod = modParam(ev, false);
  const altPrefix = ev.altKey ? '\x1b' : '';

  if (key in CURSOR_FINALS) return cursorKey(CURSOR_FINALS[key], mod, opts.appCursor);
  if (key in TILDE_CODES) return tildeKey(TILDE_CODES[key], mod);
  if (key in SS3_FINALS) return ss3Key(SS3_FINALS[key], mod);

  switch (key) {
    case 'Enter':
      return altPrefix + '\r';
    case 'Backspace':
      // Ctrl+Backspace → BS (word-delete in readline), plain → DEL.
      return altPrefix + (ev.ctrlKey ? '\x08' : '\x7f');
    case 'Tab':
      return ev.shiftKey ? '\x1b[Z' : altPrefix + '\t';
    case 'Escape':
      return altPrefix + '\x1b';
    default:
      break;
  }

  if (key.length !== 1) return null; // other named keys (CapsLock, F13+, …)

  // Ctrl + printable → C0 control byte (Shift is ignored, as in xterm).
  if (ev.ctrlKey) {
    const k = key.toLowerCase();
    if (k >= 'a' && k <= 'z') return altPrefix + String.fromCharCode(k.charCodeAt(0) - 96);
    const punct = CTRL_PUNCT[key] ?? CTRL_PUNCT[k];
    if (punct !== undefined) return altPrefix + punct;
    // Unmapped Ctrl chord (Ctrl+1, Ctrl+.) — xterm sends the plain char.
    return altPrefix + key;
  }

  // Alt / Option + printable → ESC prefix + the base character.
  if (ev.altKey) {
    const base = opts.isMac ? baseChar(ev) : key;
    return base === null ? null : '\x1b' + base;
  }

  return key;
}
