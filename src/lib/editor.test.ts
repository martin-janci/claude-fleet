import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import { editorBlockedReason, openInEditorIfAllowed, openSessionInEditor } from './editor';
import { toasts } from './toasts';

const row = { host_alias: 'mercury', tmux_name: 'dev-app', kind: 'tmux' };

describe('Open in VS Code (step 5.5)', () => {
  beforeEach(() => {
    invoke.mockReset();
    toasts.set([]);
  });

  it('asks the backend for the session by alias and tmux name', async () => {
    invoke.mockResolvedValue(null);
    expect(await openSessionInEditor(row)).toBe(true);
    expect(invoke).toHaveBeenCalledWith('open_session_in_editor', {
      args: { host_alias: 'mercury', tmux_name: 'dev-app' },
    });
  });

  it('a failure is a toast in words', async () => {
    invoke.mockRejectedValue({ code: 'E_NOTFOUND', message: 'code was not found' });
    expect(await openSessionInEditor(row)).toBe(false);
    expect(get(toasts).map((t) => t.message)).toEqual(['Open in VS Code: code was not found']);
  });

  it('needs a pane and a session this client owns', () => {
    expect(editorBlockedReason(row, 'own')).toBeNull();
    expect(editorBlockedReason(null, 'own')).toBe('No session selected');
    expect(editorBlockedReason({ ...row, kind: 'bg' }, 'own')).toContain('outside tmux');
    expect(editorBlockedReason(row, 'watch')).not.toBeNull();
    expect(editorBlockedReason(row, null)).not.toBeNull();
  });

  it('the chord says why it cannot open instead of calling the backend', async () => {
    expect(await openInEditorIfAllowed({ ...row, kind: 'external' }, 'own')).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    expect(get(toasts)[0]?.message).toContain('outside tmux');
    invoke.mockResolvedValue(null);
    expect(await openInEditorIfAllowed(row, 'own')).toBe(true);
  });
});
