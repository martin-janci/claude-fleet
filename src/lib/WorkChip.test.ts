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
  // A tracker-backed key (native item status, fix round 3): `trackerBacked`
  // is what a real hub sets when `status_category` (tracker-only on the
  // wire) is present — the signal `unbound`/`stale`/the sync tooltip key
  // off, not `status` alone (a local item has one of those too now).
  trackerBacked: true,
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
    expect(chip.title).toContain('connect its tracker in Settings → Work to see ZED-9');
  });

  it('a plain key without a tracker item is just a key', () => {
    render(WorkChip, {
      props: { workKey: { key: 'ABC-2', source: 'branch', from: 'abc-2' }, now: () => T },
    });
    const chip = screen.getByTestId('work-chip');
    expect(chip.className).not.toContain('unbound');
    expect(screen.queryByTestId('work-chip-dot')).toBeNull();
  });

  it('a local item with a ticket-shaped key still offers "connect its tracker" (fix round 3)', () => {
    // A local item's own live status shows a `status` too now, but with no
    // connected tracker for its (coincidentally ticket-shaped) key: `status`
    // alone must not read as "a tracker owns it" and hide the hint.
    render(WorkChip, {
      props: {
        workKey: {
          key: 'ZED-9',
          source: 'link',
          from: 'Local work',
          status: { category: 'in_progress', name: null, url: null, unavailable: false },
        },
        now: () => T,
      },
    });
    const chip = screen.getByTestId('work-chip');
    expect(chip.className).toContain('unbound');
    // The dot still shows the local item's own live status.
    expect(screen.getByTestId('work-chip-dot').className).toContain('dot-progress');
    // …and the hint no longer contradicts it (final review, item 6): it says
    // whose status the dot is, and what connecting a tracker would ADD —
    // never "connect a tracker to see its status" beside a status dot.
    expect(chip.title).toContain(
      "the dot is fleet's own status; connect its tracker in Settings → Work to see ZED-9's too",
    );
    expect(chip.title).not.toContain("to see ZED-9's status");
  });
});

describe('WorkChip, more than one tracker kind (work graph M6)', () => {
  it('shows the provider badge and an Asana key short', () => {
    trackers.update((l) => [
      ...l,
      {
        id: 2,
        provider: 'asana',
        name: 'Company B',
        site_url: 'https://app.asana.com',
        state: 'ok',
        created_at: 1,
        last_sync_at: T - 60,
        config: {},
      },
    ]);
    render(WorkChip, {
      props: {
        workKey: { ...linked(), key: 'asana:1207000000000001' },
        now: () => T,
      },
    });
    const chip = screen.getByTestId('work-chip');
    expect(screen.getByTestId('work-chip-provider').textContent).toBe('A');
    expect(chip.textContent).toContain('Asana …000001');
    expect(chip.title.startsWith('Asana · ')).toBe(true);
  });
});
