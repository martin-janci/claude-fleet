// Plan v2 R10: send the failing checks / the review comments to the agent.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('./sessions', async () => {
  const actual = await vi.importActual<typeof import('./sessions')>('./sessions');
  return { ...actual, queuePrompt: vi.fn() };
});

import PrFeedback from './PrFeedback.svelte';
import { session, NOW } from './hosts_fixture';
import { queuePrompt, type PrEvidence } from './sessions';
import { prFeedbackSends } from './pr_feedback';

const mockedQueue = vi.mocked(queuePrompt);
const ev = (over: Partial<PrEvidence> = {}): PrEvidence => ({
  draft: false,
  state: 'OPEN',
  checks: { total: 3, pending: 0, skipped: 0, failing_total: 1, failing: [{ name: 'rust', url: 'https://ci/9' }] },
  ...over,
});
const row = (over = {}) =>
  session('mefistos', 'pay', { pr_url: 'https://github.com/o/r/pull/1', pr_checked_at: NOW - 120, pr_evidence: ev(), ...over });
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  prFeedbackSends.set({});
  mockedQueue.mockReset();
  mockedQueue.mockResolvedValue({ ok: true, value: { session_id: 1, delivered: false, queued_id: 4 } });
});

describe('PrFeedback', () => {
  it('sends the failing checks as one queued prompt, once per reading', async () => {
    render(PrFeedback, { session: row() });
    const btn = screen.getByTestId('pr-feedback-checks');
    expect(btn).toHaveTextContent('Send failing checks to Claude');
    await fireEvent.click(btn);
    await fireEvent.click(btn);
    await settle();
    expect(mockedQueue).toHaveBeenCalledTimes(1);
    const [, prompt] = mockedQueue.mock.calls[0];
    expect(prompt).toContain('CI is failing on o/r#1 (https://github.com/o/r/pull/1)');
    expect(prompt).toContain('- "rust" ("https://ci/9")');
    expect(screen.getByTestId('pr-feedback-checks-sent')).toHaveTextContent('Waits for the turn to end');
    expect(btn).toBeDisabled();
  });

  it('sends the review comments', async () => {
    render(PrFeedback, { session: row() });
    await fireEvent.click(screen.getByTestId('pr-feedback-review'));
    await settle();
    expect(mockedQueue.mock.calls[0][1]).toContain('--comments');
  });

  it('offers no checks button while nothing fails, and nothing on a merged PR or a shell', () => {
    const { unmount } = render(PrFeedback, {
      session: row({ pr_evidence: ev({ checks: { total: 3, pending: 0, skipped: 0, failing_total: 0 } }) }),
    });
    expect(screen.queryByTestId('pr-feedback-checks')).toBeNull();
    expect(screen.getByTestId('pr-feedback-review')).toBeTruthy();
    unmount();
    const merged = render(PrFeedback, { session: row({ pr_evidence: ev({ state: 'MERGED' }) }) });
    expect(screen.queryByTestId('pr-feedback')).toBeNull();
    merged.unmount();
    render(PrFeedback, { session: row({ kind: 'shell', agent: 'shell' }) });
    expect(screen.queryByTestId('pr-feedback')).toBeNull();
  });

  it('names the agent the session runs', () => {
    render(PrFeedback, { session: row({ agent: 'codex' }) });
    expect(screen.getByTestId('pr-feedback-review')).toHaveTextContent('Send review comments to Codex');
  });

  it('files a reply under the session it was sent for, and remembers it across mounts and re-reads', async () => {
    let reply!: (v: unknown) => void;
    mockedQueue.mockImplementation(() => new Promise((res) => (reply = res as never)) as never);
    const a = row({ pr_evidence: ev({ head_oid: 'aaa' }) });
    const b = { ...row({ pr_evidence: ev({ head_oid: 'bbb' }) }), id: a.id + 1 };
    const view = render(PrFeedback, { session: a });
    await fireEvent.click(screen.getByTestId('pr-feedback-review'));
    // Switch to B before A's reply comes back.
    await view.rerender({ session: b });
    reply({ ok: true, value: { session_id: a.id, delivered: true, queued_id: null } });
    await settle();
    expect(screen.queryByTestId('pr-feedback-review-sent')).toBeNull();
    expect(screen.getByTestId('pr-feedback-review')).not.toBeDisabled();
    // Back on A, a fresh mount and a newer reading of the same head: still sent.
    view.unmount();
    render(PrFeedback, { session: { ...a, pr_checked_at: (a.pr_checked_at ?? 0) + 600 } });
    expect(screen.getByTestId('pr-feedback-review-sent')).toHaveTextContent('Sent');
    expect(screen.getByTestId('pr-feedback-review')).toBeDisabled();
  });

  it('shows why a send failed', async () => {
    mockedQueue.mockResolvedValue({ ok: false, error: { code: 'E_NOT_FOUND', message: 'session is gone' } } as never);
    render(PrFeedback, { session: row() });
    await fireEvent.click(screen.getByTestId('pr-feedback-review'));
    await settle();
    expect(screen.getByTestId('pr-feedback-error')).toBeTruthy();
    expect(screen.getByTestId('pr-feedback-review')).not.toBeDisabled();
  });
});
