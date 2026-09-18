import { describe, it, expect, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'health_check') return { version: '0.1.0', db_ready: true, schema_version: 1 };
    throw new Error('unexpected command');
  }),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { healthCheck } from './ipc';

describe('ipc.healthCheck', () => {
  it('returns version, db_ready, and schema_version from the backend', async () => {
    const r = await healthCheck();
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.value.version).toBe('0.1.0');
    expect(r.value.db_ready).toBe(true);
    expect(r.value.schema_version).toBe(1);
  });

  // Now routed to the hub's `fleet_health` tool, so it can fail. Before this
  // it could not: a bare `Health` had nowhere to put an error, so remote mode
  // answered from the local database and the footer showed a zeroed fleet.
  it('carries the backend failure rather than throwing or inventing a fleet', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({
      code: 'E_HUB_UNREACHABLE',
      message: 'https://fleet.example.com did not answer: connection refused',
    });
    const r = await healthCheck();
    expect(r.ok).toBe(false);
    if (r.ok) return;
    expect(r.error.code).toBe('E_HUB_UNREACHABLE');
    expect(r.error.message).toContain('connection refused');
  });
});
