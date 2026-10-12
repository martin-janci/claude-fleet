// The Jev envelope's health on the desktop (test map §7): one Attention item
// while `fleet_health.decide` says degraded, linking to Settings →
// Decisions (Jev); nothing otherwise.
import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...a: unknown[]) => invoke(...a),
}));

import TrackerAttention from './TrackerAttention.svelte';
import { decideAttentionItem, decideHealth } from './decide_health';
import { refreshTrackersHealth, trackersHealth } from './tracker_health';
import { settingsOpen, settingsSection } from './app_views';

describe('decideAttentionItem', () => {
  it('says nothing unless degraded', () => {
    expect(decideAttentionItem(null)).toBeNull();
    expect(decideAttentionItem({ enabled: true, degraded: false, attempts: 40, failures: 2 })).toBeNull();
  });

  it('names the failure rate and the breaker', () => {
    const it1 = decideAttentionItem({
      enabled: true,
      degraded: true,
      reason: 'failure_rate',
      attempts: 12,
      failures: 3,
      failure_rate: 0.25,
      window_secs: 3600,
    })!;
    expect(it1.label).toBe('Jev degraded');
    expect(it1.section).toBe('decide');
    expect(it1.detail).toContain('3 of 12 calls failed in the last 60 min (25%)');
    expect(it1.detail).toContain("fall back to today's rules");
    const it2 = decideAttentionItem({ degraded: true, reason: 'breaker_open', breaker_open: true })!;
    expect(it2.detail).toContain('circuit breaker is open');
  });
});

describe('TrackerAttention with the Jev item', () => {
  beforeEach(() => {
    invoke.mockReset();
    trackersHealth.set(null);
    decideHealth.set(null);
    settingsOpen.set(false);
    settingsSection.set(null);
  });

  it('shows the item from a health read and opens Settings → Decisions (Jev)', async () => {
    invoke.mockResolvedValueOnce({
      version: '1',
      db_ready: true,
      schema_version: 76,
      decide: { enabled: true, degraded: true, reason: 'breaker_open', breaker_open: true },
    });
    render(TrackerAttention);
    await refreshTrackersHealth();
    await tick();
    const item = screen.getByTestId('decide-attention-item');
    expect(item.textContent?.trim()).toBe('Jev degraded →');
    await fireEvent.click(item);
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toBe('decide');
    // Healthy again (or an older hub that sends no block): gone.
    invoke.mockResolvedValueOnce({ version: '1', db_ready: true, schema_version: 76 });
    await refreshTrackersHealth();
    await tick();
    expect(screen.queryByTestId('decide-attention-item')).toBeNull();
    expect(screen.queryByTestId('tracker-attention')).toBeNull();
  });
});
