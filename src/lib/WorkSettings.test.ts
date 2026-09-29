// Settings → General → Work since declarative pages P4: the introduction and
// a link to the usage counts, the generated `usage.work` data page (its own
// tests are in pages/PageView.test.ts). Trackers moved to the generated Trackers page
// (pages/TrackerPage.test.ts, pages/FlowView.test.ts, TrackerExtras.test.ts,
// and the connect flow's own tests in crates/fleet-core/src/pages/flows.rs).
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import WorkSettings from './WorkSettings.svelte';
import { hubStatus, STANDALONE, type HubStatus } from './hub';

const remote: HubStatus = {
  remote: true,
  url: 'https://fleet.example.com',
  client_name: 'laptop',
  client_mode: null,
  configured_url: 'https://fleet.example.com',
  configured_client_name: 'laptop',
  allow_plaintext: false,
  warning: null,
  restart_required: false,
  unavailable: null,
};

function route() {
  const inv = mockedInvoke as ReturnType<typeof vi.fn>;
  inv.mockReset();
  inv.mockImplementation(async () => null);
  return inv;
}

beforeEach(() => hubStatus.set({ ...STANDALONE }));

describe('Settings → General → Work', () => {
  it('points at the generated pages, and links to the usage page without reading anything itself (M13.2)', async () => {
    const inv = route();
    const onopen = vi.fn();
    render(WorkSettings, { onopen });
    expect(screen.getByTestId('work-section').textContent).toContain('their own pages');
    await fireEvent.click(screen.getByTestId('work-usage-link'));
    expect(onopen).toHaveBeenCalledWith('usage.work');
    expect(inv).not.toHaveBeenCalled();
  });

  it('paired with a hub: no usage link (M13.2: work_admin is the hub master’s)', async () => {
    hubStatus.set(remote);
    route();
    render(WorkSettings, { onopen: vi.fn() });
    await tick();
    expect(screen.queryByTestId('work-usage-link')).toBeNull();
  });
});
