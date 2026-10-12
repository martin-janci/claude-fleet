// Redesign step 3.13: `sessionsAnswered` ends the "fleet arriving" wait on
// the first answer of `list_sessions`, a failure included, so no loader can
// outlive a failed bootstrap.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { loadSessions, sessionsAnswered, sessionsLoaded } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessionsAnswered.set(false);
  sessionsLoaded.set(false);
});

describe('sessionsAnswered', () => {
  it('turns on with the first list', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]);
    expect(get(sessionsAnswered)).toBe(false);
    await loadSessions();
    expect(get(sessionsAnswered)).toBe(true);
  });

  it('turns on when the first list fails, which sessionsLoaded does not', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: 'E_INTERNAL', message: 'boom' });
    const r = await loadSessions();
    expect(r.ok).toBe(false);
    expect(get(sessionsAnswered)).toBe(true);
    expect(get(sessionsLoaded)).toBe(false);
  });
});
