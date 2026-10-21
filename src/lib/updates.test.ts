import { describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import {
  bannerFor,
  checkForUpdate,
  checkedLine,
  dismissUpdate,
  dismissedUpdate,
  nextCheckMs,
  type DesktopUpdate,
} from './updates';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

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

describe('checkedLine', () => {
  it('says what a person\'s check came back with', () => {
    expect(checkedLine(null)).toBeNull();
    expect(checkedLine(u({ status: 'up_to_date', version: null }))).toBe('Up to date (0.5.4).');
    expect(checkedLine(u())).toBe('claude-fleet 0.5.5 is available.');
    expect(checkedLine(u({ status: 'hold', reason: 'Held: manual mode.' }))).toBe('Held: manual mode.');
  });

  it('never calls a required update available (review w26-10)', () => {
    expect(checkedLine(u({ status: 'update_required', reason: 'This hub no longer serves 0.5.4.' }))).toBe(
      'claude-fleet 0.5.5 is required: This hub no longer serves 0.5.4.',
    );
    expect(checkedLine(u({ mandatory: true }))).toBe('claude-fleet 0.5.5 is required.');
  });
});

describe('dismissUpdate', () => {
  it('remembers the dismissed version for the session, and a check clears it (review w26-9)', () => {
    dismissUpdate('0.5.5');
    expect(get(dismissedUpdate)).toBe('0.5.5');
    expect(sessionStorage.getItem('claude-fleet.update.dismissed')).toBe('0.5.5');
    expect(bannerFor(u(), get(dismissedUpdate))).toBeNull();
    dismissUpdate(null);
    expect(get(dismissedUpdate)).toBeNull();
    expect(sessionStorage.getItem('claude-fleet.update.dismissed')).toBeNull();
    expect(bannerFor(u(), get(dismissedUpdate))?.action).toBe('restart');
  });
});

describe('checkForUpdate', () => {
  it('asks the hub to re-read its channel only when a person asked', async () => {
    const call = vi.mocked(invoke);
    call.mockResolvedValue(u());
    await checkForUpdate();
    expect(call).toHaveBeenLastCalledWith('update_check', undefined);
    await checkForUpdate(true);
    expect(call).toHaveBeenLastCalledWith('update_check', { force: true });
  });
});
