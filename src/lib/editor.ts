// Open in VS Code (Orbit Fleet redesign step 5.5): the session's worktree in
// VS Code on this machine, locally for a `local` session and through
// Remote - SSH for any other host. The backend asks the pane for its folder.
import { invokeCmd } from './result';
import { push } from './toasts';
import { hasNoPane, type SessionRow } from './sessions';
import { noAttachReason, type SessionAccess } from './access';

type Target = Pick<SessionRow, 'host_alias' | 'tmux_name' | 'kind'>;

/** Why this session cannot open in VS Code from here, or null when it can.
 *  Like the terminal, it needs a pane and a session this client owns: the
 *  folder is found over this machine's own ssh, which sharing never confers. */
export function editorBlockedReason(s: Target | null, access: SessionAccess): string | null {
  if (!s) return 'No session selected';
  if (hasNoPane(s)) return 'Runs outside tmux — no folder to open';
  if (access !== 'own') return noAttachReason(access) ?? 'Only your own sessions open in VS Code';
  return null;
}

/** Open the session in VS Code; a failure is a toast. */
export async function openSessionInEditor(s: Target): Promise<boolean> {
  const r = await invokeCmd<null>('open_session_in_editor', {
    args: { host_alias: s.host_alias, tmux_name: s.tmux_name },
  });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Open in VS Code: ${r.error.message}` });
    return false;
  }
  return true;
}

/** The ⌘⇧E / Ctrl+Alt+E chord: open the selection, or say why not. */
export async function openInEditorIfAllowed(s: Target | null, access: SessionAccess): Promise<boolean> {
  const why = editorBlockedReason(s, access);
  if (why || !s) {
    push({ kind: 'info', message: `Open in VS Code: ${why}` });
    return false;
  }
  return openSessionInEditor(s);
}
