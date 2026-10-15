import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import HostHealthChip from './HostHealthChip.svelte';
import ControlHostsView from './ControlHostsView.svelte';
import { hosts } from './hosts';
import { sessions } from './sessions';
import { selectSession } from './selection';
import { destination } from './destination';
import { controlViews, defaultLayout, selectView } from './control_views';
import { operatorHost } from './operator';
import { hostsViewRequest } from './app_views';
import { host, session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';

// Thread room (2026-10-14): the status bar's host health chip and its popover.

const GB = 1_000_000;
const now = () => Math.floor(Date.now() / 1000);
function load(alias: string, cpu: number, memUsed: number, over: Partial<Parameters<typeof host>[1]> = {}) {
  return host(alias, {
    health_at: now() - 20,
    cpu_pct: cpu,
    mem_total_kb: 100 * GB,
    mem_avail_kb: (100 - memUsed) * GB,
    disk_home_total_kb: 100 * GB,
    disk_home_free_kb: 60 * GB,
    ...over,
  });
}

beforeEach(() => {
  hosts.set([load('mac', 18, 55), load('mercury', 72, 94, { mem_low: true }), load('htz', 6, 31)]);
  operatorHost.set('mac');
  controlViews.set(defaultLayout());
  destination.set('session');
  selectSession(null);
});
afterEach(() => {
  sessions.set([]);
  hosts.set([]);
  hostsViewRequest.set(null);
});

describe('HostHealthChip', () => {
  it("shows the selected session's host and flags another host in trouble", () => {
    const s = session('htz', 'on-htz');
    sessions.set([s]);
    selectSession(s);
    render(HostHealthChip);
    const chip = screen.getByTestId('host-health-chip');
    expect(chip.textContent).toContain('htz');
    expect(chip.textContent).toContain('CPU 6%');
    expect(chip.textContent).toContain('Mem 31%');
    expect(chip.textContent).toContain('Disk 40%');
    expect(screen.getByTestId('host-health-other').textContent).toBe('· mercury mem 94%');
  });

  it("in Control with no session on the right, it shows the operator's host", async () => {
    const s = session('htz', 'on-htz');
    sessions.set([s]);
    selectSession(s);
    destination.set('control');
    render(HostHealthChip);
    expect(screen.getByTestId('host-health-chip').textContent).toContain('mac');
    selectView('session');
    await Promise.resolve();
    await new Promise((r) => setTimeout(r));
    expect(screen.getByTestId('host-health-chip').textContent).toContain('htz');
  });

  it('a click lists every host; a name opens it in Hosts; Esc closes', async () => {
    render(HostHealthChip);
    await fireEvent.click(screen.getByTestId('host-health-chip'));
    const pop = screen.getByTestId('host-health-popover');
    expect(screen.getByTestId('host-health-focus').textContent).toContain('mac');
    expect(screen.getAllByTestId('host-health-row').map((r) => r.textContent?.split(/\s/)[0])).toEqual(['mercury', 'htz']);
    expect(screen.getAllByTestId('host-health-row')[0].dataset.trouble).toBe('true');
    await expectAccessible(pop);
    await fireEvent.keyDown(pop, { key: 'Escape' });
    expect(screen.queryByTestId('host-health-popover')).toBeNull();
    await fireEvent.click(screen.getByTestId('host-health-chip'));
    await fireEvent.click(screen.getAllByTestId('host-health-open')[1]);
    expect(get(hostsViewRequest)).toEqual({ host: 'mercury' });
    expect(screen.queryByTestId('host-health-popover')).toBeNull();
  });

  it("Control's Hosts view is the same list", () => {
    render(ControlHostsView);
    expect(screen.getByTestId('host-health-focus').textContent).toContain('mac');
    expect(screen.getAllByTestId('host-health-row')).toHaveLength(2);
  });
});
