// Work graph M13.2 / D24: Settings → Work → Usage — the counts as a
// read-only table, a window picker, and Copy as text in the same form as
// `fleet-hub work usage`; nothing on a paired desktop.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...a: unknown[]) => invoke(...a),
}));
vi.mock('./clipboard', () => ({ copyText: vi.fn(async () => true) }));

import WorkUsage from './WorkUsage.svelte';
import { copyText } from './clipboard';
import { usageDuration, usageRows, usageText, type UsageSummary } from './work_usage';

function summary(days = 30): UsageSummary {
  return {
    days,
    since: 1,
    until: 2,
    links: { created: 7, by_source: { started: 1, manual: 3, branch: 1 } },
    detection: {
      suggested: 4,
      confirmed_by_person: 1,
      promoted: 1,
      rejected: 1,
      expired: 1,
      median_decision_secs: 600,
      nudges: 1,
    },
    handover: { requested: 2, written: 1, missing: 0, send_failed: 1 },
    resume: { resumed: 3, with_brief: 1, without_brief: 2 },
    journal: { briefs_queued: 3, briefs_delivered: 1, compact_summaries: 1 },
    tidy: { applied: 1, kept: 1, auto_tidied: 3, auto_by_reason: { done_idle: 2, other: 1 } },
    trackers: [{ tracker_id: 4, passes: 12, passes_failed: 2, items_failed: 5 }],
    unrecorded: ['suggestions shown (only made, confirmed, rejected and expired are stored)'],
  };
}

describe('usage text (pure)', () => {
  it('reads line for line like fleet-hub work usage', () => {
    expect(usageText(summary()).split('\n')).toEqual([
      'work graph usage, last 30 d',
      'links: 7 made (branch 1, manual 3, started 1)',
      'detection: 4 suggested, 1 confirmed by a person, 1 promoted, 1 rejected, 1 expired; median decision 10 min; 1 nudges',
      'handover: 2 requested, 1 written, 0 missing, 1 send failed',
      'resume: 3 (1 with a brief, 2 without)',
      'journal: 3 briefs queued, 1 delivered; 1 compaction summaries',
      'tidy: 1 applied, 1 kept, 3 auto-tidied (done_idle 2, other 1)',
      'tracker 4: 12 passes, 2 failed, 5 items skipped (since the sync started)',
      'not recorded: suggestions shown (only made, confirmed, rejected and expired are stored)',
    ]);
  });

  it('reads a sparse answer (null-stripped, an empty store) as zeros', () => {
    const rows = usageRows({ days: 7, since: 1, until: 2 });
    expect(rows[0]).toEqual(['links', '0 made (none)']);
    expect(rows.find(([g]) => g === 'detection')![1]).toContain('median decision n/a');
    expect(rows.at(-1)).toEqual(['trackers', 'none']);
  });

  it('says a duration in its largest whole unit', () => {
    expect([59, 600, 3 * 3600, 5 * 86_400].map(usageDuration)).toEqual(['59 s', '10 min', '3 h', '5 d']);
  });
});

describe('WorkUsage', () => {
  beforeEach(() => {
    invoke.mockReset();
    vi.mocked(copyText).mockClear();
  });

  it('loads 30 days, renders one row per group, and copies the text', async () => {
    invoke.mockResolvedValue(summary());
    render(WorkUsage);
    await waitFor(() => expect(screen.getAllByTestId('work-usage-row').length).toBe(7));
    expect(invoke).toHaveBeenCalledWith('work_usage', { days: 30 });
    expect(screen.getByTestId('work-usage-table').textContent).toContain('4 suggested');
    expect(screen.getByTestId('work-usage-unrecorded').textContent).toContain('suggestions shown');
    await fireEvent.click(screen.getByTestId('work-usage-copy'));
    expect(vi.mocked(copyText).mock.calls[0][0]).toBe(usageText(summary()));
    await waitFor(() => expect(screen.getByTestId('work-usage-copy').textContent).toBe('Copied'));
  });

  it('asks again for another window', async () => {
    invoke.mockResolvedValueOnce(summary()).mockResolvedValueOnce(summary(90));
    render(WorkUsage);
    await waitFor(() => expect(screen.getByTestId('work-usage-table')).toBeInTheDocument());
    await fireEvent.change(screen.getByTestId('work-usage-days'), { target: { value: '90' } });
    await waitFor(() => expect(invoke).toHaveBeenLastCalledWith('work_usage', { days: 90 }));
  });

  it('shows a refusal as an error', async () => {
    invoke.mockRejectedValue({ code: 'E_LOCAL_ONLY', message: 'read them on the hub' });
    render(WorkUsage);
    await waitFor(() => expect(screen.getByTestId('work-usage-error').textContent).toContain('on the hub'));
    expect(screen.queryByTestId('work-usage-table')).toBeNull();
  });
});
