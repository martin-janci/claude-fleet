import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  TOUR_STEPS,
  endTour,
  matchesChord,
  matchesStep,
  nextTourStep,
  placePopover,
  prevTourStep,
  startTour,
  tourSeen,
  tourStep,
} from './tour';

// Redesign step 10.5: the first-run tour's steps, keys and placement.

const key = (k: string, code: string, mods: Partial<KeyboardEvent> = {}) => ({
  key: k,
  code,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

beforeEach(() => {
  tourSeen.set(false);
  tourStep.set(null);
});

describe('the tour', () => {
  it('has six steps, each with a target and a key to try', () => {
    expect(TOUR_STEPS).toHaveLength(6);
    for (const s of TOUR_STEPS) {
      expect(s.targets.length).toBeGreaterThan(0);
      expect(s.chord).not.toBe('');
    }
    expect(TOUR_STEPS[1].title).toBe('Your inbox is what needs you');
  });

  it('walks forward and back, and finishing it is remembered', () => {
    startTour();
    expect(get(tourStep)).toBe(0);
    prevTourStep();
    expect(get(tourStep)).toBe(0);
    for (let i = 1; i < TOUR_STEPS.length; i++) nextTourStep();
    expect(get(tourStep)).toBe(5);
    expect(get(tourSeen)).toBe(false);
    nextTourStep();
    expect(get(tourStep)).toBeNull();
    expect(get(tourSeen)).toBe(true);
  });

  it('Skip is remembered', () => {
    startTour();
    nextTourStep();
    endTour();
    expect(get(tourStep)).toBeNull();
    expect(get(tourSeen)).toBe(true);
    expect(localStorage.getItem('cf:pref:tour-seen') ?? '').toContain('true');
  });
});

describe('matchesChord', () => {
  it('reads ⌘ as Meta on the Mac and Ctrl elsewhere', () => {
    expect(matchesChord(key('k', 'KeyK', { metaKey: true }), '⌘K', true)).toBe(true);
    expect(matchesChord(key('k', 'KeyK', { ctrlKey: true }), '⌘K', true)).toBe(false);
    expect(matchesChord(key('k', 'KeyK', { ctrlKey: true }), '⌘K', false)).toBe(true);
    expect(matchesChord(key('k', 'KeyK'), '⌘K', false)).toBe(false);
  });

  it('takes ⌥ by the physical key, whatever symbol it types', () => {
    expect(matchesChord(key('∫', 'KeyB', { metaKey: true, altKey: true }), '⌥⌘B', true)).toBe(true);
    expect(matchesChord(key('b', 'KeyB', { metaKey: true }), '⌥⌘B', true)).toBe(false);
  });

  it('a bare key wants no modifier, and ? is whatever types it', () => {
    expect(matchesChord(key('j', 'KeyJ'), 'j', true)).toBe(true);
    expect(matchesChord(key('j', 'KeyJ', { metaKey: true }), 'j', true)).toBe(false);
    expect(matchesChord(key('?', 'Slash', { shiftKey: true }), '?', false)).toBe(true);
  });
});

describe('matchesStep', () => {
  const step = (id: string) => TOUR_STEPS.find((s) => s.id === id)!;
  it('takes the global chords from the registry, so off the Mac they are the ones the app answers', () => {
    expect(matchesStep(key('K', 'KeyK', { ctrlKey: true, shiftKey: true }), step('command'), false)).toBe(true);
    expect(matchesStep(key('k', 'KeyK', { ctrlKey: true }), step('command'), false)).toBe(false);
    expect(matchesStep(key('k', 'KeyK', { metaKey: true }), step('command'), true)).toBe(true);
    expect(matchesStep(key('J', 'KeyJ', { ctrlKey: true, shiftKey: true }), step('session'), false)).toBe(true);
    expect(matchesStep(key('E', 'KeyE', { ctrlKey: true, shiftKey: true }), step('control'), false)).toBe(true);
    expect(matchesStep(key('b', 'KeyB', { ctrlKey: true, altKey: true }), step('inspector'), false)).toBe(true);
  });

  it('keeps the bare keys on their chord', () => {
    expect(matchesStep(key('j', 'KeyJ'), step('inbox'), false)).toBe(true);
    expect(matchesStep(key('?', 'Slash', { shiftKey: true }), step('status'), false)).toBe(true);
  });
});

describe('placePopover', () => {
  const pop = { width: 320, height: 200 };
  it('goes to the right of a narrow target, as the board draws it', () => {
    expect(placePopover({ left: 69, top: 45, width: 340, height: 420 }, pop, 1440, 900)).toEqual({
      left: 425,
      top: 69,
      side: 'right',
    });
  });

  it('goes inside a target that fills the window, and centres without one', () => {
    const p = placePopover({ left: 0, top: 0, width: 1440, height: 900 }, pop, 1440, 900);
    expect(p.side).toBe('inside');
    expect(p.left + pop.width).toBeLessThanOrEqual(1440);
    expect(placePopover(null, pop, 1440, 900)).toEqual({ left: 560, top: 350, side: 'centre' });
  });

  it('a footer gets the popover above it', () => {
    expect(placePopover({ left: 0, top: 875, width: 1440, height: 25 }, pop, 1440, 900).side).toBe('above');
  });
});
