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
  it('starts on Needs you with every landed view shown; Library waits on 9.7', () => {
    const l = defaultLayout();
    expect(l.order).toEqual(['needs-you', 'session', 'prs', 'today']);
    expect(l.active).toBe('needs-you');
    expect(l.open).toBe(true);
  });

  it('makes a stored layout whole: unknown ids dropped, new views appended, a hidden active replaced', () => {
    const l = normalizeLayout({ order: ['today', 'bogus', 'today', 'library'], hidden: ['today', 'x'], active: 'today', open: false });
    expect(l.order).toEqual(['today', 'needs-you', 'session', 'prs']);
    expect(l.hidden).toEqual(['today']);
    expect(l.active).toBe('needs-you');
    expect(l.open).toBe(false);
    expect(normalizeLayout('junk')).toEqual(defaultLayout());
  });

  it('"+" turns views off and on, and never the last one', () => {
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['needs-you', 'session', 'today']);
    toggleView('needs-you');
    toggleView('session');
    expect(get(controlViews).active).toBe('today');
    toggleView('today');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['today']);
    toggleView('prs');
    expect(shownViews(get(controlViews)).map((v) => v.id)).toEqual(['prs', 'today']);
  });

  it('"+" reorders within the strip and stops at its ends', () => {
    moveView('today', -1);
    expect(get(controlViews).order).toEqual(['needs-you', 'session', 'today', 'prs']);
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
  });
});
