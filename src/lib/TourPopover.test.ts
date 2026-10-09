import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import Tour from './Tour.svelte';
import FirstRun from './FirstRun.svelte';
import { TOUR_STEPS, startTour, tourSeen, tourStep } from './tour';
import { onboardingDismissed, onboardingWelcomed } from './onboarding';
import { markStartup, resetStartup } from './startup';

// Redesign step 10.5: the tour's popover, its "try it", and Skip remembered.

let target: HTMLElement;

beforeEach(() => {
  tourSeen.set(false);
  tourStep.set(null);
  target = document.createElement('section');
  target.setAttribute('data-testid', 'pane-sidebar');
  target.getBoundingClientRect = () => ({ left: 69, top: 45, width: 340, height: 420, right: 409, bottom: 465, x: 69, y: 45, toJSON: () => ({}) });
  document.body.appendChild(target);
});

afterEach(() => target.remove());

describe('Tour', () => {
  it('shows nothing until it is started', () => {
    render(Tour, { mac: true });
    expect(screen.queryByTestId('tour')).toBeNull();
  });

  it('spotlights the inbox on step two and ticks "try it" when j is pressed', async () => {
    render(Tour, { mac: true });
    tourStep.set(1);
    const pop = await screen.findByTestId('tour');
    expect(pop.textContent).toContain('Tour · 2 of 6');
    expect(pop.textContent).toContain('Your inbox is what needs you');
    await waitFor(() => expect(screen.getByTestId('tour-spotlight').getAttribute('style')).toContain('width: 340px'));
    expect(pop.getAttribute('data-side')).toBe('right');
    expect(screen.queryByTestId('tour-tried')).toBeNull();
    await fireEvent.keyDown(window, { key: 'j', code: 'KeyJ' });
    expect(screen.getByTestId('tour-tried').textContent).toBe('Done');
  });

  it('centres the popover when its part is not on screen', async () => {
    render(Tour, { mac: true });
    tourStep.set(0);
    expect((await screen.findByTestId('tour')).getAttribute('data-side')).toBe('centre');
    expect(screen.queryByTestId('tour-spotlight')).toBeNull();
  });

  it('Next walks to Done, which ends it for good', async () => {
    render(Tour, { mac: true });
    startTour();
    for (let i = 0; i < TOUR_STEPS.length - 1; i++) await fireEvent.click(await screen.findByTestId('tour-next'));
    expect(screen.getByTestId('tour-next').textContent).toContain('Done');
    await fireEvent.click(screen.getByTestId('tour-next'));
    expect(screen.queryByTestId('tour')).toBeNull();
    expect(get(tourSeen)).toBe(true);
  });

  it('Skip and Escape end it for good', async () => {
    render(Tour, { mac: true });
    startTour();
    await fireEvent.click(await screen.findByTestId('tour-skip'));
    expect(get(tourSeen)).toBe(true);
    tourSeen.set(false);
    startTour();
    await screen.findByTestId('tour');
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(get(tourStep)).toBeNull();
    expect(get(tourSeen)).toBe(true);
  });
});

describe('FirstRun', () => {
  beforeEach(() => {
    resetStartup();
    onboardingWelcomed.set(true);
    onboardingDismissed.set(true);
  });

  it('starts the tour once the startup loads are in, after the welcome', async () => {
    render(FirstRun, { mac: true });
    expect(screen.queryByTestId('tour')).toBeNull();
    markStartup('done');
    expect(await screen.findByTestId('tour')).toBeTruthy();
  });

  it('a skipped tour does not start again', async () => {
    tourSeen.set(true);
    markStartup('done');
    render(FirstRun, { mac: true });
    await Promise.resolve();
    expect(screen.queryByTestId('tour')).toBeNull();
  });

  it('holds Get started back while the tour runs', async () => {
    onboardingDismissed.set(false);
    tourSeen.set(true);
    markStartup('done');
    render(FirstRun, { mac: true });
    expect(screen.getByTestId('get-started')).toBeTruthy();
    startTour();
    await waitFor(() => expect(screen.queryByTestId('get-started')).toBeNull());
  });
});
