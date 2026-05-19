import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.1.0', db_ready: true };
    throw new Error('unexpected command');
  }),
}));

import { healthCheck } from './ipc';

describe('ipc.healthCheck', () => {
  it('returns version and db_ready from the backend', async () => {
    const h = await healthCheck();
    expect(h.version).toBe('0.1.0');
    expect(h.db_ready).toBe(true);
  });
});
