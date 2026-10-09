import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const { box, focus } = vi.hoisted(() => {
  let subs: ((v: unknown) => void)[] = [];
  let value: { msgs: Record<number, unknown[]> } = { msgs: {} };
  return {
    box: {
      set(v: { msgs: Record<number, unknown[]> }) {
        value = v;
        for (const s of subs) s(v);
      },
      subscribe(fn: (v: unknown) => void) {
        subs.push(fn);
        fn(value);
        return () => (subs = subs.filter((s) => s !== fn));
      },
    },
    focus: vi.fn((..._a: unknown[]) => true),
  };
});
vi.mock('./outbox', () => ({ outbox: { store: box } }));
vi.mock('./session_focus', () => ({ focusSession: (...a: unknown[]) => focus(...a) }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import ControlRouteReceipts from './ControlRouteReceipts.svelte';
import { resetReceiptsForTests } from './control_route';

// Redesign step 9.9 (Jev K2): the receipt under a message sent in Control.

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const TARGETS = [
  { kind: 'mission', id: 3, name: 'Hub federation v2' },
  { kind: 'session', id: 9, name: 'fed-v2' },
];
let route: unknown;

const msg = (id: string, text: string, state = 'sent') => ({ id, kind: 'prompt', text, state });

async function send(...m: unknown[]) {
  box.set({ msgs: { 1: m } });
  await tick();
  await tick();
  await tick();
}

beforeEach(() => {
  resetReceiptsForTests();
  box.set({ msgs: {} });
  focus.mockClear();
  inv.mockReset();
  route = {
    outcome: 'proposed',
    target: 'm3',
    proposal: { feature: 'control_route', value: 'm3', source: 'jev', confidence_pct: 86, run_id: 41 },
    targets: TARGETS,
    run_id: 41,
  };
  inv.mockImplementation(async (cmd: string) => (cmd === 'control_route_propose' ? route : true));
});

describe('Control routing receipts', () => {
  it('a sent message gets "About <mission> · Proposed by Jev · Change"', async () => {
    render(ControlRouteReceipts, { sessionId: 1 });
    await send(msg('a', 'How far did the federation handshake get?'));
    expect(inv).toHaveBeenCalledWith('control_route_propose', { text: 'How far did the federation handshake get?' });
    expect(screen.getByTestId('control-route-target').textContent).toBe('About Hub federation v2');
    expect(screen.getByTestId('control-route-proposed').textContent).toContain('Proposed by Jev');
    expect(screen.getByTestId('control-route-proposed').textContent).toContain('86%');
  });

  it('only messages sent while Control shows are routed, each once', async () => {
    box.set({ msgs: { 1: [msg('old', 'an earlier message about the federation')] } });
    render(ControlRouteReceipts, { sessionId: 1 });
    await tick();
    await send(msg('old', 'an earlier message about the federation'), msg('b', 'still sending this one', 'sending'));
    expect(inv).not.toHaveBeenCalled();
    await send(msg('b', 'still sending this one'));
    await send(msg('b', 'still sending this one', 'received'));
    expect(inv).toHaveBeenCalledTimes(1);
  });

  it('Change records the pick as the follow-up and shows it', async () => {
    render(ControlRouteReceipts, { sessionId: 1 });
    await send(msg('a', 'How far did the federation handshake get?'));
    await fireEvent.click(screen.getByTestId('control-route-proposed-change'));
    const choices = screen.getAllByTestId('control-route-choice');
    expect(choices.map((c) => c.textContent)).toEqual(['Hub federation v2', 'fed-v2']);
    await fireEvent.click(choices[1]);
    await tick();
    expect(inv).toHaveBeenCalledWith('control_route_follow', { runId: 41, chosen: 's9' });
    expect(screen.getByTestId('control-route-target').textContent).toBe('About fed-v2');
    // A second change updates the receipt but records nothing more.
    await fireEvent.click(screen.getByTestId('control-route-change'));
    await fireEvent.click(screen.getByTestId('control-route-choice-control'));
    await tick();
    expect(screen.getByTestId('control-route-target').textContent).toBe('For Control itself');
    expect(inv.mock.calls.filter((c) => c[0] === 'control_route_follow')).toHaveLength(1);
  });

  it('a short message asks instead, with nothing pre-selected', async () => {
    route = { outcome: 'ask', targets: TARGETS };
    render(ControlRouteReceipts, { sessionId: 1 });
    await send(msg('a', 'yes, that one'));
    expect(screen.getByTestId('control-route-ask').textContent).toBe('Which mission or session is this about?');
    expect(screen.queryByTestId('control-route-proposed')).toBeNull();
    await fireEvent.click(screen.getAllByTestId('control-route-choice')[0]);
    await tick();
    // No run behind a rule's question: nothing to record.
    expect(inv.mock.calls.some((c) => c[0] === 'control_route_follow')).toBe(false);
    expect(screen.getByTestId('control-route-target').textContent).toBe('About Hub federation v2');
  });

  it('opening a proposed session confirms it', async () => {
    route = { ...(route as object), target: 's9', proposal: { value: 's9', source: 'jev', confidence_pct: 70 } };
    render(ControlRouteReceipts, { sessionId: 1 });
    await send(msg('a', 'what is fed-v2 stuck on right now'));
    await fireEvent.click(screen.getByTestId('control-route-target'));
    await tick();
    expect(focus).toHaveBeenCalledWith(9, 'fed-v2');
    expect(inv).toHaveBeenCalledWith('control_route_follow', { runId: 41, chosen: 's9' });
  });

  it('none shows nothing', async () => {
    route = { outcome: 'none', targets: [] };
    render(ControlRouteReceipts, { sessionId: 1 });
    await send(msg('a', 'please summarise the fleet for me'));
    expect(screen.queryByTestId('control-route-receipts')).toBeNull();
  });
});
