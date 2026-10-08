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
