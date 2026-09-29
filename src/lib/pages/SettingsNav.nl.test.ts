// Settings search as a command (declarative pages P5): "set recent work to
// 3 days" shows one confirm row, now → new; Enter or Apply writes it as the
// person through set_fleet_setting, and a setting that needs confirming
// asks first. A paired desktop (no write) keeps the plain search.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import SettingsNav from './SettingsNav.svelte';
import { fleetSettings, SETTING_DEFAULTS } from '../fleet_settings';
import { allDescriptors, bundle, registryRouter } from './testing';
import { homeOf } from './pages';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const values = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));

function show(canWrite = true) {
  const onselect = vi.fn();
  render(SettingsNav, { props: { pages: bundle.pages, descs, values, selected: 'general', canWrite, onselect } });
  return onselect;
}

const sets = () => inv.mock.calls.filter((c) => c[0] === 'set_fleet_setting').map((c) => c[1]);

beforeEach(() => {
  inv.mockReset();
  inv.mockImplementation(registryRouter().impl);
});
afterEach(() => fleetSettings.set({ ...SETTING_DEFAULTS }));

describe('Settings search as a command', () => {
  it('shows now → new, applies on Enter, then opens the setting', async () => {
    const onselect = show();
    const search = screen.getByTestId('settings-search');
    await fireEvent.input(search, { target: { value: 'set recent work to 3 days' } });
    expect(screen.getByTestId('settings-nl-change').textContent).toMatch(
      new RegExp(`Recent work\\s*${values['work.recent_days']} days\\s*→\\s*3 days`),
    );
    expect(screen.queryByTestId('settings-search-hits')).toBeNull();
    await fireEvent.keyDown(search, { key: 'Enter' });
    await waitFor(() => expect(sets()).toEqual([{ key: 'work.recent_days', value: '3' }]));
    await waitFor(() =>
      expect(onselect).toHaveBeenCalledWith(homeOf(bundle.pages, 'work.recent_days')!.page, 'work.recent_days'),
    );
  });

  it('asks first for a setting that needs confirming', async () => {
    show();
    await fireEvent.input(screen.getByTestId('settings-search'), { target: { value: 'turn on auto-tidy' } });
    await fireEvent.click(screen.getByTestId('settings-nl-apply'));
    expect(await screen.findByTestId('confirm-dialog')).toBeInTheDocument();
    expect(sets()).toEqual([]);
    await fireEvent.click(screen.getByTestId('settings-nl-confirm'));
    await waitFor(() => expect(sets()).toEqual([{ key: 'work.auto_tidy', value: 'true' }]));
  });

  it('says what it cannot do, and asks which when the words fit several', async () => {
    show();
    const search = screen.getByTestId('settings-search');
    await fireEvent.input(search, { target: { value: 'set recent work to 900' } });
    expect(screen.getByTestId('settings-nl-error').textContent).toContain('out of range');
    await fireEvent.input(search, { target: { value: 'set tidy to 3' } });
    expect(screen.getByTestId('settings-nl-option-work.tidy_done_days')).toBeInTheDocument();
    expect(sets()).toEqual([]);
  });

  it('a paired desktop keeps the plain search', async () => {
    show(false);
    await fireEvent.input(screen.getByTestId('settings-search'), { target: { value: 'set recent work to 3' } });
    expect(screen.queryByTestId('settings-nl')).toBeNull();
    expect(screen.getByTestId('settings-search-hits')).toBeInTheDocument();
  });
});
