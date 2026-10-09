// Pop-out terminals (redesign step 5.4): the window label is the whole
// contract between the backend that opens the window and the page inside it.
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 'term-7-sh2') }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ label: 'main' }) }));

import { invoke } from '@tauri-apps/api/core';
import { currentPopout, openTerminalWindow, parsePopoutLabel, popoutTitle } from './terminal_popout';

describe('terminal pop-out labels', () => {
  it('reads the session and the terminal from the label `popout_label` builds', () => {
    expect(parsePopoutLabel('term-12-agent')).toEqual({ label: 'term-12-agent', sessionId: 12, shell: null });
    expect(parsePopoutLabel('term-12-sh3')).toEqual({ label: 'term-12-sh3', sessionId: 12, shell: 3 });
  });

  it('the main window and anything malformed are not pop-outs', () => {
    for (const label of ['main', '', null, undefined, 'term-0-agent', 'term-1-sh0', 'term-1-sh10', 'term-1-sh', 'term-x-agent', 'term-1-agent-x']) {
      expect(parsePopoutLabel(label)).toBeNull();
    }
    expect(currentPopout()).toBeNull();
  });

  it('opens a window through the backend, naming the terminal and a title', async () => {
    const r = await openTerminalWindow(7, 2, popoutTitle('api', 2));
    expect(r).toEqual({ ok: true, value: 'term-7-sh2' });
    expect(invoke).toHaveBeenCalledWith('open_terminal_window', {
      args: { session_id: 7, shell: 2, title: 'api · Shell 2' },
    });
    expect(popoutTitle('api', null)).toBe('api');
  });
});
