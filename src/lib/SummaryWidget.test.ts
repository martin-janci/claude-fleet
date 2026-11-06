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

  it('shows the text written last for the same request only', async () => {
    const draft = { text: 'Old answer.', model: 'haiku', host_alias: 'h', from: '1 task', at: 1 };
    localStorage.setItem('cf:pref:' + summaryKey('dash.r42', widget.title), JSON.stringify({ prompt: widget.prompt, draft }));
    const r = render(SummaryWidget, { props: { dashId: 'dash.r42', widget } });
    expect(r.getByTestId('summary-text').textContent).toBe('Old answer.');
    r.unmount();
    const changed = render(SummaryWidget, { props: { dashId: 'dash.r42', widget: { ...widget, prompt: 'Something else' } } });
    expect(changed.queryByTestId('summary-text')).toBeNull();
  });

  it('says why it was not written', async () => {
    mock.mockRejectedValue({ code: 'E_INVALID_STATE', message: 'no session of yours is running today' });
    const r = render(SummaryWidget, { props: { dashId: 'd', widget } });
    await fireEvent.click(r.getByTestId('summary-write'));
    await waitFor(() => expect(r.getByTestId('summary-error').textContent).toMatch(/Not written/));
  });
});
