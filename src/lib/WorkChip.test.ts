import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach } from 'vitest';
import WorkChip from './WorkChip.svelte';
import { trackers } from './trackers';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import type { WorkKey } from './work_keys';

const T = 10_000;

beforeEach(() => {
  fleetSettings.set({ ...SETTING_DEFAULTS });
  trackers.set([
    {
      id: 1,
      provider: 'jira',
      name: 'acme',
      site_url: 'https://acme.atlassian.net',
      state: 'ok',
      created_at: 1,
      last_sync_at: T - 60,
      config: { key_prefixes: ['ABC'] },
    },
  ]);
});

const linked = (over: Partial<NonNullable<WorkKey['status']>> = {}): WorkKey => ({
  key: 'ABC-1',
  source: 'link',
  from: 'Login page',
  status: { category: 'in_progress', name: 'In Review', url: null, unavailable: false, ...over },
});

describe('WorkChip', () => {
  it('shows the status dot and names status and sync in the tooltip', () => {
    render(WorkChip, { props: { workKey: linked(), now: () => T } });
    const chip = screen.getByTestId('work-chip');
    expect(chip.textContent?.trim()).toBe('ABC-1');
    expect(screen.getByTestId('work-chip-dot').className).toContain('dot-progress');
    expect(chip.title).toContain('In Review');
    expect(chip.title).toContain('synced 1 min ago');
    expect(screen.queryByTestId('work-chip-stale')).toBeNull();
  });

  it('strikes an unavailable item through, without a dot', () => {
    render(WorkChip, { props: { workKey: linked({ unavailable: true }), now: () => T } });
    expect(screen.getByTestId('work-chip').className).toContain('unavailable');
    expect(screen.queryByTestId('work-chip-dot')).toBeNull();
  });

  it('shows a clock when the tracker has not synced for twice the interval', () => {
    render(WorkChip, { props: { workKey: linked(), now: () => T - 60 + 601 } });
    expect(screen.getByTestId('work-chip-stale')).toBeInTheDocument();
  });

  it('a ticket-shaped key no tracker owns says where to connect one', () => {
    render(WorkChip, {
      props: { workKey: { key: 'ZED-9', source: 'branch', from: 'zed-9-x' }, now: () => T },
    });
    const chip = screen.getByTestId('work-chip');
    expect(chip.className).toContain('unbound');
    expect(chip.title).toContain('connect Jira in Settings → Work to see ZED-9');
  });

  it('a plain key without a tracker item is just a key', () => {
    render(WorkChip, {
      props: { workKey: { key: 'ABC-2', source: 'branch', from: 'abc-2' }, now: () => T },
    });
    const chip = screen.getByTestId('work-chip');
    expect(chip.className).not.toContain('unbound');
    expect(screen.queryByTestId('work-chip-dot')).toBeNull();
  });
});
