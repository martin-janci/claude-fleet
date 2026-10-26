// The hub's own disk and backups on the desktop (hardening survey
// 2026-10-26): one Attention item per flag `fleet_health.storage` raises,
// linking to Settings → Hub & sync; nothing otherwise.
import { fireEvent, render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...a: unknown[]) => invoke(...a),
}));

import TrackerAttention from './TrackerAttention.svelte';
import { storageAttentionItems, storageHealth } from './storage_health';
import { refreshTrackersHealth, trackersHealth } from './tracker_health';
import { settingsOpen, settingsSection } from './app_views';

const GB = 1024 ** 3;
const NOW = 1_800_000_000;

describe('storageAttentionItems', () => {
  it('says nothing unless the hub raised a flag', () => {
    expect(storageAttentionItems(null, NOW)).toEqual([]);
    expect(storageAttentionItems({ free_bytes: 50 * GB, total_bytes: 100 * GB, last_backup_at: NOW - 60 }, NOW)).toEqual([]);
  });

  it('names the free space and the backup age', () => {
    const [disk, backup] = storageAttentionItems(
      { free_bytes: 1.5 * GB, total_bytes: 100 * GB, low_disk: true, last_backup_at: NOW - 3 * 86400, backup_stale: true },
      NOW,
    );
    expect(disk.label).toBe('Hub disk almost full');
    expect(disk.detail).toContain('1.5 GB free of 100.0 GB');
    expect(disk.section).toBe('hub');
    expect(backup.label).toBe('Hub backup stale');
    expect(backup.detail).toContain('the newest backup is 3 days old');
    expect(storageAttentionItems({ backup_stale: true }, NOW)[0].detail).toContain('the hub has no backup');
  });
});

describe('TrackerAttention with the storage items', () => {
  beforeEach(() => {
    invoke.mockReset();
    trackersHealth.set(null);
    storageHealth.set(null);
    settingsOpen.set(false);
    settingsSection.set(null);
  });

  it('shows an item from a health read and opens Settings → Hub & sync', async () => {
    invoke.mockResolvedValueOnce({
      version: '1',
      db_ready: true,
      schema_version: 76,
      storage: { free_bytes: GB, total_bytes: 100 * GB, low_disk: true },
    });
    render(TrackerAttention);
    await refreshTrackersHealth();
    await tick();
    const item = screen.getByTestId('storage-attention-item');
    expect(item.textContent?.trim()).toBe('Hub disk almost full →');
    await fireEvent.click(item);
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toBe('hub');
    // Room again (or an older hub that sends no block): gone.
    invoke.mockResolvedValueOnce({ version: '1', db_ready: true, schema_version: 76 });
    await refreshTrackersHealth();
    await tick();
    expect(screen.queryByTestId('storage-attention-item')).toBeNull();
  });
});
