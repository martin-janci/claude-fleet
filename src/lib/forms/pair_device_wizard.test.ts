import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import type { Wizard } from './wizards';
import { NO_ORG, pairDeviceArgs, pairDeviceWizard, runPairDevice } from './pair_device_wizard';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const fieldOf = (w: Wizard, name: string) => w.spec.steps.flatMap((s) => s.fields).find((f) => f.name === name);

beforeEach(() => inv.mockReset());

describe('pair_device wizard', () => {
  it('offers the orgs by id after "No org", and none drops the field', () => {
    expect(fieldOf(pairDeviceWizard([{ id: 3, name: 'Acme' }]), 'org')?.options).toEqual([
      [NO_ORG, 'No org'],
      ['3', 'Acme'],
    ]);
    expect(fieldOf(pairDeviceWizard([]), 'org')).toBeUndefined();
    expect(pairDeviceWizard([]).loader).toBe('halo');
  });

  it('sends pair_device the answers, with no org and no person as null', async () => {
    expect(pairDeviceArgs({ device: ' ada-phone ', mode: 'readonly', org: '3', person: 'ada' })).toEqual({
      device: 'ada-phone',
      mode: 'readonly',
      org_id: 3,
      person: 'ada',
    });
    inv.mockResolvedValue({ url: 'u', code: 'C', expires_in_s: 600, name: 'p', mode: 'full', trusted: false, qr: [] });
    const r = await runPairDevice({ device: 'p', mode: 'full', org: NO_ORG, person: '' });
    expect(r).toEqual({ ok: true, summary: 'p: open u (code C)' });
    expect(inv).toHaveBeenCalledWith('pair_device', { args: { device: 'p', mode: 'full', org_id: null, person: null } });
  });
});

