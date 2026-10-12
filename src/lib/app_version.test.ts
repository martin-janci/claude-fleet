import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const getVersion = vi.fn(async () => '0.4.5');
vi.mock('@tauri-apps/api/app', () => ({ getVersion: () => getVersion() }));

import { appVersion, loadAppVersion, versionLine } from './app_version';

const health = (over: Partial<{ version: string; db_ready: boolean; schema_version: number }> = {}) => ({
  version: '0.4.6',
  db_ready: true,
  schema_version: 90,
  ...over,
});

describe('versionLine', () => {
  it('standalone names one version, because there is one', () => {
    const v = versionLine({ app: '0.4.5', health: health({ version: '0.4.5' }), remote: false, hubUrl: null });
    // Redesign 1.4: the strip says how things are; the numbers are on hover.
    expect(v.text).toBe('All systems OK');
    expect(v.title.startsWith('app 0.4.5 · db: ok · schema 90.')).toBe(true);
    expect(v.title).toContain('This app owns the fleet');
    expect(v.title).toContain('No hub');
  });

  it('standalone falls back to the health version, which IS this app there', () => {
    // `getVersion()` has not resolved yet (or failed). Standalone that costs
    // nothing: `health_check` did not route anywhere, so its version is ours.
    const v = versionLine({ app: null, health: health({ version: '0.4.5' }), remote: false, hubUrl: null });
    expect(v.title.startsWith('app 0.4.5 · db: ok · schema 90.')).toBe(true);
  });

  it('paired names both, and says which database the schema belongs to', () => {
    const v = versionLine({
      app: '0.4.5',
      health: health(),
      remote: true,
      hubUrl: 'https://fleet.rlt.sk',
    });
    // The whole point: two numbers, each labelled, in the one strip that is
    // always on screen.
    expect(v.text).toBe('All systems OK');
    expect(v.title.startsWith('app 0.4.5 · hub 0.4.6 · db: ok · schema 90.')).toBe(true);
    expect(v.title).toContain('app 0.4.5 is this window');
    expect(v.title).toContain('hub 0.4.6 is https://fleet.rlt.sk');
    expect(v.title).toContain('schema 90');
  });

  it('explains a difference between the two rather than leaving it to look broken', () => {
    const differ = versionLine({ app: '0.4.5', health: health(), remote: true, hubUrl: null });
    expect(differ.title).toContain('The two differ');
    expect(differ.title).toContain('wire contract');
    // Equal versions get no such sentence — there is nothing to explain.
    const same = versionLine({
      app: '0.4.6',
      health: health(),
      remote: true,
      hubUrl: null,
    });
    expect(same.title.startsWith('app 0.4.6 · hub 0.4.6 · db: ok · schema 90.')).toBe(true);
    expect(same.title).not.toContain('differ');
  });

  it('paired with no app version still labels the hub as the hub', () => {
    const v = versionLine({ app: null, health: health(), remote: true, hubUrl: 'https://h' });
    expect(v.title.startsWith('hub 0.4.6 · db: ok · schema 90.')).toBe(true);
    expect(v.title).toContain("This app's own version could not be read");
  });

  it('a failed database is named on whichever side it is', () => {
    const mine = versionLine({ app: '1', health: health({ db_ready: false }), remote: false, hubUrl: null });
    expect(mine.text).toBe('Database not ready');
    expect(mine.title).toContain('db: fail');
    const hubs = versionLine({ app: '1', health: health({ db_ready: false }), remote: true, hubUrl: null });
    expect(hubs.text).toBe('Database not ready');
    expect(hubs.title).toContain('db: fail');
  });

  it('with no health at all it is this app and nothing else', () => {
    // The hub-unavailable footer: no fleet to describe, and the one fact
    // still true is which app is running.
    const v = versionLine({ app: '0.4.5', health: null, remote: false, hubUrl: null });
    expect(v.text).toBe('app 0.4.5');
    expect(v.text).not.toContain('schema');
    expect(versionLine({ app: null, health: null, remote: true, hubUrl: null }).text).toBe('');
  });
});

describe('loadAppVersion', () => {
  beforeEach(() => {
    appVersion.set(null);
    getVersion.mockReset();
  });

  it('records what the runtime reports', async () => {
    getVersion.mockResolvedValueOnce('0.4.5');
    await loadAppVersion();
    expect(get(appVersion)).toBe('0.4.5');
  });

  it('leaves it unknown rather than throwing when the runtime will not say', async () => {
    getVersion.mockRejectedValueOnce(new Error('no tauri here'));
    await expect(loadAppVersion()).resolves.toBeUndefined();
    expect(get(appVersion)).toBeNull();
  });

  it('treats a blank answer as no answer', async () => {
    getVersion.mockResolvedValueOnce('   ');
    await loadAppVersion();
    expect(get(appVersion)).toBeNull();
  });
});
