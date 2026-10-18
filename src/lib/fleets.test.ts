import { describe, it, expect, vi, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';

import {
  FLEET_SCOPED,
  PRIMARY,
  applyFleetEvent,
  connectionWord,
  emptyRows,
  filterShows,
  fleetAttachCommand,
  fleetKey,
  fleetKillSession,
  fleetRows,
  fleetSendPrompt,
  fleetFilter,
  loadFleetRows,
  multiFleet,
  onFleetEvent,
  showPrimary,
  visibleExtraFleets,
  _resetFleetsForTest,
  _setFleetsForTest,
  type FleetInfo,
} from './fleets';
import type { SessionRow } from './sessions';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

function fleet(id: string, extra: Partial<FleetInfo> = {}): FleetInfo {
  return {
    id,
    label: id,
    url: id === PRIMARY ? null : `https://${id}.example.com`,
    color: '#5b8def',
    primary: id === PRIMARY,
    local: id === PRIMARY,
    client_name: null,
    connection: id === PRIMARY ? null : { state: 'connected' },
    unavailable: null,
    ...extra,
  };
}

function session(id: number, extra: Partial<SessionRow> = {}): SessionRow {
  return { id, tmux_name: `s${id}`, host_alias: 'mac', status: 'running', last_activity_at: id, ...extra } as SessionRow;
}

beforeEach(() => {
  inv.mockReset();
  inv.mockResolvedValue([]);
  _resetFleetsForTest();
});

describe('fleet rows', () => {
  it('keeps colliding ids apart per fleet', () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company'), fleet('lab')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1, { tmux_name: 'pos' }) });
    onFleetEvent({ fleet: 'lab', name: 'session:updated', payload: session(1, { tmux_name: 'exp' }) });
    const rows = get(fleetRows);
    expect(rows.get('company')?.sessions.get(1)?.tmux_name).toBe('pos');
    expect(rows.get('lab')?.sessions.get(1)?.tmux_name).toBe('exp');
    expect(fleetKey('company', 1)).not.toBe(fleetKey('lab', 1));
  });

  it('never applies a primary-tagged event to the extra-fleet stores', () => {
    onFleetEvent({ fleet: PRIMARY, name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).size).toBe(0);
  });

  it('applies kills, hosts and projects, and ignores what it does not keep', () => {
    const rows = emptyRows();
    applyFleetEvent(rows, { name: 'session:created', payload: session(2) });
    applyFleetEvent(rows, { name: 'session:killed', payload: { id: 2 } });
    expect(rows.sessions.size).toBe(0);
    applyFleetEvent(rows, { name: 'host:added', payload: { alias: 'mac', reachable: false } });
    applyFleetEvent(rows, { name: 'host:pinged', payload: { alias: 'mac', reachable: true } });
    expect(rows.hosts.get('mac')?.reachable).toBe(true);
    applyFleetEvent(rows, { name: 'host:pinged', payload: { alias: 'ghost', reachable: true } });
    expect(rows.hosts.has('ghost')).toBe(false);
    applyFleetEvent(rows, { name: 'host:removed', payload: { alias: 'mac' } });
    expect(rows.hosts.size).toBe(0);
    applyFleetEvent(rows, { name: 'project:updated', payload: { id: 7, owner: 'papayapos', repo: 'pos' } });
    expect(rows.projects[0].project.repo).toBe('pos');
    expect(applyFleetEvent(rows, { name: 'task:updated', payload: { id: 1 } })).toBeNull();
    expect(applyFleetEvent(rows, { name: 'session:updated', payload: 'garbage' })).toBeNull();
  });

  it("re-lists a fleet after its bridge's gap and when its link comes up", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company', { connection: { state: 'connecting' } })]);
    onFleetEvent({ fleet: 'company', name: 'hub:connection', payload: { state: 'connected' } });
    expect(get(visibleExtraFleets)[0].fleet.connection).toEqual({ state: 'connected' });
    onFleetEvent({ fleet: 'company', name: 'hub:resynced', payload: null });
    await Promise.resolve();
    const listed = inv.mock.calls.filter(([cmd]) => cmd === 'list_sessions').map(([, a]) => a);
    expect(listed).toEqual([{ fleet: 'company' }, { fleet: 'company' }]);
  });

  it('a failed list shows on the fleet rather than wiping its rows', async () => {
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(3) });
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_sessions') throw { code: 'E_HUB_UNREACHABLE', message: 'cannot reach' };
      return [];
    });
    await loadFleetRows('company');
    const rows = get(fleetRows).get('company')!;
    expect(rows.sessions.get(3)).toBeTruthy();
    expect(rows.error).toBe('cannot reach');
  });
});

describe('the filter', () => {
  it('shows only the primary switcher-free when there is no extra fleet', () => {
    _setFleetsForTest([fleet(PRIMARY)]);
    expect(get(multiFleet)).toBe(false);
    expect(get(showPrimary)).toBe(true);
  });

  it('narrows to one fleet, and a gone fleet shows everything', () => {
    const known = [fleet(PRIMARY), fleet('company')];
    _setFleetsForTest(known);
    expect(get(multiFleet)).toBe(true);
    fleetFilter.set('company');
    expect(get(showPrimary)).toBe(false);
    expect(get(visibleExtraFleets).map((v) => v.fleet.id)).toEqual(['company']);
    fleetFilter.set(PRIMARY);
    expect(get(showPrimary)).toBe(true);
    expect(get(visibleExtraFleets)).toEqual([]);
    expect(filterShows('vanished', PRIMARY, known)).toBe(true);
  });
});

describe('routing', () => {
  it("every action on an extra fleet's row names that fleet", async () => {
    inv.mockResolvedValue(null);
    await fleetSendPrompt('company', session(1), 'hi');
    await fleetKillSession('company', session(1));
    expect(inv.mock.calls).toEqual([
      ['send_prompt', { args: { host_alias: 'mac', tmux_name: 's1', prompt: 'hi', submit: true }, fleet: 'company' }],
      ['kill_session', { args: { host_alias: 'mac', name: 's1' }, fleet: 'company' }],
    ]);
  });

  it("mirrors the backend's FLEET_SCOPED list exactly", () => {
    const src = readFileSync('src-tauri/src/backend/fleets.rs', 'utf8');
    const block = src.split('pub const FLEET_SCOPED: &[&str] = &[')[1].split('];')[0];
    const backend = [...block.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
    expect([...FLEET_SCOPED]).toEqual(backend);
  });

  it('quotes the attach command for a shell', () => {
    expect(fleetAttachCommand({ host_alias: 'mac', tmux_name: 'dev-x' })).toBe("ssh -t mac tmux attach -t =dev-x");
    expect(fleetAttachCommand({ host_alias: 'mac', tmux_name: "it's" })).toBe(
      "ssh -t mac tmux attach -t '=it'\\''s'",
    );
  });
});

describe('connection words', () => {
  it('names every state in words, never colour alone', () => {
    expect(connectionWord({ connection: null, unavailable: 'no token', local: false }).tone).toBe('bad');
    expect(connectionWord({ connection: null, unavailable: null, local: true }).word).toBe('this machine');
    expect(connectionWord({ connection: { state: 'connected' }, unavailable: null, local: false }).word).toBe('live');
    expect(
      connectionWord({ connection: { state: 'hub_too_old', hub_contract: 1, min_contract: 13 }, unavailable: null, local: false })
        .word,
    ).toBe('hub too old');
  });
});
