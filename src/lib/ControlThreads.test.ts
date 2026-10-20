import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import ControlThreads from './ControlThreads.svelte';
import { sessions } from './sessions';
import { resetHandoffsForTests } from './handoffs';
import { operatorSession } from './operator';
import { controlViews, defaultLayout } from './control_views';
import { activeThread, controlMission } from './control_room';
import { selectedSession } from './selection';
import { destination } from './destination';
import { sidebarView } from './work_view';
import { session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';
import { mutedThreads, threadFollows } from './control_journal';

// Thread room (2026-10-14, board Main): Control's own left column.

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
const now = Math.floor(Date.now() / 1000);
const operator = session('mac', 'fleet-operator', { last_prompt: 'Ship 0.6.2 when CI is green' });
const flake = session('mercury', 'fix-flake', { origin: 'operator', claude_status: 'working', last_activity_at: now - 60 });
const release = session('mac', 'release', { origin: 'operator', claude_status: 'blocked', last_activity_at: now - 120 });
const mine = session('mac', 'mine', { origin: 'person', claude_status: 'working' });

let receipts: unknown[] = [];
let missions: unknown[] = [];
let budget: unknown = null;

beforeEach(() => {
  resetHandoffsForTests();
  mutedThreads.set(new Set());
  threadFollows.set(null);
  controlViews.set(defaultLayout());
  activeThread.set(null);
  controlMission.set(null);
  receipts = [{ id: 1, at: now - 30, kind: 'mission', tool: 'work_link', mission_id: 5, mission_name: 'Hub federation v2', mission_state: 'active' }];
  missions = [];
  budget = null;
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, args?: { args?: { action?: string } }) => {
    if (cmd === 'control_handoffs') return receipts;
    if (cmd === 'work_missions') return missions;
    if (cmd === 'routines' && args?.args?.action === 'budget') return budget;
    return null;
  });
  sessions.set([operator, flake, release, mine]);
  operatorSession.set(operator);
});
afterEach(() => {
  sessions.set([]);
  operatorSession.set(null);
  destination.set('session');
});

