import { describe, it, expect } from 'vitest';
import { limitedRecipient, readOnlyRecipient } from './share_devices';
import type { DeviceSummary } from './devices';

const dev = (name: string, person: string, mode: string): DeviceSummary => ({
  name, person, mode, trusted: false, created_at: 1, catalogs: [],
});

describe('readOnlyRecipient (step 5.8)', () => {
  it('warns when every device of the person is read-only', () => {
    expect(readOnlyRecipient('ana', [dev('ana-phone', 'ana', 'readonly')])).toBe(
      'ana can only read until you trust their ana-phone: it is read-only, so they can watch this session but not send it prompts.',
    );
    expect(
      readOnlyRecipient(' Ana ', [dev('p', 'ana', 'readonly'), dev('t', 'ana', 'readonly'), dev('x', 'bo', 'full')]),
    ).toContain('ana can only read until you trust one of their 2 devices');
  });

  it('says nothing for a full device, an unknown person or an empty name', () => {
    expect(readOnlyRecipient('ana', [dev('p', 'ana', 'readonly'), dev('m', 'ana', 'full')])).toBeNull();
    expect(readOnlyRecipient('cy', [dev('p', 'ana', 'readonly')])).toBeNull();
    expect(readOnlyRecipient('  ', [dev('p', 'ana', 'readonly')])).toBeNull();
  });
});

describe('limitedRecipient (G7.11)', () => {
  it('an answer share needs an answer or full device; a watch share needs nothing', () => {
    const ro = [dev('p', 'ana', 'readonly')];
    expect(limitedRecipient('ana', ro, 'answer')?.message).toContain('not answer its questions');
    expect(limitedRecipient('ana', ro, 'answer')?.devices.map((d) => d.name)).toEqual(['p']);
    expect(limitedRecipient('ana', [dev('p', 'ana', 'answer')], 'answer')).toBeNull();
    expect(limitedRecipient('ana', [dev('p', 'ana', 'answer')], 'drive')?.message).toContain('not send it prompts');
    expect(limitedRecipient('ana', ro, 'watch')).toBeNull();
  });
});
