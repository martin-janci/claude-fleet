// Settings → Hub & sync → Fleets (review w01): what a row says about a fleet
// that is not usable, and the colour the Add form starts on.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FleetsSettings from './FleetsSettings.svelte';
import { PRIMARY, _resetFleetsForTest, type FleetInfo } from './fleets';
import { fleetInfo } from './fleets_fixture';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const primary = fleetInfo(PRIMARY, { label: 'home' });
const extra = (id: string, over: Partial<FleetInfo> = {}): FleetInfo => ({
  ...primary,
  id,
  label: id,
  url: `https://${id}.example.com`,
  primary: false,
  local: false,
  color: '#e8833a',
  connection: { state: 'connected' },
  ...over,
});

beforeEach(() => {
  inv.mockReset();
  _resetFleetsForTest();
});

async function show(list: FleetInfo[]) {
  inv.mockImplementation(async (cmd: string) => (cmd === 'fleets_list' ? list : []));
  render(FleetsSettings);
  for (let i = 0; i < 6; i++) await tick();
}

describe('FleetsSettings (review w01)', () => {
  it('w17-13: a failed list read says so with Retry', async () => {
    let fail = true;
    inv.mockImplementation(async (cmd: string) => {
      if (cmd !== 'fleets_list') return [];
      if (fail) throw { code: 'E_DB', message: 'database is locked' };
      return [primary];
    });
    render(FleetsSettings);
    const box = await screen.findByTestId('fleets-load-error');
    expect(box.textContent).toContain("Couldn't load the fleets");
    fail = false;
    await fireEvent.click(screen.getByTestId('fleets-load-error-retry'));
    await waitFor(() => expect(screen.queryByTestId('fleets-load-error')).toBeNull());
    expect(screen.getByTestId(`fleets-row-${PRIMARY}`)).toBeTruthy();
  });

  it('w01-6: a duplicate reads "duplicate" and says to remove it', async () => {
    await show([primary, extra('again', { duplicate_of: 'home' })]);
    expect(screen.getByTestId('fleets-row-again').textContent).toContain('duplicate');
    expect(screen.getByTestId('fleets-duplicate-again').textContent).toBe('Same hub as home; remove this one.');
  });

  it('w01-4: a revoked pairing says to remove and add it again', async () => {
    await show([
      primary,
      extra('company', { connection: { state: 'offline', attempt: 4, retry_in_secs: 60, reason: 'the hub answered 401 to GET /events' } }),
    ]);
    expect(screen.getByTestId('fleets-trouble-company').textContent).toContain('pairing was revoked on https://company.example.com');
  });

  it("w01-14: the colour starts on the one the backend would give, not black", async () => {
    await show([primary, extra('company', { color: '#e8833a' })]);
    expect((screen.getByTestId('fleets-add-color') as HTMLInputElement).value).toBe('#3fb27f');
  });
});
