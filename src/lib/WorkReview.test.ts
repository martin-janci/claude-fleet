// The Review tab (work graph M14): suggestions and conflicts with their
// why, single decisions (a confirm never steals an existing primary),
// several at once with a count and per-item results (what failed stays,
// with the hub's sentence), and Undo back to a suggestion.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkReview from './WorkReview.svelte';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import type { ReviewItem, SessionTaskLink } from './work_view';

const item = (over: Partial<ReviewItem> = {}): ReviewItem => ({
  review_id: 'link:42',
  kind: 'suggestion',
  session_id: 7,
  session_name: 'api',
  host: 'mefistos',
  link_id: 42,
  link_version: 2,
  task: { task_id: 'item:12', key: 'ABC-12', title: 'Login fails', org_id: 1 },
  why: ['branch abc-12-login since 09:05 · R3'],
  strength: 'strong',
  rule: 'R3',
  preselected: false,
  alternatives: [{ link_id: 43, task_id: 'ref:ABC-13', key: 'ABC-13', title: '' }],
  created_at: 1790000000,
  ...over,
});

const items: ReviewItem[] = [
  item(),
  item({ review_id: 'link:50', session_id: 8, session_name: 'web', link_id: 50, link_version: 1, task: { task_id: 'item:20', key: 'PAY-2', title: 'Refund' } }),
  item({ review_id: 'link:60', kind: 'cross_org', session_id: 9, link_id: 60, link_version: 4, why: ['links Acme work to a Globex session'], alternatives: [] }),
  item({ review_id: 'np:9', kind: 'no_primary', session_id: 9, link_id: 61, link_version: 1, why: ['2 tasks, none primary'], alternatives: [] }),
];

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
let pending: ReviewItem[];
// session id → its links, as `work { session_tasks }` answers them.
let sessionLinks: Record<number, SessionTaskLink[]>;

const sl = (link_id: number, state: string, link_version: number, key: string): SessionTaskLink => ({
  link_id,
  link_version,
  state,
  task: { task_id: `ref:${key}`, key, title: '' },
});

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

