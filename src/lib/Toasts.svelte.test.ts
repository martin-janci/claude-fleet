vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { tick } from 'svelte';
import { resetErrorReportingForTests } from './error_report';
import {
  toasts,
  droppedToasts,
  push,
  dismiss,
  clearToasts,
  MAX_TOASTS,
  INFO_TIMEOUT_MS,
} from './toasts';
import Toasts from './Toasts.svelte';

const inv = () => invoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  clearToasts();
  inv().mockReset();
  inv().mockResolvedValue(undefined);
  resetErrorReportingForTests();
});
afterEach(() => {
  clearToasts();
});

// UX-135's second half: N failing sessions pushed N sticky errors and the
// column grew without limit. The escape hatch sits in the same column, so the
// flood pushed the only way out off the top of the viewport.
describe('toast stack cap', () => {
  it('never grows past MAX_TOASTS, however many distinct errors arrive', () => {
    for (let i = 0; i < MAX_TOASTS + 6; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    expect(get(toasts)).toHaveLength(MAX_TOASTS);
  });

  it('keeps the NEWEST sticky errors: dropping the one that just arrived is the worse bug', () => {
    for (let i = 0; i < MAX_TOASTS + 3; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    const shown = get(toasts).map((t) => t.message);
    expect(shown).toContain(`boom ${MAX_TOASTS + 2}`);
    expect(shown).not.toContain('boom 0');
  });

  it('evicts a transient before a sticky error, whatever their order', () => {
    push({ kind: 'error', code: 'E_FIRST', message: 'the first failure' });
    for (let i = 0; i < MAX_TOASTS - 1; i++) push({ kind: 'info', message: `note ${i}` });
    // Over the cap: the info toasts are the cheap ones — they were going to
    // vanish on their own anyway.
    push({ kind: 'error', code: 'E_LAST', message: 'the last failure' });
    const shown = get(toasts).map((t) => t.message);
    expect(shown).toContain('the first failure');
    expect(shown).toContain('the last failure');
    expect(shown).not.toContain('note 0');
  });

  it('counts what it dropped instead of pretending it never arrived', () => {
    for (let i = 0; i < MAX_TOASTS + 4; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    expect(get(droppedToasts)).toBe(4);
  });

  it('forgets the drop count once the stack is empty again', () => {
    for (let i = 0; i < MAX_TOASTS + 2; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    expect(get(droppedToasts)).toBe(2);
    for (const t of get(toasts)) dismiss(t.id);
    expect(get(toasts)).toHaveLength(0);
    expect(get(droppedToasts)).toBe(0);
  });

  it('clearToasts resets the drop count too', () => {
    for (let i = 0; i < MAX_TOASTS + 2; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    clearToasts();
    expect(get(droppedToasts)).toBe(0);
  });

  it('an evicted toast does not fire its timer against a later id', () => {
    vi.useFakeTimers();
    try {
      for (let i = 0; i < MAX_TOASTS; i++) push({ kind: 'info', message: `note ${i}` });
      push({ kind: 'error', code: 'E_X', message: 'sticky' });
      const survivors = get(toasts).length;
      vi.advanceTimersByTime(INFO_TIMEOUT_MS + 1);
      // The four surviving infos expire; the sticky error stays. Nothing
      // throws and nothing else is removed.
      expect(survivors).toBe(MAX_TOASTS);
      expect(get(toasts)).toHaveLength(1);
      expect(get(toasts)[0].code).toBe('E_X');
    } finally {
      vi.useRealTimers();
    }
  });
});

// The component shipped untested: `toast-dismiss-all` and the removal of the
// nested role="alert" both had zero coverage.
describe('Toasts.svelte', () => {
  it('renders each toast with its code, kind and repeat count', async () => {
    push({ kind: 'error', code: 'E_SSH', message: 'connection refused' });
    push({ kind: 'error', code: 'E_SSH', message: 'connection refused' });
    render(Toasts);
    await tick();
    const shown = screen.getAllByTestId('toast');
    expect(shown).toHaveLength(1);
    expect(shown[0].getAttribute('data-kind')).toBe('error');
    expect(screen.getByTestId('toast-code').textContent).toBe('E_SSH');
    expect(shown[0].textContent).toContain('×2');
  });

  // An assertive role="alert" nested inside the polite role="status" region
  // is undefined behaviour: readers either double-announce or drop one.
  it('has exactly one live region and no nested role="alert"', async () => {
    push({ kind: 'error', code: 'E_A', message: 'a' });
    render(Toasts);
    await tick();
    const region = screen.getByTestId('toasts');
    expect(region.getAttribute('aria-live')).toBe('polite');
    expect(region.getAttribute('role')).toBe('status');
    expect(region.querySelectorAll('[role="alert"]')).toHaveLength(0);
    expect(region.querySelectorAll('[aria-live]')).toHaveLength(0);
  });

  it('offers Dismiss all only once there is more than one, and it clears the stack', async () => {
    push({ kind: 'error', code: 'E_A', message: 'a' });
    render(Toasts);
    await tick();
    expect(screen.queryByTestId('toast-dismiss-all')).toBeNull();
    push({ kind: 'error', code: 'E_B', message: 'b' });
    await tick();
    const all = screen.getByTestId('toast-dismiss-all');
    expect(all.textContent).toContain('2');
    await fireEvent.click(all);
    expect(get(toasts)).toHaveLength(0);
    await tick();
    expect(screen.queryAllByTestId('toast')).toHaveLength(0);
  });

  // The column is anchored at its BOTTOM and grows upward, so its first child
  // is the one a tall stack pushes off the top of the viewport. The escape
  // hatch must not be that child.
  it('keeps Dismiss all at the anchored end of the column, below every toast', async () => {
    for (let i = 0; i < MAX_TOASTS + 2; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    render(Toasts);
    await tick();
    const region = screen.getByTestId('toasts');
    const all = screen.getByTestId('toast-dismiss-all');
    const children = Array.from(region.children);
    const toastEls = screen.getAllByTestId('toast');
    expect(children.indexOf(all.closest('.bar') ?? all)).toBeGreaterThan(
      Math.max(...toastEls.map((el) => children.indexOf(el))),
    );
  });

  it('says how many it dropped rather than lying about the count', async () => {
    for (let i = 0; i < MAX_TOASTS + 3; i++) push({ kind: 'error', code: `E_${i}`, message: `boom ${i}` });
    render(Toasts);
    await tick();
    expect(screen.getAllByTestId('toast')).toHaveLength(MAX_TOASTS);
    expect(screen.getByTestId('toast-dropped').textContent).toContain('3');
    expect(screen.getByTestId('toast-dismiss-all').textContent).toContain(String(MAX_TOASTS));
  });

  it('shows no dropped notice when nothing was dropped', async () => {
    push({ kind: 'error', code: 'E_A', message: 'a' });
    push({ kind: 'error', code: 'E_B', message: 'b' });
    render(Toasts);
    await tick();
    expect(screen.queryByTestId('toast-dropped')).toBeNull();
  });
});
