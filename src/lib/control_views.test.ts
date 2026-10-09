import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  controlViews,
  defaultLayout,
  moveView,
  needsYouList,
  normalizeLayout,
  openElsewhere,
  selectView,
  shownViews,
  toggleView,
} from './control_views';
import { inboxQueue } from './inbox';
import { destination } from './destination';
import { sidebarView } from './work_view';

// Redesign step 9.4: Control's Views panel.

beforeEach(() => controlViews.set(defaultLayout()));

describe('control views layout', () => {
  it('starts on Needs you with every view shown, Library (9.7) and Tasks included', () => {
    const l = defaultLayout();
    expect(l.order).toEqual(['needs-you', 'tasks', 'session', 'prs', 'library', 'today']);
    expect(l.active).toBe('needs-you');
    expect(l.open).toBe(true);
  });

  it('makes a stored layout whole: unknown ids dropped, new views appended, a hidden active replaced', () => {
    const l = normalizeLayout({ order: ['today', 'bogus', 'today', 'library'], hidden: ['today', 'x'], active: 'today', open: false });
    expect(l.order).toEqual(['today', 'library', 'needs-you', 'tasks', 'session', 'prs']);
    expect(l.hidden).toEqual(['today']);
    expect(l.active).toBe('library');
    expect(l.open).toBe(false);
    expect(normalizeLayout('junk')).toEqual(defaultLayout());
  });

  it('"+" turns views off and on, and never the last one', () => {
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['needs-you', 'tasks', 'session', 'library', 'today']);
    toggleView('needs-you');
    toggleView('tasks');
    toggleView('session');
    toggleView('library');
    expect(get(controlViews).active).toBe('today');
    toggleView('today');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['today']);
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['prs', 'today']);
  });

  it('"+" reorders within the strip and stops at its ends', () => {
    moveView('today', -1);
    expect(get(controlViews).order).toEqual(['needs-you', 'tasks', 'session', 'prs', 'today', 'library']);
    moveView('needs-you', -1);
    expect(get(controlViews).order[0]).toBe('needs-you');
  });

  it('selecting a hidden view shows it and opens the column', () => {
    controlViews.update((l) => ({ ...l, open: false, hidden: ['prs'] }));
    selectView('prs');
    expect(get(controlViews)).toMatchObject({ active: 'prs', open: true, hidden: [] });
  });

  it('Needs you is the Inbox query itself', () => {
    expect(needsYouList).toBe(inboxQueue);
  });

  it('links open where the view lives', () => {
    destination.set('control');
    openElsewhere('tasks');
    expect(get(sidebarView)).toBe('work');
    expect(get(destination)).toBe('session');
    destination.set('control');
    openElsewhere('hosts');
    expect(get(destination)).toBe('accounts');
    destination.set('control');
    openElsewhere('routines');
    expect(get(destination)).toBe('automation');
  });
});
