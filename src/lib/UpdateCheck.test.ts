// Review w26-9: a person's "Check for updates" brings back a banner they
// dismissed, so the update it finds can be installed without a restart.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() =>
    Promise.resolve({
      status: 'update_available',
      mode: 'notify',
      source: 'hub',
      installed: '0.5.4',
      reason: '0.5.5 is recommended.',
      version: '0.5.5',
      mandatory: false,
      deadline: null,
      install: 'in_place',
      download_url: null,
      notes_url: null,
      next_check_secs: 21600,
    }),
  ),
}));

import UpdateCheck from './UpdateCheck.svelte';
import { dismissUpdate, dismissedUpdate } from './updates';

describe('UpdateCheck', () => {
  it('clears an earlier dismissal when the check finds the update', async () => {
    dismissUpdate('0.5.5');
    render(UpdateCheck);
    await fireEvent.click(screen.getByTestId('update-check-btn'));
    await waitFor(() => expect(screen.getByTestId('update-check-line').textContent).toBe('claude-fleet 0.5.5 is available.'));
    expect(get(dismissedUpdate)).toBeNull();
  });
});
