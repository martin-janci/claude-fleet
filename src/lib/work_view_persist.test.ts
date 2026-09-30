// Per-view state in the Work view (work graph M14): the selected task and
// the expanded sections are kept per saved view (`custom` or `view:<id>`),
// switching views restores that view's selection, and all of it survives a
// restart through the prefs in localStorage.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

type WorkView = typeof import('./work_view');

const PREFIX = 'cf:pref:';
const stored = (key: string): unknown => {
  const raw = localStorage.getItem(PREFIX + key);
  return raw === null ? undefined : JSON.parse(raw);
};

async function load(): Promise<WorkView> {
  vi.resetModules();
  return import('./work_view');
}

describe('work view per-view persistence', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('keeps the selection per view and restores it on a switch', async () => {
    const w = await load();
    expect(get(w.workViewKey)).toBe('custom');
    expect(get(w.selectedTaskId)).toBeNull();

    w.selectedTaskId.set('item:1');
    w.activeWorkViewId.set(5);
    expect(get(w.workViewKey)).toBe('view:5');
    expect(get(w.selectedTaskId)).toBeNull();
    // Switching must not have cleared the custom view's entry.
    expect(stored('work.selected')).toEqual({ custom: 'item:1' });

    w.selectedTaskId.set('item:2');
    expect(stored('work.selected')).toEqual({ custom: 'item:1', 'view:5': 'item:2' });

    w.activeWorkViewId.set(null);
    expect(get(w.selectedTaskId)).toBe('item:1');
    w.activeWorkViewId.set(5);
    expect(get(w.selectedTaskId)).toBe('item:2');

    // Clearing the selection in view 5 drops only view 5's entry.
    w.selectedTaskId.set(null);
    expect(stored('work.selected')).toEqual({ custom: 'item:1' });
    w.activeWorkViewId.set(null);
    expect(get(w.selectedTaskId)).toBe('item:1');
  });

  it('keeps expansion per view', async () => {
    const w = await load();
    w.setExpanded('custom', 'org:1', true);
    w.setExpanded('view:5', 'org:1', false);
    w.setExpanded('view:5', 'org:1|grp', true);
    expect(get(w.workExpanded)).toEqual({
      custom: { 'org:1': true },
      'view:5': { 'org:1': false, 'org:1|grp': true },
    });
    expect(stored('work.expanded')).toEqual(get(w.workExpanded));
  });

  it('reads the view, its selection and its expansion back after a restart', async () => {
    const w = await load();
    w.selectedTaskId.set('item:1');
    w.activeWorkViewId.set(5);
    w.selectedTaskId.set('item:2');
    w.setExpanded('view:5', 'org:2', true);
    w.setExpanded('custom', 'org:1', false);
    expect(stored('work.view_id')).toBe(5);

    const again = await load();
    expect(again).not.toBe(w);
    expect(get(again.activeWorkViewId)).toBe(5);
    expect(get(again.workViewKey)).toBe('view:5');
    expect(get(again.selectedTaskId)).toBe('item:2');
    expect(get(again.workExpanded)).toEqual({ 'view:5': { 'org:2': true }, custom: { 'org:1': false } });
    again.activeWorkViewId.set(null);
    expect(get(again.selectedTaskId)).toBe('item:1');
  });

  it('falls back to empty state on malformed stored values', async () => {
    localStorage.setItem(PREFIX + 'work.selected', JSON.stringify([1, 2]));
    localStorage.setItem(PREFIX + 'work.expanded', JSON.stringify({ custom: { 'org:1': 'yes' } }));
    localStorage.setItem(PREFIX + 'work.view_id', JSON.stringify('5'));
    const w = await load();
    expect(get(w.activeWorkViewId)).toBeNull();
    expect(get(w.workViewKey)).toBe('custom');
    expect(get(w.selectedTaskId)).toBeNull();
    expect(get(w.workExpanded)).toEqual({});
    // The guards replaced the bad values, so a selection is a plain map again.
    w.selectedTaskId.set('item:3');
    expect(stored('work.selected')).toEqual({ custom: 'item:3' });
  });

  it('a selected map with a non-string value is malformed too', async () => {
    localStorage.setItem(PREFIX + 'work.selected', JSON.stringify({ custom: 7 }));
    const w = await load();
    expect(get(w.selectedTaskId)).toBeNull();
  });
});
