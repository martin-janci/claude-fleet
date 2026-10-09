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
 */
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { invokeCmd, type Result } from './result';
import { MAX_SHELL_TERMINALS } from './terminals';

export interface PopoutTarget {
  /** The window's label, which is also its pane's pty id. */
  label: string;
  sessionId: number;
  /** The shell terminal; `null` is the agent's pane. */
  shell: number | null;
}

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
