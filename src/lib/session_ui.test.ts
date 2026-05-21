import { describe, it, expect, beforeEach } from 'vitest';
import {
  loadSessionUi,
  saveSessionUi,
  migrateSessionUi,
  forgetSessionUi,
  DEFAULT_UI,
} from './session_ui';

beforeEach(() => {
  localStorage.clear();
});

describe('session_ui', () => {
  it('returns defaults for an unknown session', () => {
    const ui = loadSessionUi('local', 'dev-foo');
    expect(ui).toEqual({ centerPx: 360 });
  });

  it('persists centerPx only (centerCollapsed is now a global pref)', () => {
    saveSessionUi('local', 'dev-foo', { centerPx: 200 });
    const ui = loadSessionUi('local', 'dev-foo');
    expect(ui).toEqual({ centerPx: 200 });
  });

  it('keeps sessions on different hosts separate', () => {
    saveSessionUi('local', 'dev-foo', { centerPx: 200 });
    saveSessionUi('remote', 'dev-foo', { centerPx: 400 });
    expect(loadSessionUi('local', 'dev-foo').centerPx).toBe(200);
    expect(loadSessionUi('remote', 'dev-foo').centerPx).toBe(400);
  });

  it('migrates a session key when the tmux name is renamed', () => {
    saveSessionUi('local', 'dev-foo', { centerPx: 250 });
    migrateSessionUi('local', 'dev-foo', 'dev-bar');
    expect(loadSessionUi('local', 'dev-foo')).toEqual(DEFAULT_UI);
    expect(loadSessionUi('local', 'dev-bar').centerPx).toBe(250);
  });

  it('migration is a no-op when nothing was stored for the old name', () => {
    migrateSessionUi('local', 'never-seen', 'dev-foo');
    expect(loadSessionUi('local', 'dev-foo')).toEqual(DEFAULT_UI);
  });

  it('forgetSessionUi removes stored state', () => {
    saveSessionUi('local', 'dev-foo', { centerPx: 500 });
    forgetSessionUi('local', 'dev-foo');
    expect(loadSessionUi('local', 'dev-foo')).toEqual(DEFAULT_UI);
  });

  it('tolerates corrupt JSON in localStorage', () => {
    localStorage.setItem('cf:session-ui', '{not-json');
    expect(loadSessionUi('local', 'dev-foo')).toEqual(DEFAULT_UI);
    // Subsequent save should overwrite the corrupt entry.
    saveSessionUi('local', 'dev-foo', { centerPx: 333 });
    expect(loadSessionUi('local', 'dev-foo').centerPx).toBe(333);
  });
});
