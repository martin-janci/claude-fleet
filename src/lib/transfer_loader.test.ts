import { describe, expect, it } from 'vitest';
import { transferLoader } from './transfer_loader';

describe('transferLoader (step 10.10)', () => {
  it('a known size always picks Progress ring', () => {
    for (const f of [0, 0.01, 0.5, 0.999, 1]) {
      expect(transferLoader(f)).toEqual({ name: 'progress-ring', value: f });
    }
    expect(transferLoader(1.4)).toEqual({ name: 'progress-ring', value: 1 });
    expect(transferLoader(-0.2)).toEqual({ name: 'progress-ring', value: 0 });
  });

  it('an unknown size picks Data rain', () => {
    expect(transferLoader(null)).toEqual({ name: 'data-rain' });
    expect(transferLoader(Number.NaN)).toEqual({ name: 'data-rain' });
  });
});
