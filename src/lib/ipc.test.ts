import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.1.0', db_ready: true, schema_version: 1 };
    throw new Error('unexpected command');
  }),
}));

import { healthCheck } from './ipc';

describe('ipc.healthCheck', () => {
  it('returns version, db_ready, and schema_version from the backend', async () => {
    const h = await healthCheck();
    expect(h.version).toBe('0.1.0');
    expect(h.db_ready).toBe(true);
    expect(h.schema_version).toBe(1);
  });
});
