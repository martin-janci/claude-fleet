// Gap plan G3.9: "Suggested from your fleet" above Control's composer.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const { focus } = vi.hoisted(() => ({ focus: vi.fn((..._a: unknown[]) => true) }));
vi.mock('./session_focus', () => ({ focusSession: (...a: unknown[]) => focus(...a) }));

import { invoke } from '@tauri-apps/api/core';
import ControlSuggestions from './ControlSuggestions.svelte';
import { dismissedSuggestions, fleetSuggestions, prShort } from './control_suggestions';
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
  dismissedSuggestions.set(new Set());
  sessions.set([]);
  focus.mockClear();
});

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
    expect(invoke).not.toHaveBeenCalled();
  });

  it('a restore card opens its host, where the restore asks first', async () => {
    sessions.set([lost(7), lost(8), lost(9)]);
    render(ControlSuggestions, { sessionId: 1 });
    expect(screen.getByTestId('control-suggestion-open').textContent).toBe('Review on trn');
    await fireEvent.click(screen.getByTestId('control-suggestion-open'));
    expect(get(hostsViewRequest)).toEqual({ host: 'trn' });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('✕ dismisses a card', async () => {
    sessions.set([failing]);
    render(ControlSuggestions, { sessionId: 1 });
    await fireEvent.click(screen.getByTestId('control-suggestion-dismiss'));
    expect(screen.queryByTestId('control-suggestions')).toBeNull();
  });
});
