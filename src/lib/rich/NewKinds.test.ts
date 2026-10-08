// The chat kinds of step 10.2: progress (one card per id that updates in
// place), results (page widgets from the block's own data) and error (code
// and next step).
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock('../clipboard', () => ({ copyText: vi.fn(() => Promise.resolve(true)) }));

import RichText from '../RichText.svelte';
import { composerDrafts } from '../conversation';

const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', ...o }) + '\n```';

beforeEach(() => {
  composerDrafts.clear();
  document.body.innerHTML = '';
});

/** Replies as the Conversation panel lays them out: one RichText per text
 *  block, inside a `.conversation-panel`. */
function panel(): HTMLElement {
  const root = document.createElement('div');
  root.className = 'conversation-panel';
  document.body.appendChild(root);
  return root;
}
function reply(root: HTMLElement, source: string, sessionId: number | null = 1) {
  const target = document.createElement('div');
  root.appendChild(target);
  return render(RichText, { target, props: { source, sessionId } });
}

describe('progress', () => {
  it('shows a count against a total as a meter, and the steps', () => {
    reply(panel(), ui({ kind: 'progress', id: 'deploy', title: 'Deploying', done: 3, total: 7, unit: 'hosts', steps: [{ title: 'Build', state: 'done' }, { title: 'Push', state: 'running' }] }));
    expect(screen.getByTestId('rich-progress-count').textContent).toBe('3 of 7 hosts');
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe('3');
    expect(screen.getByTestId('rich-progress-state').textContent).toBe('Running');
    expect(screen.getAllByTestId('rich-progress-step').map((s) => s.dataset.state)).toEqual(['done', 'running']);
  });

  it('shows an unknown size as a count with no meter', () => {
    reply(panel(), ui({ kind: 'progress', id: 'imp', title: 'Importing', done: 120, unit: 'rows' }));
    expect(screen.getByTestId('rich-progress-count').textContent).toBe('120 rows so far');
    expect(screen.queryByRole('progressbar')).toBeNull();
  });

  it('updates in place: the first card shows the newest state, later ones point up', async () => {
    const root = panel();
    reply(root, ui({ kind: 'progress', id: 'deploy', title: 'Deploying', done: 1, total: 3 }));
    reply(root, 'Still going.\n\n' + ui({ kind: 'progress', id: 'deploy', title: 'Deploying', done: 2, total: 3 }));
    reply(root, ui({ kind: 'progress', id: 'deploy', title: 'Deployed', state: 'done', done: 3, total: 3 }));
    await tick();
    const cards = screen.getAllByTestId('rich-progress');
    expect(cards).toHaveLength(1);
    expect(cards[0].dataset.state).toBe('done');
    expect(screen.getByTestId('rich-progress-count').textContent).toBe('3 of 3');
    expect(screen.getByTestId('rich-progress-updates').textContent).toBe('2 updates below');
    expect(screen.getAllByTestId('rich-progress-moved')).toHaveLength(2);
  });

  it('keeps two ids, and the same id in two conversations, apart', async () => {
    const a = panel();
    const b = panel();
    reply(a, ui({ kind: 'progress', id: 'x', title: 'A', done: 1, total: 2 }));
    reply(a, ui({ kind: 'progress', id: 'y', title: 'B', done: 1, total: 2 }));
    reply(b, ui({ kind: 'progress', id: 'x', title: 'C', state: 'failed' }));
    await tick();
    expect(screen.getAllByTestId('rich-progress').map((c) => c.getAttribute('aria-label'))).toEqual(['A', 'B', 'C']);
    expect(screen.queryByTestId('rich-progress-moved')).toBeNull();
  });

  it('gives the newest state back to the first card when a later one goes away', async () => {
    const root = panel();
    reply(root, ui({ kind: 'progress', id: 'd', title: 'D', done: 1, total: 2 }));
    const later = reply(root, ui({ kind: 'progress', id: 'd', title: 'D', done: 2, total: 2 }));
    await tick();
    expect(screen.getByTestId('rich-progress-count').textContent).toBe('2 of 2');
    later.unmount();
    await tick();
    expect(screen.getByTestId('rich-progress-count').textContent).toBe('1 of 2');
  });
});

describe('results', () => {
  it('formats stats by their type and draws a chart and a table', () => {
    reply(panel(), ui({ kind: 'results', title: 'Benchmark', items: [
      { type: 'stat', label: 'Spend', value: 1830000, ty: 'usd_micros' },
      { type: 'stat', label: 'Requests', value: 12500 },
      { type: 'stat', label: 'Verdict', value: 'faster' },
      { type: 'chart', chart: 'bar', title: 'Per day', x: { label: 'Day', ty: 'day' }, y: { label: 'Requests' }, points: [['mon', 3], ['tue', 5]] },
      { type: 'table', columns: [{ label: 'Route' }, { label: 'ms' }, { label: 'Cached' }], rows: [['/a', 1200, true], ['/b', null, false]] },
    ] }));
    expect(screen.getAllByTestId('rich-results-stat').map((s) => s.querySelector('.value')!.textContent)).toEqual(['$1.83', '12,500', 'faster']);
    expect(screen.getByTestId('rich-results-chart')).toBeTruthy();
    const cells = Array.from(screen.getByTestId('rich-results-table').querySelectorAll('td')).map((td) => td.textContent);
    expect(cells).toEqual(['/a', '1,200', 'yes', '/b', '—', 'no']);
  });
});

describe('error', () => {
  it('shows the code, the detail under a fold, and fills the composer with a next step', async () => {
    reply(panel(), ui({ kind: 'error', code: 'E_SSH', title: 'mercury did not answer', detail: 'timed out', next: [{ label: 'Retry', prompt: 'Try mercury again' }] }), 4);
    expect(screen.getByTestId('rich-error-code').textContent).toBe('E_SSH');
    expect(screen.getByTestId('rich-error-detail').textContent).toBe('timed out');
    await fireEvent.click(screen.getByTestId('rich-error-next'));
    expect(composerDrafts.get(4)).toBe('Try mercury again');
  });

  it('turns its next steps off in a read-only view', () => {
    reply(panel(), ui({ kind: 'error', code: 'E', title: 'T', next: [{ label: 'Retry', prompt: 'again' }] }), null);
    expect((screen.getByTestId('rich-error-next') as HTMLButtonElement).disabled).toBe(true);
  });
});
