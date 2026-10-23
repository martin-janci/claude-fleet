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
  closeFleetSession,
  defaultFleetFor,
  fleetAddress,
  fleetCaptureSession,
  fleetChoicesFor,
  fleetInbox,
  fleetInboxCount,
  fleetListPullRequests,
  fleetNeedsCount,
  fleetOwnsOwner,
  fleetPtyId,
  fleetSelection,
  fleetSendKey,
  fleetSessionAt,
  openFleetSession,
  rememberFleetFor,
  rememberedFleetFor,
  selectedFleetSession,
  fleetAttachRefusal,
  fleetHostChoices,
  defaultFleetHost,
  fleetPinSession,
  fleetTouchViewed,
  onPrimaryConnection,
  FLEET_PALETTE,
  fleetIsDown,
  fleetTrouble,
  loadFleets,
  nextFleetColor,
  removeFleet,
  type FleetInfo,
} from './fleets';
import type { SessionRow } from './sessions';
import type { HostRow } from './hosts';

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
    applyFleetEvent(rows, { name: 'task:updated', payload: { id: 1 } });
    applyFleetEvent(rows, { name: 'session:updated', payload: 'garbage' });
    expect([rows.sessions.size, rows.hosts.size, rows.projects.length]).toEqual([0, 0, 1]);
  });

  it('copies nothing for an event it does not keep', () => {
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    const before = get(fleetRows);
    onFleetEvent({ fleet: 'company', name: 'task:updated', payload: { id: 1 } });
    onFleetEvent({ fleet: 'company', name: 'account:upserted', payload: { id: 1 } });
    expect(get(fleetRows)).toBe(before);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(2) });
    const after = get(fleetRows).get('company')!;
    expect(after.hosts).toBe(before.get('company')!.hosts);
    expect(after.sessions).not.toBe(before.get('company')!.sessions);
  });

  it('a session:updated queued behind a kill does not bring the session back', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    inv.mockResolvedValue(1);
    await fleetKillSession('company', session(1));
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).get('company')?.sessions.has(1)).toBe(false);
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 2 } });
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(2) });
    expect(get(fleetRows).get('company')?.sessions.has(2)).toBe(false);
    // A new session that took the id is let through.
    onFleetEvent({ fleet: 'company', name: 'session:created', payload: session(1, { tmux_name: 'other' }) });
    expect(get(fleetRows).get('company')?.sessions.get(1)?.tmux_name).toBe('other');
  });

  it("a kill's own answer and its frame together still let a new session take the id", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    inv.mockResolvedValue(1);
    await fleetKillSession('company', session(1));
    // The hub's frame for the same kill drops a row already gone.
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 1 } });
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).get('company')?.sessions.has(1)).toBe(false);
    onFleetEvent({ fleet: 'company', name: 'session:created', payload: session(1, { tmux_name: 'other' }) });
    expect(get(fleetRows).get('company')?.sessions.get(1)?.tmux_name).toBe('other');
  });

  it("a command's older snapshot does not overwrite a newer event", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1, { row_version: 5, claude_status: 'idle' }) });
    inv.mockResolvedValue(session(1, { row_version: 4, claude_status: 'working' }));
    await fleetTouchViewed('company', session(1));
    expect(get(fleetRows).get('company')?.sessions.get(1)?.claude_status).toBe('idle');
    inv.mockResolvedValue(session(1, { row_version: 6, pinned_at: 9 }));
    await fleetPinSession('company', session(1), true);
    expect(get(fleetRows).get('company')?.sessions.get(1)?.pinned_at).toBe(9);
  });

  it('a list answer does not undo what changed while it was out', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    let answer!: (v: unknown) => void;
    inv.mockImplementation((cmd: string) =>
      cmd === 'list_sessions' ? new Promise((r) => (answer = r)) : Promise.resolve([]),
    );
    const listing = loadFleetRows('company');
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 1 } });
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(2, { claude_status: 'idle' }) });
    answer([session(1), session(2, { claude_status: 'working' }), session(3)]);
    await listing;
    const rows = get(fleetRows).get('company')!;
    expect(rows.sessions.has(1)).toBe(false);
    expect(rows.sessions.get(2)?.claude_status).toBe('idle');
    expect(rows.sessions.has(3)).toBe(true);
  });

  // Each list_sessions call parks until the test answers it, in any order.
  function deferSessionLists(): ((rows: SessionRow[]) => void)[] {
    const answers: ((rows: SessionRow[]) => void)[] = [];
    inv.mockImplementation((cmd: string) =>
      cmd === 'list_sessions' ? new Promise((res) => answers.push(res)) : Promise.resolve([]),
    );
    return answers;
  }

  it('a kill applied while the list was in flight is not undone by its answer', async () => {
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(2) });
    const answers = deferSessionLists();
    const load = loadFleetRows('company');
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 1 } });
    answers[0]([session(1), session(2)]);
    await load;
    expect([...get(fleetRows).get('company')!.sessions.keys()]).toEqual([2]);
  });

  it('an older list answering last does not land over a newer one', async () => {
    const answers = deferSessionLists();
    const older = loadFleetRows('company');
    const newer = loadFleetRows('company');
    answers[1]([session(2)]);
    await newer;
    answers[0]([session(1), session(2)]);
    await older;
    expect([...get(fleetRows).get('company')!.sessions.keys()]).toEqual([2]);
  });

  it("flattens a ping's health sample onto the host row, as the primary does", () => {
    const rows = emptyRows();
    applyFleetEvent(rows, { name: 'host:added', payload: { alias: 'mac', reachable: true, load_1m: 0.1 } });
    applyFleetEvent(rows, {
      name: 'host:pinged',
      payload: { alias: 'mac', last_pinged_at: 9, reachable: true, health: { load_1m: 3.5, mem_avail_kb: 42 } },
    });
    const h = rows.hosts.get('mac')!;
    expect(h.load_1m).toBe(3.5);
    expect(h.mem_avail_kb).toBe(42);
    expect(h.last_pinged_at).toBe(9);
    expect('health' in h).toBe(false);
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

  it("refuses to attach a session on another hub's own machine", () => {
    expect(fleetAttachRefusal({ host_alias: 'local' })).toMatch(/local/);
    expect(fleetAttachRefusal({ host_alias: 'mac' })).toBeNull();
  });

  it("starts the New… form on a host its picker shows", () => {
    const rows = emptyRows();
    rows.hosts.set('secret', { alias: 'secret', hidden: true, reachable: true } as HostRow);
    rows.hosts.set('down', { alias: 'down', hidden: false, reachable: false } as HostRow);
    rows.hosts.set('up', { alias: 'up', hidden: false, reachable: true } as HostRow);
    expect(fleetHostChoices(rows).map((h) => h.alias)).toEqual(['down', 'up']);
    expect(defaultFleetHost(rows)).toBe('up');
    rows.hosts.delete('up');
    expect(defaultFleetHost(rows)).toBe('down');
    rows.hosts.delete('down');
    expect(defaultFleetHost(rows)).toBe('');
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

describe('Phase 2: an extra fleet as a window', () => {
  it('opens an extra fleet session by its own id, and drops it when it is gone', () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1, { tmux_name: 'pos' }) });
    openFleetSession('company', 1);
    expect(get(selectedFleetSession)?.row.tmux_name).toBe('pos');
    expect(get(selectedFleetSession)?.fleet.id).toBe('company');
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 1 } });
    expect(get(selectedFleetSession)).toBeNull();
    openFleetSession(PRIMARY, 1);
    expect(get(fleetSelection)).toEqual({ fleet: 'company', id: 1 });
    closeFleetSession();
    expect(get(fleetSelection)).toBeNull();
  });

  it('reads, keys and snapshots through the fleet, under a pty id of its own', async () => {
    inv.mockResolvedValue('pane');
    await fleetCaptureSession('company', session(4));
    await fleetSendKey('company', session(4), 'Escape');
    await fleetListPullRequests('company', { state: 'open' });
    expect(inv.mock.calls).toEqual([
      ['capture_session', { args: { session_id: 4, scrollback_lines: 200 }, fleet: 'company' }],
      ['send_prompt', { args: { host_alias: 'mac', tmux_name: 's4', prompt: '', submit: false, keys: 'Escape' }, fleet: 'company' }],
      ['list_pull_requests', { args: { state: 'open' }, fleet: 'company' }],
    ]);
    expect(fleetPtyId('company', { id: 4 })).toBe('fleet-company-4');
    expect(fleetPtyId('company', { id: 4 })).toMatch(/^[A-Za-z0-9_.:-]{1,64}$/);
  });

  it('re-reads the fleet list when a link comes up, for its fleet id', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company', { connection: { state: 'connecting' } })]);
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'fleets_list' ? [fleet(PRIMARY), fleet('company', { fleet_id: 'f-1' })] : [],
    );
    onFleetEvent({ fleet: 'company', name: 'hub:connection', payload: { state: 'connected' } });
    await vi.waitFor(() => expect(get(visibleExtraFleets)[0].fleet.fleet_id).toBe('f-1'));
  });

  it("re-lists a fleet after its bridge's gap and when its link comes up", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company', { connection: { state: 'connecting' } })]);
    onFleetEvent({ fleet: 'company', name: 'hub:connection', payload: { state: 'connected' } });
    onFleetEvent({ fleet: 'company', name: 'hub:resynced', payload: null });
    await vi.waitFor(() => expect(inv.mock.calls.filter(([cmd]) => cmd === 'list_sessions')).toHaveLength(2));
    for (const [, a] of inv.mock.calls.filter(([cmd]) => cmd === 'list_sessions')) expect(a).toEqual({ fleet: 'company' });
    expect(get(visibleExtraFleets)[0].fleet.connection).toEqual({ state: 'connected' });
    // A gap on the primary's own bridge is not this view's to re-list.
    onFleetEvent({ fleet: PRIMARY, name: 'hub:resynced', payload: null });
    await new Promise((r) => setTimeout(r, 0));
    expect(inv.mock.calls.filter(([cmd]) => cmd === 'list_sessions')).toHaveLength(2);
  });

  it("re-reads the fleet list when the primary's own link comes up, so a same-hub-twice fleet is caught", async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('again')]);
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'fleets_list' ? [fleet(PRIMARY), fleet('again', { duplicate_of: 'primary' })] : [],
    );
    onPrimaryConnection({ state: 'connecting' });
    expect(inv).not.toHaveBeenCalled();
    onPrimaryConnection({ state: 'connected' });
    await vi.waitFor(() => expect(get(visibleExtraFleets)[0].fleet.duplicate_of).toBe('primary'));
  });

  it("never shows a duplicate fleet's rows", () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company'), fleet('again', { duplicate_of: '"company"' })]);
    onFleetEvent({ fleet: 'again', name: 'session:updated', payload: session(1) });
    const again = get(visibleExtraFleets).find((v) => v.fleet.id === 'again')!;
    expect(again.rows.sessions.size).toBe(0);
  });
});

