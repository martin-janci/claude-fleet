import { describe, expect, it } from 'vitest';
import { bannerFor, nextCheckMs, type DesktopUpdate } from './updates';

function u(over: Partial<DesktopUpdate> = {}): DesktopUpdate {
  return {
    status: 'update_available',
    mode: 'notify',
    source: 'hub',
    installed: '0.5.4',
    reason: '0.5.5 is recommended.',
    version: '0.5.5',
    mandatory: false,
    deadline: null,
    install: 'in_place',
    download_url: 'https://github.com/x/claude-fleet_0.5.5_aarch64.app.tar.gz',
    notes_url: null,
    next_check_secs: 21600,
    ...over,
  };
}

describe('bannerFor', () => {
  it('offers an in-place update as a restart, dismissible', () => {
    expect(bannerFor(u(), null)).toEqual({
      tone: 'info',
      text: 'claude-fleet 0.5.5 is available — restart to update.',
      action: 'restart',
      dismissible: true,
    });
  });

  it('stays quiet for a version the person dismissed, and says nothing when up to date', () => {
    expect(bannerFor(u(), '0.5.5')).toBeNull();
    expect(bannerFor(u({ version: '0.5.6' }), '0.5.5')).not.toBeNull();
    expect(bannerFor(u({ status: 'up_to_date', version: null }), null)).toBeNull();
    expect(bannerFor(u({ status: 'hold' }), null)).toBeNull();
    expect(bannerFor(null, null)).toBeNull();
  });

  it('offers a .deb as a download', () => {
    const b = bannerFor(u({ install: 'download' }), null)!;
    expect(b.action).toBe('download');
    expect(b.text).toContain('download it to update');
  });

  it('a mandatory or required update cannot be dismissed', () => {
    const m = bannerFor(u({ mandatory: true, deadline: '2026-10-20T00:00:00Z' }), null)!;
    expect(m).toMatchObject({ tone: 'warn', dismissible: false });
    expect(m.text).toBe('claude-fleet 0.5.5 is required by 2026-10-20 — restart to update.');
    const r = bannerFor(u({ status: 'update_required', reason: 'desktop 0.5.4 can no longer talk to this hub.' }), '0.5.5')!;
    expect(r).toMatchObject({ tone: 'warn', action: 'restart', dismissible: false });
    expect(r.text).toContain('Update required');
  });

  it('a required update with nothing to install says so and offers no button', () => {
    const r = bannerFor(u({ status: 'update_required', version: null, install: 'none' }), null)!;
    expect(r.action).toBeNull();
    expect(r.text).toContain('No release this platform can install fits yet');
  });

  it('newer than the hub names the hub', () => {
    const r = bannerFor(u({ status: 'client_too_new', reason: 'the hub needs updating.', version: null }), null)!;
    expect(r).toEqual({ tone: 'warn', text: 'the hub needs updating.', action: null, dismissible: false });
  });
});

describe('nextCheckMs', () => {
  it('follows the decision within an hour and a day', () => {
    expect(nextCheckMs(u({ next_check_secs: 7200 }))).toBe(7_200_000);
    expect(nextCheckMs(u({ next_check_secs: 60 }))).toBe(3_600_000);
    expect(nextCheckMs(u({ next_check_secs: 10_000_000 }))).toBe(86_400_000);
    expect(nextCheckMs(null)).toBe(21_600_000);
  });
});
