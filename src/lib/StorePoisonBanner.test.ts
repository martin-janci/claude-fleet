// Review w19-1: a poisoned store blocks the window with a Restart button.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

const handlers = new Map<string, () => void>();
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, cb: () => void) => {
    handlers.set(name, cb);
    return () => handlers.delete(name);
  }),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }));

import { invoke } from '@tauri-apps/api/core';
import StorePoisonBanner from './StorePoisonBanner.svelte';

beforeEach(() => {
  handlers.clear();
  vi.mocked(invoke).mockClear();
});

describe('StorePoisonBanner', () => {
  it('shows nothing until the backend says the store is poisoned, then offers Restart', async () => {
    render(StorePoisonBanner);
    await waitFor(() => expect(handlers.has('app:store-poisoned')).toBe(true));
    expect(screen.queryByTestId('store-poison-banner')).toBeNull();
    handlers.get('app:store-poisoned')!();
    const banner = await screen.findByTestId('store-poison-banner');
    expect(banner.textContent).toContain('must restart');
    await fireEvent.click(screen.getByTestId('store-poison-restart'));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('restart_app', undefined);
  });
});