describe('WorkReview', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    pending = [...items];
    sessionLinks = {};
    // Session 7 has a primary already; 8 has none.
    sessions.set([
      session('mefistos', 'api', { id: 7, work: { link_id: 1, item_id: 1, key: 'OPS-1', title: 'x', source: 'manual' } }),
      session('mefistos', 'web', { id: 8 }),
      session('mefistos', 'db', { id: 9 }),
    ]);
    handlers = {
      work_review: () => ({ items: pending, total: pending.length, next_cursor: null }),
      confirm_session_work: (a) => {
        pending = pending.filter((x) => x.link_id !== a.link_id);
        return session('mefistos', 'api', { id: 7 });
      },
      reject_session_work: (a) => {
        pending = pending.filter((x) => x.link_id !== a.link_id);
        return session('mefistos', 'api', { id: 7 });
      },
      reconsider_work_link: () => session('mefistos', 'api', { id: 7 }),
      ack_work_link: () => session('mefistos', 'db', { id: 9 }),
      set_primary_work: () => session('mefistos', 'db', { id: 9 }),
      work_session_tasks: (a) => ({ session_id: a.session_id, links: sessionLinks[a.session_id as number] ?? [] }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  it('lists every item with its kind and why', async () => {
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    expect(rows.map((r) => within(r).getByTestId('work-review-kind').textContent)).toEqual([
      'suggestion',
      'suggestion',
      'cross-org',
      'no primary',
    ]);
    expect(within(rows[0]).getByTestId('work-review-why').textContent).toBe('branch abc-12-login since 09:05 · R3');
    expect(within(rows[2]).getByTestId('work-review-keep')).toBeTruthy();
    expect(within(rows[3]).getByTestId('work-review-make-primary')).toBeTruthy();
  });

  it('a confirm keeps an existing primary; a session with none gets it; Undo reconsiders at the version it left', async () => {
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')[0]).toEqual({ session_id: 7, link_id: 42, primary: false, expected_version: 2 });
    sessionLinks[8] = [sl(50, 'active', 2, 'PAY-2')];
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')[1]).toEqual({ session_id: 8, link_id: 50, primary: true, expected_version: 1 });
    expect(screen.getByTestId('work-review-summary').textContent).toContain('Confirmed: PAY-2 Refund · web');
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    // A compare-and-set: the version the confirm left, read back.
    expect(calls('reconsider_work_link')[0]).toEqual({ session_id: 8, link_id: 50, expected_version: 2 });
    expect(screen.getByTestId('work-review-summary').textContent).toContain('Undone');
  });

  it('no Undo when the link has moved on since (or its version cannot be read)', async () => {
    // Someone else confirmed it between this reject and the read-back.
    sessionLinks[7] = [sl(42, 'active', 5, 'ABC-12')];
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-reject'));
    await flush();
    expect(screen.getByTestId('work-review-summary').textContent).toContain('Rejected');
    expect(screen.queryByTestId('work-review-undo')).toBeNull();
    handlers.work_session_tasks = () => {
      throw { code: 'E_HUB_DOWN', message: 'hub unreachable' };
    };
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-reject'));
    await flush();
    expect(screen.queryByTestId('work-review-undo')).toBeNull();
    expect(calls('reconsider_work_link')).toHaveLength(0);
  });

  it('an Undo that lost a race shows the current value with Reload', async () => {
    sessionLinks[7] = [sl(42, 'active', 3, 'ABC-12')];
    handlers.reconsider_work_link = () => {
      throw {
        code: 'E_CONFLICT',
        message: 'work link 42 was changed by someone else meanwhile (now version 4)',
        details: { link_id: 42, version: 4, state: 'rejected', primary: false, ended: false },
      };
    };
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-confirm'));
    await flush();
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    expect(calls('reconsider_work_link')[0]).toEqual({ session_id: 7, link_id: 42, expected_version: 3 });
    expect(screen.getByTestId('work-review-summary').textContent).toContain('Undo failed');
  });

  it('a decision that lost a race shows the current value; Reload re-reads', async () => {
    handlers.confirm_session_work = () => {
      throw {
        code: 'E_CONFLICT',
        message: 'work link 42 was changed by someone else meanwhile (now version 4)',
        details: { link_id: 42, version: 4, state: 'rejected', primary: false, ended: false },
      };
    };
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-confirm'));
    await flush();
    const row = screen.getAllByTestId('work-review-item')[0];
    expect(within(row).getByTestId('work-review-item-error').textContent).toContain('changed elsewhere');
    expect(within(row).getByTestId('work-conflict-current').textContent).toBe('Now: rejected · version 4');
    const reads = calls('work_review').length;
    await fireEvent.click(within(row).getByTestId('work-conflict-reload'));
    await flush();
    expect(calls('work_review').length).toBe(reads + 1);
  });

  it('Keep acknowledges a conflict; Make primary sets it from none', async () => {
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[2]).getByTestId('work-review-keep'));
    await flush();
    expect(calls('ack_work_link')[0]).toEqual({ session_id: 9, link_id: 60, expected_version: 4 });
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[3]).getByTestId('work-review-make-primary'));
    await flush();
    expect(calls('set_primary_work')[0]).toEqual({ session_id: 9, link_id: 61, expected_primary: 0 });
  });

  it('Change… picks an alternative: confirm it at its version, then reject the guess', async () => {
    sessionLinks[7] = [sl(42, 'suggested', 2, 'ABC-12'), sl(43, 'suggested', 5, 'ABC-13')];
    render(WorkReview);
    await flush();
    const first = screen.getAllByTestId('work-review-item')[0];
    await fireEvent.click(within(first).getByTestId('work-review-change'));
    await fireEvent.click(within(first).getByTestId('work-review-alt'));
    await flush();
    expect(calls('confirm_session_work')[0]).toEqual({ session_id: 7, link_id: 43, primary: false, expected_version: 5 });
    expect(calls('reject_session_work')[0]).toEqual({ session_id: 7, link_id: 42, expected_version: 2 });
    expect(screen.getByTestId('work-review-summary').textContent).toContain('Changed');
  });

  it('Change… whose reject is refused takes the alternative back to a suggestion', async () => {
    sessionLinks[7] = [sl(42, 'suggested', 2, 'ABC-12'), sl(43, 'suggested', 5, 'ABC-13')];
    handlers.confirm_session_work = (a) => {
      sessionLinks[7] = sessionLinks[7].map((l) => (l.link_id === a.link_id ? { ...l, state: 'active', link_version: 6 } : l));
      return session('mefistos', 'api', { id: 7 });
    };
    handlers.reject_session_work = () => {
      throw { code: 'E_CONFLICT', message: 'work link 42 changed', details: { link_id: 42, version: 3, state: 'confirmed', primary: false } };
    };
    render(WorkReview);
    await flush();
    const first = screen.getAllByTestId('work-review-item')[0];
    await fireEvent.click(within(first).getByTestId('work-review-change'));
    await fireEvent.click(within(first).getByTestId('work-review-alt'));
    await flush();
    expect(calls('reconsider_work_link')[0]).toEqual({ session_id: 7, link_id: 43, expected_version: 6 });
    const row = screen.getAllByTestId('work-review-item')[0];
    expect(within(row).getByTestId('work-conflict-current').textContent).toBe('Now: confirmed · version 3');
    expect(screen.queryByTestId('work-review-undo')).toBeNull();
  });

  it('Change… to a new key links it expecting none, and removes it again when the reject is refused', async () => {
    sessionLinks[7] = [sl(42, 'suggested', 2, 'ABC-12')];
    handlers.link_session_work = () => {
      sessionLinks[7] = [...sessionLinks[7], sl(70, 'active', 1, 'ABC-99')];
      return session('mefistos', 'api', { id: 7 });
    };
    handlers.reject_session_work = () => {
      throw { code: 'E_NOTFOUND', message: 'work link 42 not found' };
    };
    render(WorkReview);
    await flush();
    const first = screen.getAllByTestId('work-review-item')[0];
    await fireEvent.click(within(first).getByTestId('work-review-change'));
    await fireEvent.input(within(first).getByTestId('work-review-change-query'), { target: { value: 'abc-99' } });
    await fireEvent.click(within(first).getByTestId('work-review-change-key'));
    await flush();
    expect(calls('link_session_work')[0]).toEqual({ session_id: 7, key: 'abc-99', primary: false, expected_version: 0 });
    expect(calls('unlink_session_work')[0]).toEqual({ session_id: 7, link_id: 70, expected_version: 1 });
    expect(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-item-error').textContent).toContain(
      'work link 42 not found',
    );
  });

  it('several at once: the count, per-item results, failures stay with their reason, Undo the batch', async () => {
    handlers.decide_work_batch = () => {
      pending = pending.filter((x) => x.link_id !== 42);
      return {
        results: [
          { link_id: 42, ok: true, version: 3 },
          { link_id: 50, ok: false, code: 'E_CONFLICT', message: 'the link changed (version 2)' },
        ],
      };
    };
    render(WorkReview);
    await flush();
    const picks = screen.getAllByTestId('work-review-pick');
    await fireEvent.click(picks[0]);
    await fireEvent.click(picks[1]);
    await fireEvent.click(picks[2]);
    await flush();
    expect(screen.getByTestId('work-review-confirm-n').textContent).toBe('Confirm 2');
    expect(screen.getByTestId('work-review-reject-n').textContent).toBe('Reject 2');
    expect(screen.getByTestId('work-review-keep-n').textContent).toBe('Keep 1');
    await fireEvent.click(screen.getByTestId('work-review-confirm-n'));
    await flush();
    expect(calls('decide_work_batch')[0]).toEqual({
      decisions: [
        { session_id: 7, link_id: 42, decision: 'confirm', expected_version: 2, primary: false },
        { session_id: 8, link_id: 50, decision: 'confirm', expected_version: 1, primary: true },
      ],
    });
    expect(screen.getByTestId('work-review-summary').textContent).toContain('1 confirmed · 1 failed');
    // The failed item stays, ticked, with the hub's sentence.
    const failed = screen.getAllByTestId('work-review-item').find((r) => r.getAttribute('data-link-id') === '50')!;
    expect(within(failed).getByTestId('work-review-item-error').textContent).toBe('the link changed (version 2)');
    expect((within(failed).getByTestId('work-review-pick') as HTMLInputElement).checked).toBe(true);
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    // One decision to undo: reconsider it, at the version the batch answered.
    expect(calls('reconsider_work_link')[0]).toEqual({ session_id: 7, link_id: 42, expected_version: 3 });
  });

  it('Undo of several counts what was undone, item by item', async () => {
    handlers.decide_work_batch = (a) => {
      const ds = a.decisions as { link_id: number; decision: string }[];
      if (ds[0].decision === 'reconsider') {
        return {
          results: [
            { link_id: 42, ok: true, version: 4 },
            { link_id: 50, ok: false, code: 'E_CONFLICT', message: 'work link 50 was changed by someone else' },
          ],
        };
      }
      return { results: ds.map((d, i) => ({ link_id: d.link_id, ok: true, version: 3 + i })) };
    };
    render(WorkReview);
    await flush();
    const picks = screen.getAllByTestId('work-review-pick');
    await fireEvent.click(picks[0]);
    await fireEvent.click(picks[1]);
    await fireEvent.click(screen.getByTestId('work-review-reject-n'));
    await flush();
    expect(screen.getByTestId('work-review-summary').textContent).toContain('2 rejected');
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    expect(calls('decide_work_batch')[1]).toEqual({
      decisions: [
        { session_id: 7, link_id: 42, decision: 'reconsider', expected_version: 3 },
        { session_id: 8, link_id: 50, decision: 'reconsider', expected_version: 4 },
      ],
    });
    const summary = screen.getByTestId('work-review-summary').textContent ?? '';
    expect(summary).toContain('Undone: 1 of 2');
    expect(summary).not.toContain('back to suggestions');
    const failed = screen.getAllByTestId('work-review-item').find((r) => r.getAttribute('data-link-id') === '50')!;
    expect(within(failed).getByTestId('work-review-item-error').textContent).toContain('changed by someone else');
  });

  it('keyboard: j moves, y confirms the focused item', async () => {
    render(WorkReview);
    await flush();
    const list = screen.getAllByTestId('work-review-item')[0].parentElement!;
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(list, { key: 'y' });
    await flush();
    expect(calls('confirm_session_work')[0]).toMatchObject({ link_id: 50 });
  });

  it('empty, and an older hub', async () => {
    pending = [];
    const { unmount } = render(WorkReview);
    await flush();
    expect(screen.getByTestId('work-review-empty')).toBeTruthy();
    unmount();
    handlers.work_review = () => {
      throw { code: 'E_INVALID', message: 'unknown work action: review' };
    };
    render(WorkReview);
    await flush();
    expect(screen.getByTestId('work-review-error').textContent).toContain('Needs a newer hub');
  });
});
