/**
 * Pop-out terminals (redesign step 5.4): a session's agent pane or one of
 * its shell terminals in a window of its own.
 *
 * The window is a second webview on the same page. It knows what to show
 * from its own label, `term-<session id>-agent` or `term-<session id>-sh<N>`
 * (`popout_label` in `src-tauri/src/commands/windows.rs`), and its pane
 * attaches under a pty id equal to that label, so it is a second
 * `tmux attach` beside the main window's, never a replacement for it.
 * Closing the window closes that attach and nothing else.
 *
 * Its bar (Agent board, M15 G4.4) has Send keys…, Clear view (this window's
 * copy of the screen only) and Pop back in, which hands the terminal back
 * to the main window through `POP_BACK_IN_EVENT` and closes this one.
 */
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invokeCmd, toIpcError, type Result } from './result';
import { MAX_SHELL_TERMINALS } from './terminals';

export interface PopoutTarget {
  /** The window's label, which is also its pane's pty id. */
  label: string;
  sessionId: number;
  /** The shell terminal; `null` is the agent's pane. */
  shell: number | null;
}

/** The main window's label (`MAIN_LABEL` in `windows.rs`). */
const MAIN_WINDOW = 'main';

const LABEL = /^term-([1-9][0-9]*)-(agent|sh([1-9][0-9]*))$/;

/** What a pop-out window shows, from its label; null for any other window. */
export function parsePopoutLabel(label: string | null | undefined): PopoutTarget | null {
  const m = label ? LABEL.exec(label) : null;
  if (!m) return null;
  const sessionId = Number(m[1]);
  const shell = m[3] === undefined ? null : Number(m[3]);
  if (!Number.isSafeInteger(sessionId)) return null;
  if (shell !== null && shell > MAX_SHELL_TERMINALS) return null;
  return { label: m[0], sessionId, shell };
}

/** This window's pop-out target, or null in the main window (and outside
 *  Tauri, where there is no webview to ask). */
export function currentPopout(): PopoutTarget | null {
  try {
    return parsePopoutLabel(getCurrentWebview().label);
  } catch {
    return null;
  }
}

/** Open the pop-out for `sessionId`'s terminal, or bring it forward. */
export function openTerminalWindow(
  sessionId: number,
  shell: number | null,
  title: string,
): Promise<Result<string>> {
  return invokeCmd<string>('open_terminal_window', {
    args: { session_id: sessionId, shell, title },
  });
}

/** The pop-out window's title: the session as its pane names it, and the
 *  terminal. */
export function popoutTitle(sessionName: string, shell: number | null): string {
  return shell == null ? sessionName : `${sessionName} · Shell ${shell}`;
}

/** The event a pop-out sends the main window to hand its terminal back
 *  (Agent board, "Pop back in"). */
export const POP_BACK_IN_EVENT = 'terminal-pop-back-in';

export interface PopBackIn {
  sessionId: number;
  /** The shell terminal; `null` is the agent's pane. */
  shell: number | null;
}

/** Pop back in: the main window shows this session on the same tab and
 *  comes forward, then this window closes (which closes its own attach and
 *  nothing else). The session and the terminal keep running. */
export async function popBackIn(target: PopoutTarget): Promise<Result<void>> {
  try {
    const payload: PopBackIn = { sessionId: target.sessionId, shell: target.shell };
    await emitTo(MAIN_WINDOW, POP_BACK_IN_EVENT, payload);
    await getCurrentWindow().close();
    return { ok: true, value: undefined };
  } catch (e) {
    return { ok: false, error: toIpcError(e) };
  }
}

/** The main window's half of Pop back in: `show` picks the session and its
 *  tab, then the window comes forward. Returns the unlisten. */
export async function listenForPopBackIn(show: (req: PopBackIn) => void): Promise<UnlistenFn> {
  return listen<PopBackIn>(POP_BACK_IN_EVENT, (e) => {
    const p = e.payload;
    if (!p || !Number.isSafeInteger(p.sessionId) || p.sessionId <= 0) return;
    const shell = p.shell == null ? null : Number(p.shell);
    if (shell !== null && !(Number.isInteger(shell) && shell >= 1 && shell <= MAX_SHELL_TERMINALS)) return;
    show({ sessionId: p.sessionId, shell });
    try {
      void getCurrentWindow()
        .setFocus()
        .catch(() => {});
    } catch {
      /* no window to bring forward (outside Tauri) */
    }
  });
}

/** The keys Send keys… offers (Agent board): the ones a pop-out window may
 *  not pass on by itself, or that are awkward to type there. Bytes are what
 *  the grid's own key table sends for the same key. */
export const SEND_KEYS: readonly { id: string; label: string; bytes: string; title: string }[] = [
  { id: 'esc', label: 'Esc', bytes: '\x1b', title: 'Escape: interrupts Claude, closes a menu' },
  { id: 'enter', label: 'Enter', bytes: '\r', title: 'Return' },
  { id: 'tab', label: 'Tab', bytes: '\t', title: 'Tab' },
  { id: 'shift-tab', label: 'Shift+Tab', bytes: '\x1b[Z', title: "Shift+Tab: Claude's mode switch" },
  { id: 'ctrl-c', label: 'Ctrl+C', bytes: '\x03', title: 'Interrupt (^C)' },
  { id: 'ctrl-d', label: 'Ctrl+D', bytes: '\x04', title: 'End of input (^D)' },
  { id: 'up', label: '↑', bytes: '\x1b[A', title: 'Up arrow' },
  { id: 'down', label: '↓', bytes: '\x1b[B', title: 'Down arrow' },
];

/** The bytes Send keys… writes for typed text: the text as typed, and a
 *  Return after it when asked. Line breaks become Returns, as a paste
 *  without bracketing would send them. */
export function sendKeysText(text: string, enter: boolean): string {
  const body = text.replace(/\r\n?|\n/g, '\r');
  return enter ? `${body}\r` : body;
}