describe('Phase 2: which fleet New session starts on', () => {
  const project = (id: number, owner: string, repo: string) => ({
    project: { id, owner, repo, base_path: '', last_session_at: null, adopted: false, system: false },
    worktrees: [],
  });

  it('offers every usable extra fleet, by its own project id or null when it lacks the repository', () => {
    const known = [
      fleet(PRIMARY),
      fleet('company'),
      fleet('lab'),
      fleet('down', { unavailable: 'no token' }),
      fleet('twice', { duplicate_of: '"company"' }),
    ];
    const rows = new Map([
      ['company', { ...emptyRows(), projects: [project(9, 'PapayaPOS', 'pos')] }],
      ['lab', { ...emptyRows(), projects: [project(3, 'me', 'other')] }],
      ['down', { ...emptyRows(), projects: [project(1, 'papayapos', 'pos')] }],
      ['twice', { ...emptyRows(), projects: [project(1, 'papayapos', 'pos')] }],
    ]);
    const got = fleetChoicesFor('papayapos', 'POS', known, rows);
    expect(got.map((c) => [c.fleet.id, c.projectId])).toEqual([
      ['company', 9],
      ['lab', null],
    ]);
  });

  it('defaults to the remembered fleet, else the one that owns the owner, else the primary', () => {
    const company = { fleet: fleet('company', { label: 'company', url: 'https://fleet.papayapos.dev' }), projectId: 9 };
    const lab = { fleet: fleet('lab'), projectId: 2 };
    expect(defaultFleetFor('PapayaPOS', [lab, company], null)).toBe('company');
    expect(defaultFleetFor('PapayaPOS', [lab, company], 'lab')).toBe('lab');
    expect(defaultFleetFor('PapayaPOS', [lab, company], PRIMARY)).toBe(PRIMARY);
    expect(defaultFleetFor('PapayaPOS', [lab, company], 'gone')).toBe('company');
    expect(defaultFleetFor('martin-janci', [lab, company], null)).toBe(PRIMARY);
    expect(fleetOwnsOwner({ label: 'Papaya POS', url: null }, 'papayapos')).toBe(true);
  });

  it('remembers per repository, and forgets for the primary', () => {
    rememberFleetFor('PapayaPOS', 'pos', 'company');
    expect(rememberedFleetFor('papayapos', 'POS')).toBe('company');
    rememberFleetFor('PapayaPOS', 'pos', PRIMARY);
    expect(rememberedFleetFor('papayapos', 'pos')).toBeNull();
  });
});