describe('ControlThreads', () => {
  it('lists Main chat, then the threads Control handed out by group, never the rest of the fleet', async () => {
    render(ControlThreads, { oncollapse: () => {} });
    expect(screen.getByTestId('control-thread-main').textContent).toContain('Ship 0.6.2');
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(3));
    const names = screen.getAllByTestId('control-thread').map((r) => r.textContent ?? '');
    expect(names.join(' ')).not.toContain('mine');
    expect(screen.getByTestId('control-thread-group-needs').textContent).toContain('release');
    expect(screen.getByTestId('control-thread-group-working').textContent).toContain('fix-flake');
    expect(screen.getByTestId('control-thread-group-working').textContent).toContain('Hub federation v2');
  });

  it('the chips narrow to Needs you and to Missions, with their counts', async () => {
    render(ControlThreads, { oncollapse: () => {} });
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(3));
    expect(screen.getByTestId('control-threads-filter-all').textContent).toContain('3');
    await fireEvent.click(screen.getByTestId('control-threads-filter-needs'));
    expect(screen.getAllByTestId('control-thread').map((r) => r.dataset.kind)).toEqual(['session']);
    await fireEvent.click(screen.getByTestId('control-threads-filter-missions'));
    expect(screen.getAllByTestId('control-thread').map((r) => r.dataset.kind)).toEqual(['mission']);
    expect(screen.getByTestId('control-threads-filter-missions').getAttribute('aria-pressed')).toBe('true');
  });

  it('a session thread opens in the Session tab and a mission in Missions; Control stays', async () => {
    destination.set('control');
    render(ControlThreads, { oncollapse: () => {} });
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(3));
    const rows = screen.getAllByTestId('control-thread');
    await fireEvent.click(rows.find((r) => r.textContent?.includes('release'))!);
    expect(get(selectedSession)?.id).toBe(release.id);
    expect(get(controlViews).active).toBe('session');
    expect(rows.find((r) => r.textContent?.includes('release'))!.getAttribute('aria-current')).toBe('true');
    await fireEvent.click(rows.find((r) => r.dataset.kind === 'mission')!);
    expect(get(controlMission)).toBe(5);
    expect(get(controlViews).active).toBe('missions');
    await fireEvent.click(screen.getByTestId('control-thread-main'));
    expect(get(activeThread)).toBeNull();
    expect(get(destination)).toBe('control');
  });

  it('says what will show up before anything was handed out', async () => {
    receipts = [];
    sessions.set([operator, mine]);
    render(ControlThreads, { oncollapse: () => {} });
    expect((await screen.findByTestId('control-threads-empty')).textContent).toContain('No threads yet');
  });

  it("shows today's spend against the daily budget when one is set", async () => {
    budget = { spent_micros: 3_100_000, budget_micros: 20_000_000, since: 0 };
    render(ControlThreads, { oncollapse: () => {} });
    expect((await screen.findByTestId('control-threads-spend')).textContent).toContain('$3.10 of $20.00');
  });

  it('All sessions goes back to the sessions list; « folds the column', async () => {
    const oncollapse = vi.fn();
    destination.set('control');
    render(ControlThreads, { oncollapse });
    await fireEvent.click(screen.getByTestId('control-threads-collapse'));
    expect(oncollapse).toHaveBeenCalled();
    await fireEvent.click(screen.getByTestId('control-threads-all-sessions'));
    expect(get(sidebarView)).toBe('sessions');
    expect(get(destination)).toBe('session');
  });

  it("a thread's bell mutes it for Control, and the journal says which are muted", async () => {
    let muted = false;
    inv.mockImplementation(async (cmd: string, args?: { args?: { action?: string; thread?: string; id?: number } }) => {
      if (cmd === 'control_handoffs') return receipts;
      if (cmd === 'work_missions') return missions;
      if (cmd === 'control_journal') {
        const a = args?.args ?? {};
        if (a.action === 'mute') muted = true;
        if (a.action === 'unmute') muted = false;
        return a.action === 'read' ? { follows: [{ kind: 'session', id: flake.id, state: muted ? 'muted' : 'following', name: 'fix-flake', muted }] } : { changed: true };
      }
      return null;
    });
    render(ControlThreads, { oncollapse: () => {} });
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(3));
    const row = () => screen.getAllByTestId('control-thread').find((r) => (r.textContent ?? '').includes('fix-flake'))!;
    const bell = () => row().parentElement!.querySelector('[data-testid="control-thread-mute"]') as HTMLElement;
    expect(bell().getAttribute('aria-pressed')).toBe('false');
    await fireEvent.click(bell());
    await waitFor(() => expect(row().dataset.muted).toBe('true'));
    expect(row().textContent).toContain('Muted ·');
    // A toggle keeps its name; only the pressed state says it is muted.
    expect(bell().getAttribute('aria-pressed')).toBe('true');
    expect(bell().getAttribute('aria-label')).toBe('Mute fix-flake');
    expect(inv).toHaveBeenCalledWith('control_journal', { args: { action: 'mute', thread: 'session', id: flake.id } });
    await fireEvent.click(bell());
    await waitFor(() => expect(row().dataset.muted).toBeUndefined());
  });

  it("lists a thread the hub follows past the newest receipts, and says when Control stopped following one (w04-1, w04-10)", async () => {
    const old = session('htz', 'old-brief', { origin: 'person', claude_status: 'idle', last_activity_at: now - 90_000 });
    sessions.set([operator, flake, release, mine, old]);
    missions = [{ id: 8, name: 'Docs sweep', goal: '', mode: 'finite', state: 'paused', level: 2, plan_version: 1, created_at: 1, updated_at: now - 60, version: 1 }];
    inv.mockImplementation(async (cmd: string, args?: { args?: { action?: string } }) => {
      if (cmd === 'control_handoffs') return receipts;
      if (cmd === 'work_missions') return missions;
      if (cmd === 'control_journal' && args?.args?.action === 'read')
        return {
          follows: [
            { kind: 'session', id: old.id, state: 'quiet', name: 'old-brief', muted: false },
            { kind: 'mission', id: 8, state: 'following', name: 'Docs sweep', muted: false },
          ],
        };
      return null;
    });
    render(ControlThreads, { oncollapse: () => {} });
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(5));
    const line = (name: string) =>
      screen
        .getAllByTestId('control-thread')
        .find((r) => (r.textContent ?? '').includes(name))!
        .querySelector('[data-testid="control-thread-line"]')!.textContent;
    expect(line('old-brief')).toContain('not followed (went quiet)');
    expect(line('fix-flake')).not.toContain('not followed');
    // A paused mission says so, under Draft and paused (w04-9).
    expect(line('Docs sweep')).toBe('Paused');
    expect(screen.getByTestId('control-thread-group-paused').textContent).toContain('Docs sweep');
  });

  it('passes the axe and audit checks', async () => {
    const { container } = render(ControlThreads, { oncollapse: () => {} });
    await waitFor(() => expect(screen.getAllByTestId('control-thread')).toHaveLength(3));
    await expectAccessible(container);
  });
});
