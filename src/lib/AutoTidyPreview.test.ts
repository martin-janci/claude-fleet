import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AutoTidyPreview from './AutoTidyPreview.svelte';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

afterEach(() => fleetSettings.set({ ...SETTING_DEFAULTS }));

describe('AutoTidyPreview (work graph M7.3)', () => {
  it('lists only the safe kills of the ticked reasons, and says auto-tidy is off', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.auto_tidy_reasons': 'pr_merged_idle' });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'work_tidy')
        return {
          candidates: [
            { session_id: 1, host_alias: 'h', tmux_name: 'done-one', reason: 'done_idle', action: 'safe_kill', since: 0, idle_secs: 18000, key: 'ABC-1' },
            { session_id: 2, host_alias: 'h', tmux_name: 'merged-one', reason: 'pr_merged_idle', action: 'safe_kill', since: 0, idle_secs: 18000, key: 'ABC-2' },
            { session_id: 3, host_alias: 'h', tmux_name: 'dup', reason: 'duplicate_worktree', action: 'kill', since: 0, idle_secs: 90000 },
            { session_id: 4, host_alias: 'h', tmux_name: 'wontdo', reason: 'not_planned', action: 'safe_kill', since: 0, idle_secs: 18000 },
          ],
          auto_tidy: false,
          auto_reasons: ['pr_merged_idle'],
          done_days: 2,
          idle_hours: 4,
        };
      if (cmd === 'work_reopened') return [];
      return null;
    });
    render(AutoTidyPreview);
    await fireEvent.click(screen.getByTestId('work-auto-tidy-dry-run'));
    const preview = await screen.findByTestId('work-auto-tidy-preview');
    const rows = screen.getAllByTestId('work-auto-tidy-preview-row');
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain('merged-one');
    expect(preview.textContent).not.toContain('done-one');
    expect(preview.textContent).toContain('once turned on');
  });
});