describe('Phase 2: attention and addresses per fleet', () => {
  it('counts what needs you per fleet with that fleet’s own table', () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company'), fleet('lab')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1, { claude_status: 'blocked', kind: 'work' }) });
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(2, { claude_status: 'working', kind: 'work' }) });
    onFleetEvent({ fleet: 'lab', name: 'session:updated', payload: session(1, { claude_status: 'working', kind: 'work' }) });
    expect(get(fleetNeedsCount).get('company')).toBe(1);
    expect(get(fleetNeedsCount).get('lab')).toBe(0);
    expect(get(fleetInbox).map((g) => [g.fleet.id, g.rows.map((r) => r.id)])).toEqual([['company', [1]]]);
    expect(get(fleetInboxCount)).toBe(1);
    fleetFilter.set('lab');
    expect(get(fleetInboxCount)).toBe(0);
  });

  it('names a federation address by the fleet this window knows, and finds its session', () => {
    const known = [fleet(PRIMARY, { fleet_id: 'aaaaaaaa-1111' }), fleet('company', { label: 'company', fleet_id: 'bbbbbbbb-2222' })];
    const rows = new Map([['company', { ...emptyRows(), sessions: new Map([[5, session(5, { host_alias: 'mac', tmux_name: 'dev-x' })]]) }]]);
    expect(fleetAddress('bbbbbbbb-2222/session/mac/dev-x', known).text).toBe('company › mac/dev-x');
    expect(fleetSessionAt('BBBBBBBB-2222/session/mac/dev-x', known, rows)).toEqual({ fleet: 'company', id: 5 });
    expect(fleetSessionAt('aaaaaaaa-1111/session/mac/dev-x', known, rows)).toBeNull();
    expect(fleetAddress('cccccccc-3333/session/mac/x', known)).toEqual({ text: 'cccccccc-3333/session/mac/x', fleet: null, host: 'mac', name: 'x' });
    expect(fleetAddress('session 12 on mefistos', known).fleet).toBeNull();
  });
});

