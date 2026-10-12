import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock('./clipboard', () => ({ copyText: vi.fn(() => Promise.resolve(true)) }));

import RichText from './RichText.svelte';
import { composerDrafts } from './conversation';
import { TASK_223 } from './handover_fixture';

const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', ...o }) + '\n```';

beforeEach(() => composerDrafts.clear());

// The report from the request that asked for this, as the worker printed it.
const TASK_DONE = `FLEET_TASK_DONE_9d2404fdfabee30f21062b4435f38952
\`\`\`json
{
  "summary": "TASK-225 was not in the fleet store, so I read 'this feature' as missions and wrote docs/missions.md.",
  "outcome": "done",
  "tests_run": ["link check: every relative .md link in docs/missions.md and docs/README.md resolves (shell loop)"],
  "warnings": [
    "Ticket brief was ambiguous; 'this feature' was taken to mean missions",
    "Branch rebased onto origin/main (missions were not on the original base)"
  ],
  "blockers": [],
  "followups": ["Push the branch and open a PR"],
  "confidence": "medium"
}
\`\`\``;

describe('RichText', () => {
  it('draws a task report as a card, not as JSON', () => {
    render(RichText, { source: TASK_DONE, sessionId: 3 });
    expect(screen.getByTestId('rich-report-outcome').textContent).toContain('Done');
    expect(screen.getByTestId('rich-report-confidence').textContent).toBe('confidence medium');
    expect(screen.getByTestId('rich-report-warnings').querySelectorAll('li')).toHaveLength(2);
    expect(screen.getByTestId('rich-report-tests').textContent).toContain('link check');
    expect(screen.queryByTestId('rich-report-blockers')).toBeNull();
    expect(screen.queryByTestId('md-copy')).toBeNull(); // no raw JSON code block
  });

  it('puts a follow-up in the composer, and offers no button without a session', async () => {
    render(RichText, { source: TASK_DONE, sessionId: 3 });
    await fireEvent.click(screen.getByTestId('rich-report-followup'));
    expect(composerDrafts.get(3)).toBe('Push the branch and open a PR');
  });

  it('offers no actions for a read-only view', () => {
    render(RichText, { source: TASK_DONE + '\n\n' + ui({ kind: 'choices', options: [{ label: 'A', prompt: 'do A' }] }), sessionId: null });
    expect(screen.queryByTestId('rich-report-followup')).toBeNull();
    expect((screen.getByTestId('rich-choice') as HTMLButtonElement).disabled).toBe(true);
  });

  it('a choice fills the composer and sends nothing', async () => {
    render(RichText, { source: ui({ kind: 'choices', question: 'Next?', options: [{ label: 'PR', prompt: 'Open a draft PR' }, { label: 'Stop', prompt: 'Stop here' }] }), sessionId: 5 });
    const [, stop] = screen.getAllByTestId('rich-choice');
    await fireEvent.click(stop);
    expect(composerDrafts.get(5)).toBe('Stop here');
  });

  it('counts ticked steps of a tutorial', async () => {
    render(RichText, { source: ui({ kind: 'steps', title: 'Set up', steps: [{ title: 'Install', code: 'pnpm i' }, { title: 'Run' }] }), sessionId: 1 });
    expect(screen.getByTestId('rich-steps-progress').textContent).toBe('0 of 2 done');
    await fireEvent.click(screen.getAllByTestId('rich-step-check')[0]);
    expect(screen.getByTestId('rich-steps-progress').textContent).toBe('1 of 2 done');
  });

  it('a reply form puts its answers in the composer as JSON', async () => {
    const form = { spec: 'fleet.form/1', title: 'Deploy', submit: 'Use these', steps: [{ title: 'Target', fields: [{ name: 'env', type: 'select', label: 'Env', options: [['stg', 'Staging'], ['prod', 'Production']], value: 'stg' }] }] };
    render(RichText, { source: ui({ kind: 'form', form }), sessionId: 8 });
    await fireEvent.click(screen.getByTestId('form-submit'));
    await tick();
    expect(composerDrafts.get(8)).toBe('Answers to the form "Deploy":\n\n```json\n{\n  "env": "stg"\n}\n```');
    expect(screen.getByTestId('rich-form-sent')).toBeTruthy();
  });

  it('draws guide, callout and facts', () => {
    render(RichText, {
      source: [
        ui({ kind: 'guide', title: 'Missions', sections: [{ title: 'What', body: 'A **mission**' }, { title: 'How', body: 'Steps' }] }),
        ui({ kind: 'callout', tone: 'danger', title: 'Careful', body: 'Prod' }),
        ui({ kind: 'facts', items: [['Host', 'mercury']] }),
      ].join('\n\n'),
    });
    expect(screen.getByTestId('rich-guide').querySelectorAll('details')).toHaveLength(2);
    expect(screen.getByTestId('rich-callout').classList.contains('danger')).toBe(true);
    expect(screen.getByTestId('rich-facts').textContent).toContain('mercury');
  });

  it('shows a broken block as its code and why', () => {
    render(RichText, { source: ui({ kind: 'callout', tone: 'loud', body: 'x' }) });
    expect(screen.getByTestId('rich-invalid').textContent).toContain('`tone` must be one of');
    expect(screen.getByTestId('md-copy')).toBeTruthy();
  });

  it('draws a work handover as a card: key, open counts, sections, steps that only fill the composer', async () => {
    render(RichText, { source: `Here it is.\nWORK_HANDOVER_BEGIN_7c86\n${TASK_223}\nWORK_HANDOVER_END_7c86`, sessionId: 4 });
    expect(screen.queryByText(/WORK_HANDOVER_/)).toBeNull();
    expect(screen.getByTestId('rich-handover-key').textContent).toBe('TASK-223');
    expect(screen.getByTestId('rich-handover-title').textContent).toBe('Polish functionalities');
    expect(screen.getByTestId('rich-handover-blocked')).toBeTruthy();
    expect(screen.getByTestId('rich-handover-glance').textContent).toMatch(/2\s*Blockers.*3\s*Next steps.*2\s*Gotchas/);
    expect(screen.getByTestId('rich-handover-next').querySelectorAll('li')).toHaveLength(3);
    expect(screen.getByTestId('rich-handover-gotchas').querySelectorAll('li')).toHaveLength(2);
    const ask = screen.getAllByTestId('rich-handover-ask');
    await fireEvent.click(ask[2]);
    expect(composerDrafts.get(4)).toBe('Once it is closed, the branch and its worktree can be removed.');
  });

  it('keeps the handover card read-only without a session', () => {
    render(RichText, { source: 'WORK_HANDOVER_BEGIN_n1\nDone: the parser.\nWORK_HANDOVER_END_n1' });
    expect(screen.getByTestId('rich-handover-done').textContent).toContain('the parser.');
    expect(screen.queryByTestId('rich-handover-ask')).toBeNull();
    expect(screen.queryByTestId('rich-handover-glance')).toBeNull();
  });
});
