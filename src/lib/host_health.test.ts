import { describe, it, expect } from 'vitest';
import { checkedAgo, chipHost, fleetLoads, hostLoad, loadParts, otherTrouble } from './host_health';
import { host, NOW } from './hosts_fixture';

// Thread room (2026-10-14): the status bar's host health chip.

const GB = 1_000_000;
const mac = host('mac', { health_at: NOW - 20, cpu_pct: 18.4, mem_total_kb: 16 * GB, mem_avail_kb: 7.2 * GB, disk_home_total_kb: 100 * GB, disk_home_free_kb: 60 * GB });
const mercury = host('mercury', { health_at: NOW - 20, cpu_pct: 72, mem_total_kb: 32 * GB, mem_avail_kb: 1.9 * GB, mem_low: true, disk_home_total_kb: 100 * GB, disk_home_free_kb: 39 * GB });
const htz = host('htz', { health_at: NOW - 20, cpu_pct: 6, mem_total_kb: 64 * GB, mem_avail_kb: 44 * GB, disk_home_total_kb: 100 * GB, disk_home_free_kb: 17 * GB });

describe('host health', () => {
  it("reads a host's CPU, memory and disk as whole percents", () => {
    const l = hostLoad(mac, NOW);
    expect([l.cpu, l.mem, l.disk]).toEqual([18, 55, 40]);
    expect(loadParts(l)).toEqual(['CPU 18%', 'Mem 55%', 'Disk 40%']);
    expect(l.trouble).toBeNull();
  });

  it('judges trouble from the fresh sample only: low memory, a saturated CPU, a full disk, offline', () => {
    expect(hostLoad(mercury, NOW).trouble).toBe('mem 94%');
    expect(hostLoad(htz, NOW).trouble).toBeNull();
    expect(hostLoad({ ...htz, disk_home_free_kb: 5 * GB }, NOW).trouble).toBe('disk 95%');
    expect(hostLoad({ ...mac, cpu_saturated: true, cpu_pct: 97 }, NOW).trouble).toBe('cpu 97%');
    expect(hostLoad({ ...mac, reachable: false }, NOW).trouble).toBe('offline');
    expect(loadParts(hostLoad({ ...mac, reachable: false }, NOW))).toEqual(['offline']);
    // A stale sample still shows its numbers, but judges nothing.
    const stale = hostLoad({ ...mercury, health_at: NOW - 7200 }, NOW);
    expect(stale.mem).toBe(94);
    expect(stale.trouble).toBeNull();
  });

  it('a host that answered nothing says so', () => {
    expect(loadParts(hostLoad(host('new', { health_at: null, cpu_pct: null, mem_total_kb: null, disk_home_total_kb: null }), NOW))).toEqual(['not sampled yet']);
  });

  it('lists the focused host first, then the ones in trouble, and names the worst other one', () => {
    const loads = fleetLoads([htz, mercury, mac], 'mac', NOW);
    expect(loads.map((l) => l.alias)).toEqual(['mac', 'mercury', 'htz']);
    expect(otherTrouble(loads, 'mac')?.alias).toBe('mercury');
    expect(otherTrouble(loads, 'mercury')).toBeNull();
  });

  it("follows the selected session's host; in Control, the operator's unless the panel shows a session", () => {
    expect(chipHost({ inControl: false, sessionShown: false, selectedHost: 'htz', operatorHost: 'mac' })).toBe('htz');
    expect(chipHost({ inControl: false, sessionShown: false, selectedHost: null, operatorHost: 'mac' })).toBe('mac');
    expect(chipHost({ inControl: true, sessionShown: false, selectedHost: 'htz', operatorHost: 'mac' })).toBe('mac');
    expect(chipHost({ inControl: true, sessionShown: true, selectedHost: 'htz', operatorHost: 'mac' })).toBe('htz');
  });

  it('says when it was checked', () => {
    expect(checkedAgo(NOW - 20, NOW)).toBe('checked 20s ago');
    expect(checkedAgo(NOW - 300, NOW)).toBe('checked 5m ago');
    expect(checkedAgo(null, NOW)).toBe('not sampled yet');
  });
});
