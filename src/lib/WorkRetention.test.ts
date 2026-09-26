import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WorkRetention from './WorkRetention.svelte';
import SettingsDialog from './SettingsDialog.svelte';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { lastSweepLine, retentionLine, type RetentionStatus } from './work_retention';

const NOW = 1_800_000_000;

const status = (over: Partial<RetentionStatus> = {}): RetentionStatus => ({
  tables: [
    { table: 'work_journal', setting: 'work.retention.journal_days', days: 365, rows: 300, would_delete: 12 },
    { table: 'work_items', setting: 'work.retention.tracker_items_days', days: 180, rows: 50, would_delete: 3 },
    { table: 'session_events', setting: 'work.retention.timeline_work_events_days', days: 0, rows: 9, would_delete: 0 },
  ],
  last_sweep: { at: NOW - 600, journal: 4, tracker_items: 1, timeline_work_events: 2 },
  tick_cap: 2000,
  ...over,
});

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => inv.mockReset());

describe('work_retention lines', () => {
  it('says forever for 0 and the dry-run count otherwise', () => {
    const [j, , e] = status().tables;
    expect(retentionLine(j)).toBe('journal: 300 rows, 12 older than 365 d with nothing live on them');
    expect(retentionLine(e)).toBe('work timeline: 9 rows, kept forever');
    expect(lastSweepLine(null, NOW)).toBe('no sweep yet');
    expect(lastSweepLine(status().last_sweep, NOW)).toBe(
      'last sweep 10 min ago: 4 journal, 1 tickets, 2 events deleted',
    );
  });
});

describe('WorkRetention', () => {
  it('shows counts, the dry run and the last sweep; Sweep now sweeps then re-reads', async () => {
    let swept = false;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_retention_status')
        return swept
          ? status({
              tables: status().tables.map((t) => ({ ...t, would_delete: 0 })),
              last_sweep: { at: NOW, journal: 12, tracker_items: 3, timeline_work_events: 0 },
            })
          : status();
      if (cmd === 'work_retention_sweep') {
        swept = true;
        return { at: NOW, journal: 12, tracker_items: 3, timeline_work_events: 0 };
      }
      return null;
    });
    render(WorkRetention, { props: { now: () => NOW } });
    const journal = await screen.findByTestId('work-retention-work_journal');
    expect(journal.textContent).toContain('12 older than 365 d');
    expect(screen.getByTestId('work-retention-session_events').textContent).toContain('kept forever');
    expect(screen.getByTestId('work-retention-last').textContent).toContain('10 min ago');
    expect(screen.queryByTestId('work-retention-backlog')).toBeNull();

    await fireEvent.click(screen.getByTestId('work-retention-sweep'));
    await waitFor(() =>
      expect(screen.getByTestId('work-retention-last').textContent).toContain('12 journal, 3 tickets'),
    );
    expect(inv).toHaveBeenCalledWith('work_retention_sweep', undefined);
    // Nothing left to delete: the button waits for the next preview.
    expect((screen.getByTestId('work-retention-sweep') as HTMLButtonElement).disabled).toBe(true);
  });

  it('says a backlog drains over sweeps, and shows an error', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'work_retention_status')
        return status({ tables: [{ ...status().tables[0], would_delete: 5000 }] });
      if (cmd === 'work_retention_sweep') throw { code: 'E_LOCAL_ONLY', message: 'on the hub' };
      return null;
    });
    render(WorkRetention, { props: { now: () => NOW } });
    expect((await screen.findByTestId('work-retention-backlog')).textContent).toContain('at most 2000');
    await fireEvent.click(screen.getByTestId('work-retention-sweep'));
    expect((await screen.findByTestId('work-retention-error')).textContent).toContain('on the hub');
  });
});

describe('SettingsDialog — work retention (M12.3)', () => {
  afterEach(() => fleetSettings.set({ ...SETTING_DEFAULTS }));

  it('shows the stored windows and writes a new one', async () => {
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_fleet_settings')
        return {
          'work.retention.journal_days': '30',
          'work.retention.tracker_items_days': '0',
          'work.retention.timeline_work_events_days': '90',
        };
      if (cmd === 'work_retention_status') return status();
      if (cmd === 'mcp_status') return { enabled: false, running: false, port: 4180, token: 't', url: '', bind_error: null, confirm_destructive: false };
      if (cmd === 'list_hosts' || cmd === 'list_host_tokens' || cmd === 'discover_hosts') return [];
      return null;
    });
    render(SettingsDialog, { props: { onClose: () => {} } });
    const input = (id: string) => screen.getByTestId(id) as HTMLInputElement;
    await waitFor(() => expect(input('work-retention-journal-days').value).toBe('30'));
    expect(input('work-retention-tracker-items-days').value).toBe('0');
    expect(input('work-retention-timeline-days').value).toBe('90');
    expect(await screen.findByTestId('work-retention-work_journal')).toBeTruthy();
    expect(screen.queryByTestId('work-journal-days')).toBeNull();

    input('work-retention-tracker-items-days').value = '120';
    await fireEvent.change(input('work-retention-tracker-items-days'));
    await waitFor(() =>
      expect(inv).toHaveBeenCalledWith('set_fleet_setting', {
        key: 'work.retention.tracker_items_days',
        value: '120',
      }),
    );
  });
});
