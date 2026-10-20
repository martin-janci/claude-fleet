// Gap plan G3.9: "Suggested from your fleet" above Control's composer.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { focus } = vi.hoisted(() => ({ focus: vi.fn((..._a: unknown[]) => true) }));
vi.mock('./control_room', () => ({ openInControl: (...a: unknown[]) => focus(...a) }));

import { invoke } from '@tauri-apps/api/core';
import ControlSuggestions from './ControlSuggestions.svelte';
import { DISMISSED_PREF, fleetSuggestions, prShort, resetSuggestionsForTests } from './control_suggestions';
import { escalationScope } from './control_escalations';
import { EMPTY_REPORT, tidyReport, tidyRequest, type TidyCandidate } from './tidy';
import { readPref } from './prefs';
import { sessions, type SessionRow } from './sessions';
import { composerInsert } from './conversation';
import { hostsViewRequest } from './app_views';

const row = (over: Partial<SessionRow>): SessionRow =>
  ({
    id: 1,
    tmux_name: 's',
    host_alias: 'mac',
    friendly_name: null,
    lost_at: null,
    kind: 'work',
    claude_session_id: 'c',
    pr_url: null,
    ci_status: null,
    ...over,
  }) as SessionRow;

const failing = row({
  id: 4,
  tmux_name: 'hub-e2e',
  pr_url: 'https://github.com/acme/fleet/pull/478',
  ci_status: 'failing',
  pr_evidence: { draft: false, checks: { total: 3, pending: 0, skipped: 0, failing_total: 1, failing: [{ name: 'hub-headless' }] } },
});
const lost = (id: number) => row({ id, tmux_name: `t${id}`, host_alias: 'trn', lost_at: 100 });

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  resetSuggestionsForTests();
  escalationScope.set('fleet');
  tidyReport.set(EMPTY_REPORT);
  tidyRequest.set(null);
  sessions.set([]);
  focus.mockClear();
});

/** What the card made the backend DO: anything but the tidy report's reads. */
const acted = () => vi.mocked(invoke).mock.calls.map((c) => c[0]).filter((c) => c !== 'work_tidy' && c !== 'work_reopened');

const tidyCand = (id: number, reason: string): TidyCandidate => ({ session_id: id, host_alias: 'mac', tmux_name: `t${id}`, reason, action: 'safe_kill', since: 1 });

describe('fleetSuggestions', () => {
  it('suggests finishing CI on a live failing PR, then restoring a mass loss', () => {
    const out = fleetSuggestions([failing, lost(7), lost(8), lost(9), row({ id: 2, ci_status: 'passing', pr_url: 'u' })]);
    expect(out.map((s) => s.title)).toEqual(['Finish CI on PR #478', 'Restore 3 stopped sessions on trn']);
    expect(out[0].why).toBe('hub-headless failing · hub-e2e on mac');
  });
  it('says nothing about a lost session, a fold under three, or a dismissed card', () => {
    expect(fleetSuggestions([{ ...failing, lost_at: 5 }, lost(7), lost(8)])).toEqual([]);
    const id = fleetSuggestions([failing])[0].id;
    expect(fleetSuggestions([failing], new Set([id]))).toEqual([]);
  });
  it("with Control's scope, a session card is only for Control's own sessions (w04-14)", () => {
    expect(fleetSuggestions([failing], new Set(), { scope: new Set([99]) })).toEqual([]);
    expect(fleetSuggestions([failing], new Set(), { scope: new Set([4]) }).map((s) => s.kind)).toEqual(['ci']);
  });
  it('finished sessions in the tidy report are one /tidy card (w04-14)', () => {
    const out = fleetSuggestions([], new Set(), { tidy: [tidyCand(3, 'done_idle'), tidyCand(5, 'pr_merged_idle'), tidyCand(6, 'idle_unlinked')] });
    expect(out.map((s) => [s.kind, s.title, s.ask])).toEqual([['tidy', 'Tidy 2 finished sessions', '/tidy']]);
  });
  it('prShort reads the number off a PR URL', () => {
    expect(prShort('https://github.com/a/b/pull/12')).toBe('#12');
    expect(prShort('https://x/y')).toBe('https://x/y');
  });
});

describe('ControlSuggestions', () => {
  it('shows nothing when the fleet suggests nothing', () => {
    render(ControlSuggestions, { sessionId: 1 });
    expect(screen.queryByTestId('control-suggestions')).toBeNull();
  });

  it('a CI card opens the session, or puts the ask in Control’s box without sending', async () => {
    sessions.set([failing]);
    render(ControlSuggestions, { sessionId: 1 });
    expect(screen.getByRole('heading').textContent).toBe('Suggested from your fleet');
    await fireEvent.click(screen.getByTestId('control-suggestion-open'));
    expect(focus).toHaveBeenCalledWith(4, 'hub-e2e');
    await fireEvent.click(screen.getByTestId('control-suggestion-ask'));
    expect(get(composerInsert)?.sessionId).toBe(1);
    expect(get(composerInsert)?.draft).toContain('Finish CI on PR #478');
    expect(acted()).toEqual([]);
  });

  it('a restore card opens its host, where the restore asks first', async () => {
    sessions.set([lost(7), lost(8), lost(9)]);
    render(ControlSuggestions, { sessionId: 1 });
    expect(screen.getByTestId('control-suggestion-open').textContent).toBe('Review on trn');
    await fireEvent.click(screen.getByTestId('control-suggestion-open'));
    expect(get(hostsViewRequest)).toEqual({ host: 'trn' });
    expect(acted()).toEqual([]);
  });

  it('✕ dismisses a card, and the dismissal is kept across restarts (w04-14)', async () => {
    sessions.set([failing]);
    render(ControlSuggestions, { sessionId: 1 });
    await fireEvent.click(screen.getByTestId('control-suggestion-dismiss'));
    expect(screen.queryByTestId('control-suggestions')).toBeNull();
    const kept = readPref<[string, number][]>(DISMISSED_PREF, [], (v): v is [string, number][] => Array.isArray(v));
    expect(kept.map(([k]) => k)).toEqual([fleetSuggestions([failing])[0].id]);
  });

  it('a tidy card opens the tidy sheet on those sessions', async () => {
    tidyReport.set({ ...EMPTY_REPORT, candidates: [tidyCand(3, 'done_idle')] });
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_tidy' ? { ...EMPTY_REPORT, candidates: [tidyCand(3, 'done_idle')] } : []));
    render(ControlSuggestions, { sessionId: 1 });
    await fireEvent.click(await screen.findByTestId('control-suggestion-open'));
    expect(get(tidyRequest)?.sessionIds).toEqual([3]);
  });
});
