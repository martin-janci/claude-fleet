// Multi-hub Phase 3: one id space. An extra fleet's session 3 and the
// primary's session 3 are two sessions: what the panel keeps per session
// (draft, outbox) never crosses between them, and every call names the
// session's own fleet.
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ConversationPanel from './ConversationPanel.svelte';
import {
  PRIMARY,
  _resetFleetsForTest,
  _setFleetRowsForTest,
  _setFleetsForTest,
  keyTarget,
  sessionKey,
} from './fleets';
import { outbox } from './outbox';
import { sendAnswer } from './answer_send';
import { pendingInputFor } from './pending_input';
import type { SessionRow } from './sessions';
import { fleetInfo } from './fleets_fixture';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const company = fleetInfo('company', { url: 'https://fleet.papayapos.dev', color: '#e8833a', fleet_id: 'bbbbbbbb-2222' });

const row = {
  id: 3,
  tmux_name: 'pos-3',
  host_alias: 'mac',
  kind: 'work',
  status: 'running',
  last_activity_at: 1,
  claude_status: 'idle',
  claude_session_id: 'c-3',
  turn_seq: 1,
} as SessionRow;
const theirs = { ...row, tmux_name: 'their-3', fleet: 'company' } as SessionRow;

const EMPTY = { truncated: false, context: null, events: [], turns: [] };
const PROBE = { claude_status: 'idle', stuck_kind: null, current_activity: null, waiting_for: null, spinner: null };

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string) =>
    cmd === 'session_conversation' ? EMPTY : cmd === 'session_conversations' ? [] : cmd === 'session_activity' ? PROBE : null,
  );
  _resetFleetsForTest();
  _setFleetsForTest([{ ...company, id: PRIMARY, primary: true, label: 'home', fleet_id: 'aaaaaaaa-1111' }, company]);
  _setFleetRowsForTest('company', { sessions: new Map([[3, theirs]]), hosts: new Map(), projects: [], error: null, loaded: true });
  outbox.resetForTests();
});
afterEach(() => cleanup());

describe('sessionKey', () => {
  it("keeps the primary's ids and gives each extra fleet's session its own negative key", () => {
    expect(sessionKey(row)).toBe(3);
    const k = sessionKey(theirs);
    expect(k).toBeLessThan(0);
    expect(sessionKey({ id: 3, fleet: 'company' })).toBe(k);
    expect(sessionKey({ id: 3, fleet: 'private' })).not.toBe(k);
    expect(sessionKey({ id: 3, fleet: PRIMARY })).toBe(3);
    expect(keyTarget(k)).toEqual({ fleet: 'company', id: 3 });
    expect(keyTarget(3)).toEqual({ fleet: PRIMARY, id: 3 });
  });
});

describe('ConversationPanel: one id space', () => {
  it("a draft typed on the primary's session 3 never shows on another fleet's session 3", async () => {
    const view = render(ConversationPanel, { props: { session: row, visible: true } });
    await tick();
    const box = () => screen.getByTestId('conv-composer-input') as HTMLTextAreaElement;
    await fireEvent.input(box(), { target: { value: 'for home' } });
    await view.rerender({ session: theirs, visible: true });
    await tick();
    expect(box().value).toBe('');
    await fireEvent.input(box(), { target: { value: 'for company' } });
    await view.rerender({ session: row, visible: true });
    await tick();
    expect(box().value).toBe('for home');
  });

  it('a send to the other fleet goes to its hub and waits in its own line', async () => {
    render(ConversationPanel, { props: { session: theirs, visible: true } });
    await tick();
    await fireEvent.input(screen.getByTestId('conv-composer-input'), { target: { value: 'go on' } });
    await fireEvent.click(screen.getByTestId('conv-composer-send'));
    await vi.waitFor(() => expect(inv.mock.calls.some(([cmd]) => cmd === 'send_prompt')).toBe(true));
    const sent = inv.mock.calls.find(([cmd]) => cmd === 'send_prompt')!;
    expect(sent[1]).toMatchObject({ fleet: 'company', args: { tmux_name: 'their-3', prompt: 'go on' } });
    const msgs = get(outbox.store).msgs;
    expect(msgs[3]).toBeUndefined();
    expect(msgs[sessionKey(theirs)]).toHaveLength(1);
  });

  it('reads the conversation and its list from that fleet', async () => {
    render(ConversationPanel, { props: { session: theirs, visible: true } });
    await vi.waitFor(() => expect(inv.mock.calls.some(([cmd]) => cmd === 'session_conversations')).toBe(true));
    for (const cmd of ['session_conversation', 'session_conversations']) {
      const call = inv.mock.calls.find(([c]) => c === cmd)!;
      expect(call[1]).toMatchObject({ fleet: 'company', args: { session_id: 3 } });
    }
  });
});

describe('sendAnswer on another fleet', () => {
  it("re-reads that fleet's pane and presses the key there", async () => {
    const dialog = { kind: 'permission', question: 'Allow?', options: [{ n: 1, label: 'Yes', selected: true }], detail: null };
    const probe = { ...PROBE, claude_status: 'blocked', pending_input: dialog };
    inv.mockImplementation(async (cmd: string) => (cmd === 'session_activity' ? probe : null));
    const view = pendingInputFor({ rowStatus: 'blocked', rowStuck: null, rowPending: null, probe: probe as never })!;
    const blocked = { ...theirs, claude_status: 'blocked' } as SessionRow;
    expect(await sendAnswer(blocked, view, view.options[0].key!)).toEqual({ ok: true });
    const read = inv.mock.calls.find(([cmd]) => cmd === 'session_activity')!;
    expect(read[1]).toEqual({ args: { session_id: 3 }, fleet: 'company' });
    const press = inv.mock.calls.find(([cmd]) => cmd === 'send_prompt')!;
    expect(press[1]).toMatchObject({ fleet: 'company', args: { tmux_name: 'their-3', prompt: '', keys: '1' } });
  });
});
