// Work graph M12.4: trackers in the fleet's health — the "Reconnect …"
// Attention item and the footer's tracker line.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { tick } from 'svelte';

import TrackerAttention from './TrackerAttention.svelte';
import { trackers, trackerAttention, describeSyncMetrics, RECONNECT_STATES, type TrackerRow } from './trackers';
import { settingsFocus, settingsOpen } from './app_views';
import { trackersHealthLine } from './ipc';

function tracker(over: Partial<TrackerRow>): TrackerRow {
  return {
    id: 1,
    provider: 'jira',
    name: 'acme',
    site_url: 'https://acme.atlassian.net',
    state: 'ok',
    created_at: 1,
    ...over,
  };
}

describe('trackerAttention', () => {
  it('raises one item per tracker a person has to reconnect', () => {
    const items = trackerAttention([
      tracker({ id: 1, state: 'ok' }),
      tracker({ id: 2, name: 'acme', state: 'auth_failed', last_error: 'the tracker refused the credential' }),
      tracker({ id: 3, provider: 'jira_dc', name: 'corp', state: 'captcha' }),
      tracker({ id: 4, provider: 'github', name: 'gh', state: 'unreachable' }),
      tracker({ id: 5, provider: 'linear', name: 'lin', state: 'rate_limited' }),
      tracker({ id: 6, provider: 'asana', name: 'as', state: 'unconfigured' }),
      tracker({ id: 2, name: 'acme', state: 'auth_failed' }), // a duplicate row
    ]);
    expect(items.map((i) => i.label)).toEqual(['Reconnect Jira (acme)', 'Reconnect Jira (corp)']);
    expect(items[0].detail).toBe('the tracker refused the credential');
    expect(items[1].detail).toBe('log in via the browser');
  });

  it('matches the states the hub calls failing without a pass count', () => {
    expect([...RECONNECT_STATES].sort()).toEqual(['auth_failed', 'captcha']);
  });

  it('names every provider by its short name', () => {
    const label = (provider: string) => trackerAttention([tracker({ provider, state: 'auth_failed' })])[0].label;
    expect(label('github')).toBe('Reconnect GitHub (acme)');
    expect(label('asana')).toBe('Reconnect Asana (acme)');
    expect(label('linear')).toBe('Reconnect Linear (acme)');
    expect(label('a_newer_provider')).toBe('Reconnect a_newer_provider (acme)');
  });
});

describe('TrackerAttention', () => {
  beforeEach(() => {
    trackers.set([]);
    settingsOpen.set(false);
    settingsFocus.set(null);
  });

  it('shows nothing while every tracker is fine', () => {
    trackers.set([tracker({ state: 'ok' }), tracker({ id: 2, state: 'unreachable' })]);
    render(TrackerAttention);
    expect(screen.queryByTestId('tracker-attention')).toBeNull();
  });

  it('opens Settings → Work, and goes away once the tracker syncs again', async () => {
    trackers.set([tracker({ state: 'auth_failed', last_error: 'token expired' })]);
    render(TrackerAttention);
    const item = screen.getByTestId('tracker-attention-item');
    expect(item.textContent).toContain('Reconnect Jira (acme)');
    expect(item.getAttribute('title')).toBe('token expired');

    await fireEvent.click(item);
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsFocus)).toBe('work');

    // A `work:tracker` frame after the credential is set again.
    trackers.set([tracker({ state: 'ok' })]);
    await tick();
    expect(screen.queryByTestId('tracker-attention-item')).toBeNull();
  });
});

describe('trackersHealthLine', () => {
  it('says nothing for a healthy fleet or an older hub', () => {
    expect(trackersHealthLine(undefined)).toBeNull();
    expect(trackersHealthLine({})).toBeNull();
    expect(trackersHealthLine({ failing: 0, degraded: 0, detection_backlog: 0, trackers: [] })).toBeNull();
  });

  it('counts failing and degraded trackers and the waiting suggestions', () => {
    expect(trackersHealthLine({ failing: 1, degraded: 2, detection_backlog: 4, backlog_days: 7 })).toBe(
      'trackers: 1 failing, 2 degraded · 4 suggestions waiting > 7 d',
    );
    expect(trackersHealthLine({ degraded: 1 })).toBe('trackers: 1 degraded');
    expect(trackersHealthLine({ detection_backlog: 1 })).toBe('1 suggestion waiting > 7 d');
  });
});

describe('describeSyncMetrics', () => {
  it('says how many passes in a row failed, and nothing once one is ok', () => {
    const m = { tracker_id: 1, last_pass_at: 1, duration_ms: 40, items_listed: 0, items_changed: 0, frames_emitted: 1 };
    expect(describeSyncMetrics({ ...m, consecutive_failures: 3 })).toBe(
      'last pass 40 ms · 0 listed · 0 changed · 1 frames · 3 failed in a row',
    );
    expect(describeSyncMetrics({ ...m, consecutive_failures: 0 })).toBe('last pass 40 ms · 0 listed · 0 changed · 1 frames');
  });
});
