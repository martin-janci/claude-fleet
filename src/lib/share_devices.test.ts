import { describe, it, expect } from 'vitest';
import { readOnlyRecipient } from './share_devices';
import type { DeviceSummary } from './devices';

const dev = (name: string, person: string, mode: string): DeviceSummary => ({
  name, person, mode, trusted: false, created_at: 1, catalogs: [],
});

describe('readOnlyRecipient (step 5.8)', () => {
  it('warns when every device of the person is read-only', () => {
    expect(readOnlyRecipient('ana', [dev('ana-phone', 'ana', 'readonly')])).toContain(
      "ana's only device (ana-phone) is read-only",
    );
    expect(
      readOnlyRecipient(' Ana ', [dev('p', 'ana', 'readonly'), dev('t', 'ana', 'readonly'), dev('x', 'bo', 'full')]),
    ).toContain("All 2 of ana's devices are read-only");
  });

  it('says nothing for a full device, an unknown person or an empty name', () => {
    expect(readOnlyRecipient('ana', [dev('p', 'ana', 'readonly'), dev('m', 'ana', 'full')])).toBeNull();
    expect(readOnlyRecipient('cy', [dev('p', 'ana', 'readonly')])).toBeNull();
    expect(readOnlyRecipient('  ', [dev('p', 'ana', 'readonly')])).toBeNull();
  });
});
