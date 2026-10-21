import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import AutonomyChip from './AutonomyChip.svelte';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';
import { settingsSection } from './app_views';
import { expectAccessible } from './a11y_check';

// Thread room (2026-10-14, board Autonomy): the chip in Control's composer.

const inv = invoke as unknown as ReturnType<typeof vi.fn>;
let budget: unknown = null;

beforeEach(() => {
  budget = null;
  settingsSection.set(null);
  fleetSettings.set({ ...SETTING_DEFAULTS, 'orchestrator.max_level': '2', 'control.auto_continue': 'true', 'control.auto_continue_minutes': '10', 'control.react_to_events': 'true' });
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a?: { key?: string; value?: string; args?: { action?: string } }) => {
    if (cmd === 'routines' && a?.args?.action === 'budget') return budget;
    if (cmd === 'set_fleet_setting') return { ...get(fleetSettings), [a!.key!]: a!.value! };
    return null;
  });
});

describe('AutonomyChip', () => {
  it('names the position the settings are in, and a pick writes the settings and closes', async () => {
    render(AutonomyChip);
    const chip = screen.getByTestId('autonomy-chip');
    expect(chip.textContent).toContain('Within your brief');
    await fireEvent.click(chip);
    const pop = screen.getByTestId('autonomy-popover');
    expect(screen.getByTestId('autonomy-option-brief').getAttribute('aria-checked')).toBe('true');
    expect(screen.queryByTestId('autonomy-custom')).toBeNull();
    await expectAccessible(pop);
    await fireEvent.click(screen.getByTestId('autonomy-option-autopilot'));
    await waitFor(() => expect(screen.queryByTestId('autonomy-popover')).toBeNull());
    const writes = inv.mock.calls.filter((c) => c[0] === 'set_fleet_setting').map((c) => [c[1].key, c[1].value]);
    expect(writes).toEqual([
      ['orchestrator.max_level', '3'],
      ['control.auto_continue_minutes', '2'],
    ]);
    expect(screen.getByTestId('autonomy-chip').textContent).toContain('Autopilot');
  });

  it('a mix it does not name reads Custom; spend shows when a budget is set; Detailed settings opens Control API', async () => {
    fleetSettings.update((s) => ({ ...s, 'control.auto_continue_minutes': '30' }));
    budget = { spent_micros: 3_100_000, budget_micros: 20_000_000, since: 0 };
    render(AutonomyChip);
    expect(screen.getByTestId('autonomy-chip').textContent).toContain('Custom');
    await fireEvent.click(screen.getByTestId('autonomy-chip'));
    expect(screen.getByTestId('autonomy-custom')).toBeTruthy();
    expect((await screen.findByTestId('autonomy-spend')).textContent).toContain('of');
    await fireEvent.click(screen.getByTestId('autonomy-settings'));
    expect(get(settingsSection)).toBe('control-api');
    expect(screen.queryByTestId('autonomy-popover')).toBeNull();
  });

  it('Esc closes it', async () => {
    render(AutonomyChip);
    await fireEvent.click(screen.getByTestId('autonomy-chip'));
    await fireEvent.keyDown(screen.getByTestId('autonomy-popover'), { key: 'Escape' });
    expect(screen.queryByTestId('autonomy-popover')).toBeNull();
  });

  it('review w19-6/8: arrows move between options, and a pick or Escape gives focus back to the chip', async () => {
    render(AutonomyChip);
    const chip = screen.getByTestId('autonomy-chip');
    await fireEvent.click(chip);
    const brief = screen.getByTestId('autonomy-option-brief');
    expect(screen.getAllByRole('radio').filter((r) => r.getAttribute('tabindex') === '0')).toEqual([brief]);
    brief.focus();
    await fireEvent.keyDown(brief, { key: 'ArrowDown' });
    const auto = screen.getByTestId('autonomy-option-autopilot');
    expect(document.activeElement).toBe(auto);
    expect(inv.mock.calls.some((c) => c[0] === 'set_fleet_setting')).toBe(false);
    await fireEvent.click(auto);
    await waitFor(() => expect(screen.queryByTestId('autonomy-popover')).toBeNull());
    expect(document.activeElement).toBe(chip);

    await fireEvent.click(chip);
    screen.getByTestId('autonomy-option-ask').focus();
    await fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
    expect(screen.queryByTestId('autonomy-popover')).toBeNull();
    expect(document.activeElement).toBe(chip);
  });
});
