// The states kit (redesign step 10.6): one test per state. A loading state
// waits 400 ms, an empty state says what happened and offers the next step,
// a search with no results always has a way out, and an offline host is said
// inside its own pane with Try again.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, afterEach } from 'vitest';
import Skeleton from './Skeleton.svelte';
import EmptyState from './EmptyState.svelte';
import HostOffline from './HostOffline.svelte';
import { LOADING_DELAY_MS, sinceWords } from './states';
import { motionPref } from '../motion';

afterEach(() => vi.useRealTimers());

describe('skeleton', () => {
  it('shows nothing before 400 ms, so a quick load never flashes', async () => {
    vi.useFakeTimers();
    render(Skeleton, { props: { rows: 4 } });
    expect(LOADING_DELAY_MS).toBe(400);
    await vi.advanceTimersByTimeAsync(399);
    expect(screen.queryByTestId('skeleton')).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    const s = screen.getByTestId('skeleton');
    expect(s.getAttribute('aria-busy')).toBe('true');
    expect(s.querySelectorAll('.bar')).toHaveLength(4);
  });

  // Review r12: the app's Motion setting, not only the OS query, governs it,
  // by the Loader kit's rule: Reduced fades slowly, Off rests.
  it('fades its bars under Reduced motion and rests them under Off', async () => {
    vi.useFakeTimers();
    motionPref.set('reduced');
    try {
      const { unmount } = render(Skeleton);
      await vi.advanceTimersByTimeAsync(LOADING_DELAY_MS);
      let el = screen.getByTestId('skeleton');
      expect(el.classList.contains('fade')).toBe(true);
      expect(el.classList.contains('still')).toBe(false);
      unmount();
      motionPref.set('off');
      render(Skeleton);
      await vi.advanceTimersByTimeAsync(LOADING_DELAY_MS);
      el = screen.getByTestId('skeleton');
      expect(el.classList.contains('still')).toBe(true);
      expect(el.classList.contains('fade')).toBe(false);
    } finally {
      motionPref.set('system');
    }
  });

  it('a load that ends first never shows one', async () => {
    vi.useFakeTimers();
    const { unmount } = render(Skeleton);
    await vi.advanceTimersByTimeAsync(200);
    unmount();
    await vi.advanceTimersByTimeAsync(400);
    expect(screen.queryByTestId('skeleton')).toBeNull();
  });

  it('says what is slow', async () => {
    vi.useFakeTimers();
    render(Skeleton, { props: { slow: 'mercury is slow (2.4 s)' } });
    await vi.advanceTimersByTimeAsync(LOADING_DELAY_MS);
    expect(screen.getByTestId('skeleton-slow').textContent).toBe('mercury is slow (2.4 s)');
  });
});

describe('empty state', () => {
  it('a calm one says nothing needs you, with no void', () => {
    render(EmptyState, { props: { title: 'Nothing needs you', body: '6 sessions are running.' } });
    const e = screen.getByTestId('empty-state');
    expect(e.dataset.kind).toBe('calm');
    expect(e.textContent).toContain('6 sessions are running.');
  });

  it('no results always offers the way out', async () => {
    const clear = vi.fn();
    render(EmptyState, {
      props: { kind: 'none', title: 'No live session matches', actions: [{ label: 'Search without filters', onclick: clear, testid: 'way-out' }] },
    });
    await fireEvent.click(screen.getByTestId('way-out'));
    expect(clear).toHaveBeenCalledOnce();
  });
});

