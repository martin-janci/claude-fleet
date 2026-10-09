/**
 * Shell terminals (redesign step 5.3): a session's terminals 0..N beside its
 * agent. Each is a tmux session of its own, `<name>--sh<N>`, which no session
 * list shows; the terminal pane attaches to it through `pty_open` like the
 * agent's, under a pty id of its own, so opening one never replaces the
 * agent's terminal.
 */
import { invokeCmd, type Result } from './result';

/** The most terminals one session keeps (`tmux::MAX_SHELL_TERMINALS`). */
export const MAX_SHELL_TERMINALS = 9;

export interface ShellTerminal {
  n: number;
  /** The tmux session the terminal pane attaches to. */
  tmux_name: string;
}

export interface ShellTerminalsResult {
  session_id: number;
  host_alias: string;
  terminals: ShellTerminal[];
  /** The terminal an `open` opened (or found already open). */
  opened?: number | null;
}

export type ShellTerminalAction = 'list' | 'open' | 'close';

export function shellTerminals(
  sessionId: number,
  action: ShellTerminalAction = 'list',
  n?: number,
): Promise<Result<ShellTerminalsResult>> {
  return invokeCmd<ShellTerminalsResult>('shell_terminals', {
    args: { session_id: sessionId, action, n: n ?? null },
  });
}

/** `tmux::shell_terminal_name`. */
export function shellTerminalName(session: string, n: number): string {
  return `${session}--sh${n}`;
}

/** The pty id a terminal pane lives under (`pty.rs`): `agent`, or `sh<N>`. */
export function terminalPtyId(n: number | null): string {
  return n == null ? 'agent' : `sh${n}`;
}

/** The tab after `current` in `agent, 1, 2, …` order, wrapping round: ⌘` . */
export function nextTerminalTab(current: number | null, open: readonly number[]): number | null {
  const tabs: (number | null)[] = [null, ...[...open].sort((a, b) => a - b)];
  const at = tabs.indexOf(current);
  return tabs[(at + 1) % tabs.length] ?? null;
}
