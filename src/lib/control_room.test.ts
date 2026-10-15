import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import {
  RIGHT_MAX,
  RIGHT_MIN,
  activeThread,
  clampLeft,
  clampRight,
  controlMission,
  controlRightPx,
  dragRight,
  openInControl,
  openMissionInControl,
} from './control_room';
import { controlViews, defaultLayout } from './control_views';
import { sessions } from './sessions';
import { selectedSession, onSessionOpened } from './selection';
import { destination } from './destination';
import { toasts, clearToasts } from './toasts';
import { session } from './hosts_fixture';
import { readPref } from './prefs';

// Thread room (2026-10-14): the right panel's width and the session it shows.

beforeEach(() => {
  controlViews.set(defaultLayout());
  controlRightPx.set(380);
  clearToasts();
});

describe('the right panel', () => {
  it('stays between 320 and 640 px', () => {
    expect(clampRight(100)).toBe(RIGHT_MIN);
    expect(clampRight(9999)).toBe(RIGHT_MAX);
    expect(clampLeft(10)).toBe(200);
  });

  it('a drag on its left edge widens it, and a drag to zero hides it', () => {
    // The handle sits on the panel's left edge: dragging left (negative) widens.
    let raw = dragRight(380, -100);
    expect(get(controlRightPx)).toBe(480);
    raw = dragRight(raw, 1000);
    expect(get(controlViews).open).toBe(false);
    expect(get(controlRightPx)).toBe(480);
    expect(raw).toBe(RIGHT_MIN);
  });

  it('remembers its width the way the sidebar does', async () => {
    vi.useFakeTimers();
    controlRightPx.set(512);
    vi.advanceTimersByTime(300);
    vi.useRealTimers();
    expect(readPref('layout.control-right', 0, (v): v is number => typeof v === 'number')).toBe(512);
  });
});

describe('opening in Control', () => {
  it('a session opens in the Session tab, in place', () => {
    const s = session('mac', 'fix');
    sessions.set([s]);
    destination.set('control');
    const opened = vi.fn();
    const off = onSessionOpened(opened);
    expect(openInControl(s.id)).toBe(true);
    off();
    expect(opened).not.toHaveBeenCalled();
    expect(get(selectedSession)?.id).toBe(s.id);
    expect(get(controlViews)).toMatchObject({ active: 'session', open: true });
    expect(get(activeThread)).toEqual({ kind: 'session', id: s.id });
    expect(get(destination)).toBe('control');
  });

  it('a session the fleet no longer has says so', () => {
    sessions.set([]);
    expect(openInControl(42, 'gone-one')).toBe(false);
    expect(get(toasts).at(-1)?.message).toContain('gone-one is gone');
  });

  it('a mission opens in the Missions view', () => {
    openMissionInControl(5);
    expect(get(controlMission)).toBe(5);
    expect(get(controlViews).active).toBe('missions');
    expect(get(activeThread)).toEqual({ kind: 'mission', id: 5 });
  });
});