describe('host offline', () => {
  const NOW = 1_800_000_000;

  it('says how long ago it answered and what it means for its sessions', () => {
    render(HostOffline, { props: { alias: 'claude-fleet-trn', lastSeen: NOW - 360, now: NOW, sessions: 2, reason: 'SSH timed out after 10 s', code: 'E_SSH_TIMEOUT' } });
    const t = screen.getByTestId('host-offline-state').textContent ?? '';
    expect(t).toContain('claude-fleet-trn is offline');
    expect(t).toContain('last answered 6 m ago');
    expect(t).toContain('SSH timed out after 10 s.');
    expect(t).toContain('2 sessions are probably still running in tmux');
    expect(screen.getByTestId('host-offline-code').textContent).toBe('E_SSH_TIMEOUT');
  });

  // Redesign 3.14: the paused sessions, Show sessions, and Wake host only
  // where the host can be woken.
  it('lists its sessions as Paused and offers Show sessions', async () => {
    const onshow = vi.fn();
    render(HostOffline, { props: { alias: 'mercury', paused: ['api', 'web', 'docs', 'ops', 'ml', 'ci'], onshow } });
    const list = screen.getByTestId('host-offline-paused');
    const items = Array.from(list.querySelectorAll('li')).map((li) => li.textContent?.trim());
    expect(items).toEqual(['Paused api', 'Paused web', 'Paused docs', 'Paused ops', 'and 2 more']);
    await fireEvent.click(screen.getByTestId('host-offline-show'));
    expect(onshow).toHaveBeenCalledOnce();
  });

  it('offers Wake host only for a host that can be woken', async () => {
    const { rerender } = render(HostOffline, { props: { alias: 'mercury', ontry: vi.fn() } });
    expect(screen.queryByTestId('host-offline-wake')).toBeNull();
    expect(screen.queryByTestId('host-offline-paused')).toBeNull();
    const onwake = vi.fn();
    await rerender({ alias: 'mercury', ontry: vi.fn(), onwake });
    await fireEvent.click(screen.getByTestId('host-offline-wake'));
    expect(onwake).toHaveBeenCalledOnce();
    await rerender({ alias: 'mercury', ontry: vi.fn(), onwake, waking: true });
    expect(screen.getByTestId('host-offline-wake').textContent).toBe('Waking…');
  });

  it('marks it with Signal lost, which plays once and rests (redesign 3.14)', () => {
    render(HostOffline, { props: { alias: 'mercury' } });
    const mark = screen.getByTestId('host-offline-mark');
    expect(mark.dataset.loader).toBe('signal-lost');
    expect(mark.classList).toContain('ofl-name-signal-lost');
  });

  it('Try again runs the probe, and is off while one runs or the hub blocks it', async () => {
    const ontry = vi.fn();
    const { rerender } = render(HostOffline, { props: { alias: 'mercury', ontry } });
    await fireEvent.click(screen.getByTestId('host-offline-try'));
    expect(ontry).toHaveBeenCalledOnce();
    await rerender({ alias: 'mercury', ontry, trying: true });
    expect(screen.getByTestId('host-offline-try').textContent).toBe('Trying…');
    await rerender({ alias: 'mercury', ontry, tryBlocked: 'needs the hub' });
    const b = screen.getByTestId('host-offline-try') as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    expect(b.title).toBe('needs the hub');
  });

  it('since in words', () => {
    expect(sinceWords(null, NOW)).toBeNull();
    expect(sinceWords(NOW - 5, NOW)).toBe('5 s');
    expect(sinceWords(NOW - 7200, NOW)).toBe('2 h');
    expect(sinceWords(NOW - 3 * 86_400, NOW)).toBe('3 d');
  });
});

describe('a failed load (review r13)', () => {
  it('says the read failed in a sentence, keeps the code under Details, and Retry re-runs it', async () => {
    const { default: LoadError } = await import('./LoadError.svelte');
    const onretry = vi.fn();
    render(LoadError, {
      props: { title: "Couldn't load the library", error: { code: 'E_HUB_TIMEOUT', message: 'deadline 10 s' }, onretry },
    });
    expect(screen.getByTestId('load-error-text').textContent).toBe('The hub took too long to answer.');
    expect(screen.getByTestId('load-error-code').closest('details')).not.toBeNull();
    expect(screen.getByTestId('load-error-code').textContent).toBe('E_HUB_TIMEOUT: deadline 10 s');
    await fireEvent.click(screen.getByTestId('load-error-retry'));
    expect(onretry).toHaveBeenCalledOnce();
  });
});
