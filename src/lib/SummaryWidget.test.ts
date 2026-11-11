import { render, waitFor, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SummaryWidget from './SummaryWidget.svelte';
import { summaryKey, type Widget } from './dashboards';
import { expectAccessible } from './a11y_check';

const mock = invoke as ReturnType<typeof vi.fn>;
const widget: Extract<Widget, { kind: 'summary' }> = {
  kind: 'summary',
  title: 'What blocks it',
  query: { status: 'open' },
  prompt: 'What blocks the release, in three lines?',
};

beforeEach(() => {
  mock.mockReset();
  localStorage.clear();
});

describe('SummaryWidget', () => {
  it('writes nothing on open, and writes on Write with the widget’s request and counts', async () => {
    mock.mockResolvedValue({ text: 'Two tasks wait on review.', model: 'haiku', host_alias: 'mercury', from: '5 tasks', at: 1_760_000_000, cost_micros: 1_234 });
    const r = render(SummaryWidget, { props: { dashId: 'dash.r42', widget } });
    expect(r.getByText(/Not written yet/)).toBeTruthy();
    expect(mock).not.toHaveBeenCalled();
    await fireEvent.click(r.getByTestId('summary-write'));
    await waitFor(() => expect(r.getByTestId('summary-text').textContent).toBe('Two tasks wait on review.'));
    expect(mock).toHaveBeenCalledWith('dashboard_summary', {
      args: { prompt: widget.prompt, filters: { archived: true, status: 'open' } },
    });
    const meta = r.getByTestId('summary-meta').textContent ?? '';
    expect(meta).toContain('haiku on mercury');
    expect(meta).toContain('$0.0012');
    expect(r.getByTestId('summary-write').textContent?.trim()).toBe('Refresh');
    await expectAccessible(r.container);
  });

  it('shows the text written last for the same request, counts and sources only, with its age', async () => {
    const at = Math.floor(Date.now() / 1000) - 3 * 86_400;
    mock.mockResolvedValue({ text: 'Old answer.', model: 'haiku', host_alias: 'h', from: '1 task', at });
    const first = render(SummaryWidget, { props: { dashId: 'dash.r42', widget } });
    await fireEvent.click(first.getByTestId('summary-write'));
    await waitFor(() => expect(first.getByTestId('summary-text').textContent).toBe('Old answer.'));
    first.unmount();
    const r = render(SummaryWidget, { props: { dashId: 'dash.r42', widget } });
    expect(r.getByTestId('summary-text').textContent).toBe('Old answer.');
    // Days old reads as days old, not as a time of today.
    expect(r.getByTestId('summary-meta').textContent).toContain('written 3d ago');
    r.unmount();
    for (const other of <(typeof widget)[]>[
      { ...widget, prompt: 'Something else' },
      { ...widget, query: { status: 'done' } },
      { ...widget, sources: ['work', 'hosts'] as typeof widget.sources },
    ]) {
      const changed = render(SummaryWidget, { props: { dashId: 'dash.r42', widget: other } });
      expect(changed.queryByTestId('summary-text')).toBeNull();
      changed.unmount();
    }
    // A text kept by an older build (keyed by the prompt alone) is not shown.
    localStorage.setItem('cf:pref:' + summaryKey('dash.old', widget.title), JSON.stringify({ prompt: widget.prompt, draft: { text: 'x', model: 'm', host_alias: 'h', from: '', at } }));
    const old = render(SummaryWidget, { props: { dashId: 'dash.old', widget } });
    expect(old.queryByTestId('summary-text')).toBeNull();
  });

  it('sends only the feeds its dashboard names', async () => {
    mock.mockResolvedValue({ text: 'ok', model: 'haiku', host_alias: 'h', from: '1 task', at: 1 });
    const { dashboardFeeds } = await import('./dashboards');
    dashboardFeeds.set({
      deploys: { key: 'deploys', value: '3', by: 'person', updated_at: 1 },
      notes: { key: 'notes', value: 'long', by: 'person', updated_at: 1 },
    });
    const fleet = { sessions: [], hosts: [], accounts: [], usage: {} };
    const r = render(SummaryWidget, { props: { dashId: 'd', widget: { ...widget, sources: ['feeds'] }, fleet, feedKeys: ['deploys'] } });
    await fireEvent.click(r.getByTestId('summary-write'));
    await waitFor(() => expect(mock).toHaveBeenCalled());
    const sent = mock.mock.calls[0][1] as { args: { context: { feeds: Record<string, unknown> } } };
    expect(Object.keys(sent.args.context.feeds)).toEqual(['deploys']);
    dashboardFeeds.set({});
  });

  it('says why it was not written', async () => {
    mock.mockRejectedValue({ code: 'E_INVALID_STATE', message: 'no session of yours is running today' });
    const r = render(SummaryWidget, { props: { dashId: 'd', widget } });
    await fireEvent.click(r.getByTestId('summary-write'));
    await waitFor(() => expect(r.getByTestId('summary-error').textContent).toMatch(/Not written/));
  });
});
