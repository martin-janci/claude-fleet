import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import Attention from './Attention.svelte';
import { sessions, sessionsLoaded, type SessionRow } from './sessions';
import { toasts, clearToasts, runToastAction } from './toasts';
import { clearSelection, selectedSession } from './selection';
import { notifyStuckOs, notifyStuckToast } from './notify';
import { fleetSettings } from './fleet_settings';
import { failing, type FailingRoutine } from './routines';
import { PRIMARY, fleetSelection, _resetFleetsForTest, _setFleetRowsForTest, _setFleetsForTest, emptyRows, type FleetInfo } from './fleets';
import { fleetInfo } from './fleets_fixture';
import { hosts, type HostRow } from './hosts';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: 'dev-x',
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  };
}

beforeEach(() => {
  sessions.set([]);
  sessionsLoaded.set(true);
  clearToasts();
  notifyStuckToast.set(true);
  notifyStuckOs.set(false);
});

describe('Attention', () => {
  it('announces a session that transitions into a stuck state, once', async () => {
    const a = row({ tmux_name: 'dev-a', host_alias: 'mefistos' });
    sessions.set([a]);
    render(Attention);
    await tick();
    const region = screen.getByTestId('stuck-announcer');
    expect(region).toHaveAttribute('aria-live', 'assertive');
    expect(region).toHaveTextContent('');

    sessions.set([{ ...a, stuck_kind: 'press_enter' }]);
    await tick();
    expect(region).toHaveTextContent('dev-a on mefistos is stuck: press Enter');
    expect(get(toasts).map((t) => t.message)).toEqual(['dev-a on mefistos is stuck: press Enter']);

    // Same stuck state on the next update: no second announcement/toast.
    sessions.set([{ ...a, stuck_kind: 'press_enter', last_activity_at: 2 }]);
    await tick();
    expect(get(toasts)).toHaveLength(1);
  });

  it('w11-12: the stuck toast opens the session', async () => {
    const a = row({ tmux_name: 'dev-open', host_alias: 'mefistos' });
    sessions.set([a]);
    clearSelection();
    render(Attention);
    await tick();
    sessions.set([{ ...a, stuck_kind: 'press_enter' }]);
    await tick();
    const t = get(toasts)[0];
    expect(t.action?.label).toBe('Open');
    runToastAction(t.id);
    expect(get(selectedSession)?.id).toBe(a.id);
  });

  it('does not replay stuck rows delivered by the bootstrap fill', async () => {
    // App.svelte mounts the sidebar (and this watcher) before the first
    // list_sessions resolves: the store goes [] -> [rows]. That fill is
    // baseline, not a burst of transitions.
    sessionsLoaded.set(false);
    render(Attention);
    await tick();
    const a = row({ tmux_name: 'dev-boot', stuck_kind: 'oom' });
    const b = row({ tmux_name: 'dev-boot2', stuck_kind: 'trust_prompt' });
    sessions.set([a, b]);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('');
    expect(get(toasts)).toHaveLength(0);
    // Bootstrap completes; the same rows re-delivered are still baseline.
    sessionsLoaded.set(true);
    sessions.set([a, b]);
    await tick();
    expect(get(toasts)).toHaveLength(0);
    // A genuinely new transition after load is announced.
    sessions.set([a, b, row({ tmux_name: 'dev-new', stuck_kind: 'reconnect' })]);
    await tick();
    expect(get(toasts).map((t) => t.message)).toEqual(['dev-new on local is stuck: reconnecting']);
  });

  it('does not announce rows that were already stuck at mount', async () => {
    sessions.set([row({ stuck_kind: 'oom' })]);
    render(Attention);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('');
    expect(get(toasts)).toHaveLength(0);
  });

  it('respects the toast toggle but still fills the live region', async () => {
    notifyStuckToast.set(false);
    const a = row({ tmux_name: 'dev-b' });
    sessions.set([a]);
    render(Attention);
    await tick();
    sessions.set([{ ...a, stuck_kind: 'auth_menu' }]);
    await tick();
    expect(screen.getByTestId('stuck-announcer')).toHaveTextContent('dev-b on local is stuck: auth menu');
    expect(get(toasts)).toHaveLength(0);
  });
});

