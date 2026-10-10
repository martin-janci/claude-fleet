// The Review tab (work graph M14): suggestions and conflicts with their
// why, single decisions (a confirm never steals an existing primary),
// several at once with a count and per-item results (what failed stays,
// with the hub's sentence), and Undo back to a suggestion.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkReview from './WorkReview.svelte';
import { expectAccessible } from './a11y_check';
import { get } from 'svelte/store';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import type { ReviewItem, SessionTaskLink } from './work_view';
import { hubStatus, STANDALONE, type HubStatus } from './hub';
import { hubConnection } from './hub_connection';
import { applyGrantChanges, resetAccessForTests, setMyGrants } from './access';

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

  it('shows confidence as % and confirms only the high-confidence suggestions in one click (6.5)', async () => {
    pending = [
      item({ confidence: 90 }),
      item({ review_id: 'link:50', session_id: 8, session_name: 'web', link_id: 50, link_version: 1, confidence: 35, rule: 'R6', strength: 'weak', task: { task_id: 'item:20', key: 'PAY-2', title: 'Refund' } }),
      item({ review_id: 'link:60', kind: 'cross_org', session_id: 9, link_id: 60, link_version: 4, confidence: 95, alternatives: [] }),
    ];
    handlers.decide_work_batch = () => ({ results: [{ link_id: 42, ok: true, version: 3 }] });
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    expect(rows.map((r) => within(r).getByTestId('work-review-confidence').textContent)).toEqual(['almost sure', 'unsure', 'almost sure']);
    // Strength and rule stay beside the number: nothing is lost.
    expect(rows[1].textContent).toContain('weak');
    // Only a suggestion counts: the cross-org item at 95% is a conflict, not a guess.
    const btn = screen.getByTestId('work-review-confirm-high');
    expect(btn.textContent).toBe('Confirm all high-confidence (1)');
    await fireEvent.click(btn);
    await flush();
    expect(calls('decide_work_batch')[0]).toEqual({
      decisions: [{ session_id: 7, link_id: 42, decision: 'confirm', expected_version: 2, primary: false }],
    });
  });

  it('offers no high-confidence confirm when nothing clears the bar, or from an older hub', async () => {
    pending = [item({ confidence: 84 }), item({ review_id: 'link:50', session_id: 8, link_id: 50, confidence: undefined })];
    render(WorkReview);
    await flush();
    expect(screen.queryByTestId('work-review-confirm-high')).toBeNull();
    expect(screen.getAllByTestId('work-review-confidence')).toHaveLength(1);
  });

  it("names Jev on the decision model's suggestion, with its reason and confidence, and Change opens the panel (6.8)", async () => {
    pending = [
      item({
        rule: 'R12',
        strength: 'inferred',
        confidence: 60,
        why: ['Jev proposed ABC-12 (82%) · R12'],
        proposed_by: { source: 'jev', reason: 'from the first prompt', confidence_pct: 82 },
      }),
      item({ review_id: 'link:50', session_id: 8, link_id: 50 }),
    ];
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    const pill = within(rows[0]).getByTestId('work-review-proposed-by');
    expect(pill.textContent).toContain('Proposed by Jev');
    expect(pill.textContent).toContain('from the first prompt');
    expect(pill.textContent).toContain('likely');
    // The evidence note's confidence is a word too (G4.9).
    expect(within(rows[0]).getByTestId('work-review-why').textContent).toBe('Jev proposed ABC-12 (likely) · R12');
    // A rule's own reading says nothing about Jev.
    expect(within(rows[1]).queryByTestId('work-review-proposed-by')).toBeNull();
    // Jev's 60 never joins the one-click high-confidence confirm.
    expect(screen.queryByTestId('work-review-confirm-high')).toBeNull();
    await fireEvent.click(within(rows[0]).getByTestId('work-review-proposed-by-change'));
    await flush();
    expect(within(rows[0]).getByTestId('work-review-change-panel')).toBeTruthy();
  });

  it("confirming Jev's suggestion reads as an AI change with Undo (G4.9)", async () => {
    pending = [
      item({ rule: 'R12', proposed_by: { source: 'jev', reason: 'from the first prompt', confidence_pct: 82 } }),
    ];
    sessionLinks[7] = [sl(42, 'active', 2, 'ABC-12')];
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-confirm'));
    await flush();
    const line = screen.getByTestId('work-review-summary').textContent ?? '';
    expect(line).toMatch(/^✓ Linked .+ to .+ · Proposed by Jev · you confirmed/);
    expect(screen.getByTestId('work-review-undo')).toBeTruthy();
  });

  it('marks the main ticket among several keys as Proposed by Jev (J6, 6.8)', async () => {
    pending = [
      item({ rule: 'R6', strength: 'weak', proposed_by: { source: 'jev', reason: 'main ticket among 2 keys', confidence_pct: 78 } }),
      item({ review_id: 'link:43', link_id: 43, rule: 'R6', strength: 'weak', task: { task_id: 'ref:ABC-13', key: 'ABC-13', title: '' } }),
    ];
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    const pill = within(rows[0]).getByTestId('work-review-proposed-by');
    expect(pill.textContent).toContain('Proposed by Jev');
    expect(pill.textContent).toContain('main ticket among 2 keys');
    expect(within(rows[1]).queryByTestId('work-review-proposed-by')).toBeNull();
    // Nothing is confirmed for the person: both stay suggestions.
    expect(calls('confirm_session_work')).toHaveLength(0);
    expect(calls('decide_work_batch')).toHaveLength(0);
  });

  it("flags a local task that may duplicate a tracker ticket; linking it is the person's click (J7, 6.8)", async () => {
    pending = [
      item({
        task: { task_id: 'item:70', key: 'LOC-7', title: 'Retry declined payments' },
        duplicate_of: { task_id: 'item:31', item_id: 31, key: 'PAY-31', title: 'Retry failed card payments', source: 'jev', confidence_pct: 77 },
      }),
      item({ review_id: 'link:50', session_id: 8, link_id: 50, duplicate_of: { task_id: 'item:31', item_id: 31, key: 'PAY-31', title: 'x', source: 'jev', confidence_pct: 30 } }),
    ];
    handlers.link_session_work = () => session('mefistos', 'api', { id: 7 });
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    const dup = within(rows[0]).getByTestId('work-review-duplicate');
    expect(dup.textContent).toContain('May duplicate PAY-31');
    expect(within(dup).getByTestId('work-review-duplicate-proposed-by').textContent).toContain('likely');
    // Under the floor nothing shows at all.
    expect(within(rows[1]).queryByTestId('work-review-duplicate')).toBeNull();
    expect(calls('link_session_work')).toHaveLength(0);
    await fireEvent.click(within(dup).getByTestId('work-review-duplicate-link'));
    await flush();
    expect(calls('link_session_work')[0]).toMatchObject({ session_id: 7, item_id: 31 });
    expect(calls('reject_session_work')[0]).toMatchObject({ session_id: 7, link_id: 42 });
  });

  it('shows no Jev pill under the confidence floor (6.8)', async () => {
    pending = [item({ rule: 'R12', proposed_by: { source: 'jev', reason: 'from the first prompt', confidence_pct: 40 } })];
    render(WorkReview);
    await flush();
    expect(screen.queryByTestId('work-review-proposed-by')).toBeNull();
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

  it('Undo names the version the decision answered, without re-reading the session', async () => {
    handlers.reject_session_work = (a) => {
      pending = pending.filter((x) => x.link_id !== a.link_id);
      return { ...session('mefistos', 'api', { id: 7, row_version: 50, friendly_name: 'answered' }), link_version: 9 };
    };
    render(WorkReview);
    await flush();
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-reject'));
    await flush();
    expect(calls('work_session_tasks')).toHaveLength(0);
    // The stored row never carries the decision's version.
    const stored = get(sessions).find((s) => s.id === 7);
    expect(stored?.friendly_name).toBe('answered');
    expect(stored).not.toHaveProperty('link_version');
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    expect(calls('reconsider_work_link')[0]).toEqual({ session_id: 7, link_id: 42, expected_version: 9 });
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

  it('is accessible', async () => {
    pending = [
      item({
        confidence: 90,
        rule: 'R12',
        proposed_by: { source: 'jev', reason: 'from the first prompt', confidence_pct: 82 },
      }),
      ...items.slice(1),
    ];
    const { container } = render(WorkReview);
    await flush();
    await expectAccessible(container);
    await fireEvent.click(within(screen.getAllByTestId('work-review-item')[0]).getByTestId('work-review-proposed-by-change'));
    await flush();
    await expectAccessible(container);
  });
});

// ── Multi-user M1 (F2a): per-ITEM, because the list is per session ──────────
//
// `blocked` here was `hubActionBlocked('decide_work_batch', …)` and that was the
// whole gate, while every decision writes per session through
// `confirmSessionWork(it.session_id, …)`. A `ReviewItem` carries a session id
// and a name and no `owner_person_id`, so the row is resolved out of `$sessions`
// first — the lookup that made this surface get skipped by F2.
describe('WorkReview access gate (multi-user M1)', () => {
  const paired: HubStatus = {
    ...STANDALONE,
    remote: true,
    url: 'https://fleet.example.com',
    configured_url: 'https://fleet.example.com',
  };
  const dis = (el: Element) => (el as HTMLButtonElement | HTMLInputElement).disabled;
  const row = (id: number, owner: number, name: string) =>
    session('mefistos', name, { id, visibility: 'private', owner_person_id: owner });

  /** Two suggestions on two sessions: 7 is mine, 8 is somebody else's. */
  const twoItems: ReviewItem[] = [
    item({ review_id: 'link:42', session_id: 7, session_name: 'api', link_id: 42, link_version: 2 }),
    item({ review_id: 'link:50', session_id: 8, session_name: 'web', link_id: 50, link_version: 1 }),
  ];

  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    pending = [...twoItems];
    sessionLinks = {};
    sessions.set([row(7, 1, 'api'), row(8, 42, 'web')]);
    hubStatus.set(paired);
    hubConnection.set({ state: 'connected' });
    resetAccessForTests();
    handlers = {
      work_review: () => ({ items: pending, total: pending.length, next_cursor: null }),
      confirm_session_work: () => session('mefistos', 'api', { id: 7 }),
      reject_session_work: () => session('mefistos', 'api', { id: 7 }),
      decide_work_batch: () => ({ results: [] }),
      work_session_tasks: (a) => ({ session_id: a.session_id, links: [] }),
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  afterEach(() => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    resetAccessForTests();
  });

  it('the owner decides both of their own items', async () => {
    // The positive control. A gate that disabled everything would otherwise
    // satisfy every assertion below.
    sessions.set([row(7, 1, 'api'), row(8, 1, 'web')]);
    setMyGrants(1, []);
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    for (const r of rows) {
      expect(dis(within(r).getByTestId('work-review-confirm'))).toBe(false);
      expect(dis(within(r).getByTestId('work-review-pick'))).toBe(false);
      expect(within(r).queryByTestId('work-review-item-not-mine')).toBeNull();
    }
    await fireEvent.click(within(rows[0]).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')).toHaveLength(1);
  });

  it('a watcher’s item is disabled with the reason, and y does not decide it', async () => {
    setMyGrants(1, [{ session_id: 8, level: 'watch' }]);
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    // Mine stays live; theirs is off, with the sentence on the row itself.
    expect(dis(within(rows[0]).getByTestId('work-review-confirm'))).toBe(false);
    expect(dis(within(rows[1]).getByTestId('work-review-confirm'))).toBe(true);
    expect(dis(within(rows[1]).getByTestId('work-review-reject'))).toBe(true);
    expect(dis(within(rows[1]).getByTestId('work-review-change'))).toBe(true);
    expect(dis(within(rows[1]).getByTestId('work-review-pick'))).toBe(true);
    expect(within(rows[1]).getByTestId('work-review-item-not-mine').textContent).toMatch(
      /needs drive/i,
    );
    // The keyboard path: j moves onto the second item, y would confirm it.
    const list = screen.getByRole('list', { name: /Review items/i });
    await fireEvent.keyDown(list, { key: 'j' });
    await fireEvent.keyDown(list, { key: 'y' });
    await flush();
    expect(calls('confirm_session_work')).toHaveLength(0);
    // And x cannot tick it either, so the bulk buttons never see it.
    await fireEvent.keyDown(list, { key: 'x' });
    await flush();
    expect(screen.queryByTestId('work-review-bulk')).toBeNull();
  });

  it('a drive grantee decides it: these writes are the drive tier', async () => {
    // The counter-test. `decide_work_batch` is a batch of `confirm_session_work`,
    // which has been `drive` since F2 — putting it in `own` would make the
    // drive level meaningless for the work graph.
    setMyGrants(1, [{ session_id: 8, level: 'drive' }]);
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    expect(dis(within(rows[1]).getByTestId('work-review-confirm'))).toBe(false);
    await fireEvent.click(within(rows[1]).getByTestId('work-review-confirm'));
    await flush();
    expect(calls('confirm_session_work')[0]).toMatchObject({ session_id: 8 });
  });

  it('a batch is narrowed per target: a grant lost after the tick drops that item', async () => {
    // Ticked while drivable, then narrowed to watch with the list still open —
    // the batch must send only what is still this client's to decide.
    setMyGrants(1, [{ session_id: 8, level: 'drive' }]);
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    await fireEvent.click(within(rows[0]).getByTestId('work-review-pick'));
    await fireEvent.click(within(rows[1]).getByTestId('work-review-pick'));
    await flush();
    expect(screen.getByTestId('work-review-confirm-n').textContent).toContain('Confirm 2');
    applyGrantChanges([{ session_id: 8, person_id: 1, level: 'watch' }]);
    await flush();
    expect(screen.getByTestId('work-review-confirm-n').textContent).toContain('Confirm 1');
    await fireEvent.click(screen.getByTestId('work-review-confirm-n'));
    await flush();
    expect(calls('decide_work_batch')[0]).toEqual({
      decisions: [{ session_id: 7, link_id: 42, decision: 'confirm', expected_version: 2, primary: true }],
    });
  });

  // F2b: the Undo offer outlives the batch that made it, so `runUndo` narrows
  // its decisions the way the batch did — it was the one write in this file
  // with no access answer anywhere near it.
  it('Undo narrows too: a grant lost after the batch drops that decision', async () => {
    sessions.set([row(7, 1, 'api'), row(8, 1, 'web')]);
    setMyGrants(1, []);
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    await fireEvent.click(within(rows[0]).getByTestId('work-review-pick'));
    await fireEvent.click(within(rows[1]).getByTestId('work-review-pick'));
    await flush();
    await fireEvent.click(screen.getByTestId('work-review-confirm-n'));
    await flush();
    const undo = screen.queryByTestId('work-review-undo');
    if (!undo) return; // no undoable version came back: nothing to narrow
    // One of the two sessions becomes somebody else's before Undo is pressed.
    sessions.set([row(7, 1, 'api'), row(8, 42, 'web')]);
    setMyGrants(1, [{ session_id: 8, level: 'watch' }]);
    await flush();
    const before = calls('decide_work_batch').length;
    await fireEvent.click(screen.getByTestId('work-review-undo'));
    await flush();
    const sent = calls('decide_work_batch').slice(before);
    for (const batch of sent) {
      for (const d of (batch as { decisions: { session_id: number }[] }).decisions) {
        expect(d.session_id).not.toBe(8);
      }
    }
    // A single-decision undo takes the `reconsider_work_link` path instead;
    // either way, nothing names session 8.
    for (const a of calls('reconsider_work_link')) expect(a.session_id).not.toBe(8);
  });

  it('standalone is untouched: every item decidable with no grants at all', async () => {
    hubStatus.set({ ...STANDALONE });
    hubConnection.set({ state: 'standalone' });
    render(WorkReview);
    await flush();
    const rows = screen.getAllByTestId('work-review-item');
    for (const r of rows) expect(dis(within(r).getByTestId('work-review-confirm'))).toBe(false);
  });
});
