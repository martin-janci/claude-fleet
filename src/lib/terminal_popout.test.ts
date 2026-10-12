// Pop-out terminals (redesign step 5.4): the window label is the whole
// contract between the backend that opens the window and the page inside it.
import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 'term-7-sh2') }));
vi.mock('@tauri-apps/api/webview', () => ({ getCurrentWebview: () => ({ label: 'main' }) }));
const win = vi.hoisted(() => ({ close: vi.fn(async () => {}), setFocus: vi.fn(async () => {}) }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => win }));
const ev = vi.hoisted(() => ({
  emitTo: vi.fn(async () => {}),
  handler: null as null | ((e: { payload: unknown }) => void),
}));
vi.mock('@tauri-apps/api/event', () => ({
  emitTo: ev.emitTo,
  listen: vi.fn(async (_name: string, h: (e: { payload: unknown }) => void) => {
    ev.handler = h;
    return () => {};
  }),
}));

import { invoke } from '@tauri-apps/api/core';
import {
  currentPopout,
  listenForPopBackIn,
  openTerminalWindow,
  parsePopoutLabel,
  popBackIn,
  popoutTitle,
  POP_BACK_IN_EVENT,
  SEND_KEYS,
  sendKeysText,
} from './terminal_popout';

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

describe('Pop back in (Agent board, M15 G4.4)', () => {
  it('tells the main window which terminal to show, then closes this window', async () => {
    const r = await popBackIn({ label: 'term-7-sh2', sessionId: 7, shell: 2 });
    expect(r.ok).toBe(true);
    expect(ev.emitTo).toHaveBeenCalledWith('main', POP_BACK_IN_EVENT, { sessionId: 7, shell: 2 });
    expect(win.close).toHaveBeenCalledTimes(1);
    // The event goes first: a closed window sends nothing.
    expect(ev.emitTo.mock.invocationCallOrder[0]).toBeLessThan(win.close.mock.invocationCallOrder[0]);
  });

  it('keeps the window open and says why when the main window cannot be told', async () => {
    ev.emitTo.mockRejectedValueOnce(new Error('no main window'));
    win.close.mockClear();
    const r = await popBackIn({ label: 'term-7-agent', sessionId: 7, shell: null });
    expect(r.ok).toBe(false);
    expect(win.close).not.toHaveBeenCalled();
  });

  it('the main window shows what a well-formed request names and comes forward', async () => {
    const show = vi.fn();
    await listenForPopBackIn(show);
    ev.handler!({ payload: { sessionId: 7, shell: null } });
    ev.handler!({ payload: { sessionId: 7, shell: 3 } });
    for (const bad of [null, { sessionId: 0, shell: null }, { sessionId: 7, shell: 10 }, { sessionId: 'x', shell: 1 }]) {
      ev.handler!({ payload: bad });
    }
    expect(show.mock.calls).toEqual([[{ sessionId: 7, shell: null }], [{ sessionId: 7, shell: 3 }]]);
    expect(win.setFocus).toHaveBeenCalledTimes(2);
  });
});

describe('Send keys… (Agent board, M15 G4.4)', () => {
  it('offers the keys a pop-out may not pass on, as the bytes the grid sends', () => {
    const by = Object.fromEntries(SEND_KEYS.map((k) => [k.id, k.bytes]));
    expect(by).toMatchObject({ esc: '\x1b', enter: '\r', 'ctrl-c': '\x03', 'shift-tab': '\x1b[Z' });
    expect(new Set(SEND_KEYS.map((k) => k.id)).size).toBe(SEND_KEYS.length);
  });

  it('sends text as typed, line breaks as Returns, and a Return after only when asked', () => {
    expect(sendKeysText('/compact', true)).toBe('/compact\r');
    expect(sendKeysText('/compact', false)).toBe('/compact');
    expect(sendKeysText('a\nb\r\nc', false)).toBe('a\rb\rc');
  });
});