const shown: string[] = [];
const made: FakeNotification[] = [];
class FakeNotification {
  static permission = 'granted';
  static requestPermission = async () => 'granted';
  onclick: ((e: Event) => void) | null = null;
  closed = false;
  constructor(
    public title: string,
    public opts?: { tag?: string },
  ) {
    shown.push(title);
    made.push(this);
  }
  close() {
    this.closed = true;
  }
}

describe('Attention: the notifications matrix (11.9)', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    failing.set([]);
  });
  beforeEach(() => {
    shown.length = 0;
    vi.stubGlobal('Notification', FakeNotification);
    vi.spyOn(document, 'hasFocus').mockReturnValue(false);
    notifyStuckOs.set(true);
    fleetSettings.set({ ...get(fleetSettings), 'notify.desktop': 'needs_you,failed', 'notify.sound': '', 'notify.quiet_hours': '' });
  });

  it('announces a session that comes to need you, by the Desktop column', async () => {
    const a = row({ tmux_name: 'dev-q', claude_status: 'working' });
    sessions.set([a]);
    render(Attention);
    await tick();
    sessions.set([{ ...a, claude_status: 'blocked' }]);
    await tick();
    expect(shown).toEqual(['dev-q needs you']);
  });

  it('w11-4: clicking the notification opens the session', async () => {
    const a = row({ tmux_name: 'dev-click', claude_status: 'working' });
    sessions.set([a]);
    clearSelection();
    made.length = 0;
    render(Attention);
    await tick();
    sessions.set([{ ...a, claude_status: 'blocked' }]);
    await tick();
    expect(made).toHaveLength(1);
    made[0].onclick?.(new Event('click'));
    expect(get(selectedSession)?.id).toBe(a.id);
  });

  it('stays quiet for a state the column leaves out, and while the window has focus', async () => {
    const a = row({ tmux_name: 'dev-r', claude_status: 'working' });
    const b = row({ tmux_name: 'dev-s', claude_status: 'working' });
    sessions.set([a, b]);
    render(Attention);
    await tick();
    fleetSettings.set({ ...get(fleetSettings), 'notify.desktop': 'failed' });
    sessions.set([{ ...a, claude_status: 'blocked' }, b]);
    await tick();
    expect(shown).toEqual([]);
    vi.mocked(document.hasFocus).mockReturnValue(true);
    sessions.set([{ ...a, claude_status: 'blocked' }, { ...b, claude_status: 'failed' }]);
    await tick();
    expect(shown).toEqual([]);
  });

  it('w11-12: what quiet hours held back is summed up once when they end, without what was answered', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] });
    try {
      vi.setSystemTime(new Date(2026, 10, 24, 23, 10));
      fleetSettings.set({ ...get(fleetSettings), 'notify.quiet_hours': '22:00-07:00', 'notify.quiet_except': '' });
      const a = row({ tmux_name: 'dev-n1', claude_status: 'working' });
      const b = row({ tmux_name: 'dev-n2', claude_status: 'working' });
      const c = row({ tmux_name: 'dev-n3', claude_status: 'working' });
      sessions.set([a, b, c]);
      render(Attention);
      await tick();
      sessions.set([a, b, c].map((r) => ({ ...r, claude_status: 'blocked' })));
      await tick();
      // Answered from the phone during the night: not in the summary.
      sessions.set([{ ...a, claude_status: 'blocked' }, { ...b, claude_status: 'blocked' }, c]);
      await tick();
      vi.advanceTimersByTime(30_000);
      expect(shown).toEqual([]);
      vi.setSystemTime(new Date(2026, 10, 25, 7, 0, 5));
      vi.advanceTimersByTime(30_000);
      expect(shown).toEqual(['2 sessions need you']);
      vi.advanceTimersByTime(60_000);
      expect(shown).toEqual(['2 sessions need you']);
    } finally {
      vi.useRealTimers();
    }
  });

  describe('review w11: a cause is announced once, and old news is not repeated', () => {
    const hostRow = (alias: string, reachable: boolean) => ({ alias, reachable, last_pinged_at: 1, hidden: false }) as unknown as HostRow;
    beforeEach(() => {
      made.length = 0;
      fleetSettings.set({ ...get(fleetSettings), 'notify.desktop': 'needs_you,failed,blocked,done' });
    });
    afterEach(() => hosts.set([]));

    it('w11-2: a host going down is one notification for all its sessions', async () => {
      hosts.set([hostRow('trn', true)]);
      const rows = [1, 2, 3].map((i) => row({ tmux_name: `dev-${i}`, host_alias: 'trn', claude_status: 'working' }));
      sessions.set(rows);
      render(Attention);
      await tick();
      sessions.set([...rows]);
      await tick();
      hosts.set([hostRow('trn', false)]);
      sessions.set([...rows]);
      await tick();
      expect(shown).toEqual(['trn is down · 3 sessions']);
      expect(made[0].opts?.tag).toBe('blocked-host-trn');
      // w11-10: taken down when the host answers again.
      hosts.set([hostRow('trn', true)]);
      sessions.set([...rows]);
      await tick();
      expect(made[0].closed).toBe(true);
      expect(shown).toHaveLength(1);
    });

    it('r5-06-3: a row joining a cause already announced does not ring again', async () => {
      hosts.set([hostRow('trn', true)]);
      const a = row({ tmux_name: 'dev-1', host_alias: 'trn', claude_status: 'working' });
      const b = row({ tmux_name: 'dev-2', host_alias: 'trn', claude_status: 'working' });
      sessions.set([a]);
      render(Attention);
      await tick();
      sessions.set([{ ...a }]);
      await tick();
      hosts.set([hostRow('trn', false)]);
      sessions.set([{ ...a }]);
      await tick();
      sessions.set([{ ...a }, b]);
      await tick();
      expect(shown).toEqual(['trn is down · 1 session']);
    });

    it('r5-06-2: a signed-out login is announced with no usage reading at all', async () => {
      const up = hostRow('trn', true);
      hosts.set([up]);
      const a = row({ tmux_name: 'dev-so', host_alias: 'trn', claude_status: 'idle', last_viewed_at: 9_999_999_999 } as Partial<SessionRow>);
      sessions.set([a]);
      render(Attention);
      await tick();
      sessions.set([{ ...a }]);
      await tick();
      hosts.set([{ ...up, signed_out_logins: [{ profile: null }] } as unknown as HostRow]);
      sessions.set([{ ...a }]);
      await tick();
      expect(shown).toEqual(['A login is signed out · 1 session']);
    });

    it('w11-2: rows already blocked when the hosts first answer are the baseline', async () => {
      const a = row({ tmux_name: 'dev-a', host_alias: 'trn', claude_status: 'working' });
      sessions.set([a]);
      render(Attention);
      await tick();
      hosts.set([hostRow('trn', false)]);
      sessions.set([{ ...a }]);
      await tick();
      expect(shown).toEqual([]);
    });

    it('w11-3: a block lifting does not announce the Failed underneath again', async () => {
      hosts.set([hostRow('trn', true)]);
      const a = row({ tmux_name: 'dev-f', host_alias: 'trn', claude_status: 'failed', kind: 'bg' });
      sessions.set([a]);
      render(Attention);
      await tick();
      sessions.set([{ ...a }]);
      await tick();
      hosts.set([hostRow('trn', false)]);
      sessions.set([{ ...a }]);
      await tick();
      hosts.set([hostRow('trn', true)]);
      sessions.set([{ ...a }]);
      await tick();
      expect(shown).toEqual(['trn is down · 1 session']);
    });

    it('w11-10: a notification is taken down once its session no longer waits', async () => {
      const a = row({ tmux_name: 'dev-w', claude_status: 'working' });
      sessions.set([a]);
      render(Attention);
      await tick();
      sessions.set([{ ...a, claude_status: 'blocked' }]);
      await tick();
      expect(made).toHaveLength(1);
      expect(made[0].closed).toBe(false);
      sessions.set([{ ...a, claude_status: 'working' }]);
      await tick();
      expect(made[0].closed).toBe(true);
    });

    it("w11-1: a teammate's row on an extra fleet does not notify", async () => {
      const base = fleetInfo(PRIMARY, { label: 'home' });
      _setFleetsForTest([base, { ...base, id: 'company', label: 'company', primary: false, local: false, connection: { state: 'connected' } }]);
      const mine = row({ tmux_name: 'mine', claude_status: 'working', owner_person_id: 3 });
      const theirs = row({ tmux_name: 'theirs', claude_status: 'working', owner_person_id: 7 });
      const rowsOf = (list: SessionRow[]) => ({ ...emptyRows(), loaded: true, me: 3, sessions: new Map(list.map((r) => [r.id, r])) });
      try {
        render(Attention);
        _setFleetRowsForTest('company', rowsOf([mine, theirs]));
        await tick();
        _setFleetRowsForTest('company', rowsOf([{ ...mine, claude_status: 'blocked' }, { ...theirs, claude_status: 'blocked' }]));
        await tick();
        expect(shown).toEqual(['mine needs you']);
      } finally {
        _resetFleetsForTest();
      }
    });
  });

  describe('extra fleets (review w01)', () => {
    const base = fleetInfo(PRIMARY, { label: 'home' });
    const extra = (id: string, over: Partial<FleetInfo> = {}): FleetInfo => ({
      ...base,
      id,
      label: id,
      url: `https://${id}.example.com`,
      primary: false,
      local: false,
      connection: { state: 'connected' },
      ...over,
    });
    const rowsOf = (list: SessionRow[], loaded = true) => ({ ...emptyRows(), loaded, sessions: new Map(list.map((r) => [r.id, r])) });
    afterEach(() => _resetFleetsForTest());

    it('w01-3: a duplicate fleet never notifies, the fleet it duplicates does', async () => {
      _setFleetsForTest([base, extra('company'), extra('again', { duplicate_of: 'company' })]);
      const a = row({ tmux_name: 'pos', claude_status: 'working' });
      const b = row({ tmux_name: 'pos-dup', claude_status: 'working' });
      render(Attention);
      _setFleetRowsForTest('company', rowsOf([a]));
      _setFleetRowsForTest('again', rowsOf([b]));
      await tick();
      _setFleetRowsForTest('company', rowsOf([{ ...a, claude_status: 'blocked' }]));
      _setFleetRowsForTest('again', rowsOf([{ ...b, claude_status: 'blocked' }]));
      await tick();
      expect(shown).toEqual(['pos needs you']);
    });

    it("w17-6: clicking a fleet's notification opens that fleet's session", async () => {
      _setFleetsForTest([base, extra('company')]);
      const a = row({ tmux_name: 'far', claude_status: 'working' });
      made.length = 0;
      render(Attention);
      _setFleetRowsForTest('company', rowsOf([a]));
      await tick();
      _setFleetRowsForTest('company', rowsOf([{ ...a, claude_status: 'blocked' }]));
      await tick();
      expect(made).toHaveLength(1);
      made[0].onclick?.(new Event('click'));
      expect(get(fleetSelection)).toEqual({ fleet: 'company', id: a.id });
    });

    it("w01-13: an event before a fleet's first list is no baseline", async () => {
      _setFleetsForTest([base, extra('company')]);
      const a = row({ tmux_name: 'early', claude_status: 'working' });
      const b = row({ tmux_name: 'waiting-1', claude_status: 'blocked' });
      const c = row({ tmux_name: 'waiting-2', claude_status: 'blocked' });
      render(Attention);
      _setFleetRowsForTest('company', rowsOf([a], false));
      await tick();
      _setFleetRowsForTest('company', rowsOf([a, b, c]));
      await tick();
      expect(shown).toEqual([]);
      _setFleetRowsForTest('company', rowsOf([{ ...a, claude_status: 'blocked' }, b, c]));
      await tick();
      expect(shown).toEqual(['early needs you']);
    });
  });

  it('announces a routine run that fails after the app opened', async () => {
    fleetSettings.set({ ...get(fleetSettings), 'notify.desktop': 'routine_failed' });
    render(Attention);
    await tick();
    const now = Math.floor(Date.now() / 1000);
    const routine = { id: 3, name: 'nightly' } as FailingRoutine['routine'];
    failing.set([{ routine, run: { id: 9, started_at: now - 99_999, finished_at: now - 99_000 } as FailingRoutine['run'], may_change: true }]);
    await tick();
    expect(shown).toEqual([]);
    failing.set([{ routine, run: { id: 10, started_at: now, finished_at: now + 1 } as FailingRoutine['run'], may_change: true }]);
    await tick();
    expect(shown).toEqual(['Routine nightly failed']);
    // Review w11-7: a teammate's routine is not in this person's Inbox.
    failing.set([{ routine, run: { id: 11, started_at: now, finished_at: now + 2 } as FailingRoutine['run'], may_change: false }]);
    await tick();
    expect(shown).toEqual(['Routine nightly failed']);
  });
});
