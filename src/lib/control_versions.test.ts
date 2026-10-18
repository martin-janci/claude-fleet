import { describe, expect, it } from 'vitest';
import { newestPhone, versionsSummary } from './control_versions';

describe('control_versions', () => {
  it('names hub, desktop and phone, and flags a hub older than the desktop', () => {
    const s = versionsSummary({ hub: '0.6.2', desktop: '0.6.3', phone: '0.6.3', remote: true })!;
    expect(s.text).toBe('hub 0.6.2 · desktop 0.6.3 · phone 0.6.3 · hub behind');
    expect(s.hubBehind).toBe(true);
    expect(s.title).toMatch(/older than this desktop/);
    const same = versionsSummary({ hub: '0.6.3', desktop: '0.6.3', phone: null, remote: true })!;
    expect(same.text).toBe('hub 0.6.3 · desktop 0.6.3');
    expect(same.hubBehind).toBe(false);
  });

  it('standalone, the desktop is the fleet and nothing is behind', () => {
    const s = versionsSummary({ hub: '0.6.3', desktop: null, phone: null, remote: false })!;
    expect(s.text).toBe('desktop 0.6.3');
    expect(s.hubBehind).toBe(false);
    expect(versionsSummary({ hub: null, desktop: null, phone: null, remote: false })).toBeNull();
  });

  it('takes the newest phone', () => {
    expect(
      newestPhone([
        { part: 'Phone', version: '0.6.1', reported_at: 1 },
        { part: 'Desktop', version: '9.9.9', reported_at: 1 },
        { part: 'Phone', version: '0.6.10', reported_at: 1 },
      ]),
    ).toBe('0.6.10');
    expect(newestPhone([])).toBeNull();
  });
});
