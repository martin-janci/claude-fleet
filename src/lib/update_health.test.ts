// Review w26-3: the fleet's own updates in fleet health raise Attention
// items on the desktop, one per entry, to Settings → Updates.
import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...a: unknown[]) => invoke(...a),
}));

import TrackerAttention from './TrackerAttention.svelte';
import { refreshTrackersHealth } from './tracker_health';
import { updateAttentionItems, updatesHealth } from './update_health';
import { settingsOpen, settingsSection } from './app_views';

describe('updateAttentionItems', () => {
  it('names each problem and the part it is on, once', () => {
    const items = updateAttentionItems({
      channel: 'stale',
      attention: [
        { reason: 'rollback_failed', target: 'hub:self', component: 'hub', version: '0.6.2', detail: 'did not come back' },
        { reason: 'update_rolled_back', target: 'agent:mercury', component: 'agent', version: '0.6.2', detail: 'soak failed' },
        { reason: 'update_rolled_back', target: 'agent:mercury', component: 'agent', version: '0.6.2' },
        { reason: 'channel_stale', target: 'channel:stable' },
        { reason: 'update_required', target: 'client:4', component: 'android', version: '0.6.0' },
        { reason: 'something_new', target: 'client:3', component: 'desktop' },
      ],
    });
    expect(items.map((i) => i.label)).toEqual([
      'Hub update: rollback failed — hub stopped',
      'Update rolled back — agent on mercury',
      'Release channel stale',
      'Update required — phone 4',
      'Update: something new — device 3',
    ]);
    expect(items[1].detail).toBe('agent:mercury runs 0.6.2 · soak failed · Open Settings → Updates');
    expect(items.every((i) => i.section === 'updates')).toBe(true);
    expect(updateAttentionItems(null)).toEqual([]);
    expect(updateAttentionItems({ channel: 'fresh', attention: [] })).toEqual([]);
  });

  it('reads them from health_check and opens Settings → Updates', async () => {
    invoke.mockResolvedValue({
      version: '0.6.3',
      db_ready: true,
      schema_version: 1,
      updates: { channel: 'fresh', attention: [{ reason: 'update_failed', target: 'hub:self', component: 'hub', version: '0.6.3' }] },
    });
    await refreshTrackersHealth();
    expect(get(updatesHealth)?.attention?.length).toBe(1);
    render(TrackerAttention);
    await tick();
    const item = screen.getByTestId('update-attention-item');
    expect(item.textContent).toContain('Update failed — the hub');
    await fireEvent.click(item);
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toBe('updates');
  });
});