// ── review w01 ──────────────────────────────────────────────────────────

describe('review w01: multi-hub gaps', () => {
  const project = (id: number, owner: string, repo: string) => ({
    project: { id, owner, repo, base_path: '', last_session_at: null, adopted: false, system: false },
    worktrees: [],
  });

  it('w01-3: drops row events for a duplicate fleet, and its rows once the list names it one', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('again', { duplicate_of: 'primary' })]);
    onFleetEvent({ fleet: 'again', name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).has('again')).toBe(false);

    _setFleetsForTest([fleet(PRIMARY), fleet('later')]);
    onFleetEvent({ fleet: 'later', name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).get('later')?.sessions.size).toBe(1);
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'fleets_list' ? [fleet(PRIMARY), fleet('later', { duplicate_of: 'primary' })] : [],
    );
    await loadFleets();
    expect(get(fleetRows).has('later')).toBe(false);
  });

  it('w01-13: a fleet is loaded only once its sessions were listed, not by an event', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(1) });
    expect(get(fleetRows).get('company')?.loaded).toBe(false);
    inv.mockImplementation(async (cmd: string) => (cmd === 'list_sessions' ? [session(1), session(2)] : []));
    await loadFleetRows('company');
    expect(get(fleetRows).get('company')?.loaded).toBe(true);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(3) });
    expect(get(fleetRows).get('company')?.loaded).toBe(true);
  });

  it('w01-5: never offers or defaults to a fleet whose link is down', () => {
    const known = [
      fleet(PRIMARY),
      fleet('company', { url: 'https://fleet.papayapos.dev', connection: { state: 'offline', attempt: 3, retry_in_secs: 30, reason: 'refused' } }),
      fleet('lab', { connection: { state: 'reconnecting', attempt: 1, retry_in_secs: 2, reason: 'x' } }),
      fleet('old', { connection: { state: 'hub_too_old', hub_contract: 1, min_contract: 9 } }),
    ];
    const rows = new Map(known.slice(1).map((f) => [f.id, { ...emptyRows(), projects: [project(9, 'PapayaPOS', 'pos')] }]));
    const got = fleetChoicesFor('papayapos', 'pos', known, rows);
    expect(got.map((c) => c.fleet.id)).toEqual(['lab']);
    expect(defaultFleetFor('PapayaPOS', got, 'company')).toBe(PRIMARY);
    expect(fleetIsDown(known[1])).toBe(true);
    expect(fleetIsDown(known[2])).toBe(false);
  });

  it('w01-6: a duplicate reads "duplicate", not "live"', () => {
    expect(connectionWord({ connection: { state: 'connected' }, unavailable: null, local: false, duplicate_of: 'primary' })).toEqual({
      word: 'duplicate',
      tone: 'warn',
    });
  });

  it("w01-9: refuses to attach a session on another hub's WSL host", () => {
    expect(fleetAttachRefusal({ host_alias: 'wsl-ubuntu' })).toContain('WSL');
    expect(fleetAttachRefusal({ host_alias: 'local' })).not.toBeNull();
    expect(fleetAttachRefusal({ host_alias: 'mac' })).toBeNull();
  });

  it('w01-4: says why a fleet is offline, and that a 401 means pairing again', () => {
    const offline = (reason: string) =>
      fleet('company', { connection: { state: 'offline', attempt: 2, retry_in_secs: 30, reason } });
    expect(fleetTrouble(offline('the hub answered 401 Unauthorized to GET /events'))).toBe(
      "This desktop's pairing was revoked on https://company.example.com. Remove it and add it again with a fresh code.",
    );
    expect(fleetTrouble(offline('connection refused'))).toBe('connection refused');
    expect(fleetTrouble(offline('x'.repeat(500)))!.length).toBe(200);
    expect(fleetTrouble(fleet('company'))).toBeNull();
    expect(fleetTrouble({ ...offline('refused'), duplicate_of: 'primary' })).toBeNull();
  });

  it('w01-12: removing a fleet forgets its open session and its remembered repositories', async () => {
    _setFleetsForTest([fleet(PRIMARY), fleet('company'), fleet('lab')]);
    openFleetSession('company', 12);
    rememberFleetFor('PapayaPOS', 'pos', 'company');
    rememberFleetFor('me', 'exp', 'lab');
    // Review w30-9: the filter and a kill's tombstone are the old fleet's too.
    fleetFilter.set('company');
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(12) });
    onFleetEvent({ fleet: 'company', name: 'session:killed', payload: { id: 12 } });
    inv.mockImplementation(async (cmd: string) => (cmd === 'fleets_list' ? [fleet(PRIMARY), fleet('lab')] : null));
    const r = await removeFleet('company');
    expect(r.ok).toBe(true);
    expect(get(fleetSelection)).toBeNull();
    expect(rememberedFleetFor('PapayaPOS', 'pos')).toBeNull();
    expect(rememberedFleetFor('me', 'exp')).toBe('lab');
    expect(get(fleetFilter)).toBe('all');
    // Added again under the same label: its session #12 is listed, not hidden
    // behind the old fleet's tombstone.
    _setFleetsForTest([fleet(PRIMARY), fleet('company'), fleet('lab')]);
    onFleetEvent({ fleet: 'company', name: 'session:updated', payload: session(12) });
    expect(get(fleetRows).get('company')?.sessions.has(12)).toBe(true);
  });

  it("w01-14: the Add form's colour is the one the backend would pick, from the same palette", () => {
    const rs = readFileSync('src-tauri/src/backend/fleets.rs', 'utf8');
    const m = /pub const PALETTE: &\[&str\] = &\[(.*?)\];/s.exec(rs);
    expect(m).not.toBeNull();
    expect([...FLEET_PALETTE]).toEqual([...m![1].matchAll(/"([^"]+)"/g)].map((x) => x[1]));
    expect(nextFleetColor([fleet(PRIMARY)])).toBe('#e8833a');
    expect(nextFleetColor([fleet(PRIMARY), fleet('company', { color: '#E8833A' })])).toBe('#3fb27f');
  });
});
